//! Per-mirror request and byte budgets.
//!
//! Some mirrors publish their limits in response headers (`X-Ratelimit-*` for
//! catboy.best, `X-Download-Budget-*` for osu.direct); osu! official has a
//! documented hourly download quota. [`LimitState`] keeps the latest view of
//! those limits and answers "when may the next request go out?" *before* the
//! scheduler sends it. The AIMD spacing in [`pool`](super::pool) stays as the
//! fallback for mirrors that publish nothing.

use reqwest::header::HeaderMap;
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Quota units one catboy.best download costs on its `X-Ratelimit-*` budget.
const CATBOY_RATE_COST: u64 = 20;

/// Headroom kept on osu.direct's daily byte budget, so a set that is larger
/// than the running average cannot overshoot the last block of budget.
const OSU_DIRECT_BYTE_RESERVE: u64 = 64 * 1024 * 1024;

/// osu! official's hourly download quota for a free account.
const OSU_API_DOWNLOAD_LIMIT: u32 = 10;
const OSU_API_DOWNLOAD_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Reset used when a budget header arrives without its matching reset header.
/// Re-probing after a minute is safer than sidelining a mirror indefinitely.
const HEADER_RESET_DEFAULT: Duration = Duration::from_secs(60);

/// Upper clamp for a server-supplied reset: a bogus value can never sideline a
/// mirror for more than a week.
const HEADER_RESET_MAX: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const RATE_REMAINING: &str = "x-ratelimit-remaining";
const RATE_RESET: &str = "x-ratelimit-reset";
const BURST_REMAINING: &str = "x-ratelimit-burst-remaining";
const BURST_RESET: &str = "x-ratelimit-burst-reset";
const DAILY_DOWNLOADS_REMAINING: &str = "x-download-budget-remaining";
const DAILY_DOWNLOADS_RESET: &str = "x-download-budget-reset";
const DAILY_BYTES_REMAINING: &str = "x-download-budget-bytes-remaining";
const DAILY_BYTES_RESET: &str = "x-download-budget-bytes-reset";

/// A fixed request quota for mirrors that publish no budget headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaticQuotaSpec {
    pub(crate) limit: u32,
    pub(crate) window: Duration,
}

/// Per-kind budget policy: what one request costs and what headroom to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LimitPolicy {
    /// Quota units one download costs on the `X-Ratelimit-*` budget.
    pub(crate) rate_cost: u64,
    /// Extra bytes kept in reserve on a byte budget.
    pub(crate) byte_reserve: u64,
    /// A fixed quota for mirrors that publish no headers.
    pub(crate) static_quota: Option<StaticQuotaSpec>,
}

impl LimitPolicy {
    pub(crate) const fn simple() -> Self {
        Self {
            rate_cost: 1,
            byte_reserve: 0,
            static_quota: None,
        }
    }

    pub(crate) const fn catboy() -> Self {
        Self {
            rate_cost: CATBOY_RATE_COST,
            byte_reserve: 0,
            static_quota: None,
        }
    }

    pub(crate) const fn osu_direct() -> Self {
        Self {
            rate_cost: 1,
            byte_reserve: OSU_DIRECT_BYTE_RESERVE,
            static_quota: None,
        }
    }

    pub(crate) const fn osu_api() -> Self {
        Self {
            rate_cost: 1,
            byte_reserve: 0,
            static_quota: Some(StaticQuotaSpec {
                limit: OSU_API_DOWNLOAD_LIMIT,
                window: OSU_API_DOWNLOAD_WINDOW,
            }),
        }
    }
}

impl Default for LimitPolicy {
    fn default() -> Self {
        Self::simple()
    }
}

/// One server-reported budget: how much is left and when it resets.
#[derive(Debug, Clone, Copy)]
struct Budget {
    remaining: u64,
    reset_at: Instant,
}

/// A fixed request quota tracked as a sliding window of grants.
#[derive(Debug)]
struct StaticQuota {
    limit: u32,
    window: Duration,
    grants: VecDeque<Instant>,
}

impl StaticQuota {
    fn new(spec: StaticQuotaSpec) -> Self {
        Self {
            limit: spec.limit,
            window: spec.window,
            grants: VecDeque::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some(&first) = self.grants.front() {
            if first + self.window <= now {
                self.grants.pop_front();
            } else {
                break;
            }
        }
    }

    fn note_request(&mut self, now: Instant) {
        self.prune(now);
        self.grants.push_back(now);
    }

    fn blocked_until(&mut self, now: Instant) -> Option<Instant> {
        self.prune(now);
        if self.grants.len() >= self.limit as usize {
            self.grants.front().map(|first| *first + self.window)
        } else {
            None
        }
    }
}

/// Everything known about one mirror's request and byte limits.
#[derive(Debug)]
pub(crate) struct LimitState {
    policy: LimitPolicy,
    rate: Option<Budget>,
    burst: Option<Budget>,
    daily_downloads: Option<Budget>,
    daily_bytes: Option<Budget>,
    static_quota: Option<StaticQuota>,
    /// Bytes reserved by requests currently in flight, against `daily_bytes`.
    in_flight_bytes: u64,
    /// Rolling estimate of one set's size, used to reserve byte budget.
    est_bytes: u64,
}

impl LimitState {
    pub(crate) fn new(policy: LimitPolicy) -> Self {
        Self {
            policy,
            rate: None,
            burst: None,
            daily_downloads: None,
            daily_bytes: None,
            static_quota: policy.static_quota.map(StaticQuota::new),
            in_flight_bytes: 0,
            est_bytes: 0,
        }
    }

    /// Fold a response's budget headers into this slot. Missing headers leave
    /// the previous value in place; a mirror that stops publishing keeps its
    /// last known budget until its reset passes.
    pub(crate) fn observe_headers_at(&mut self, headers: &HeaderMap, now: Instant, now_unix: u64) {
        if let Some(budget) = read_budget(
            headers,
            RATE_REMAINING,
            RATE_RESET,
            self.rate,
            now,
            now_unix,
        ) {
            self.rate = Some(budget);
        }
        if let Some(budget) = read_budget(
            headers,
            BURST_REMAINING,
            BURST_RESET,
            self.burst,
            now,
            now_unix,
        ) {
            self.burst = Some(budget);
        }
        if let Some(budget) = read_budget(
            headers,
            DAILY_DOWNLOADS_REMAINING,
            DAILY_DOWNLOADS_RESET,
            self.daily_downloads,
            now,
            now_unix,
        ) {
            self.daily_downloads = Some(budget);
        }
        if let Some(budget) = read_budget(
            headers,
            DAILY_BYTES_REMAINING,
            DAILY_BYTES_RESET,
            self.daily_bytes,
            now,
            now_unix,
        ) {
            self.daily_bytes = Some(budget);
        }
    }

    /// When the next request can be afforded, or `None` if it can go now.
    ///
    /// Checks every live budget: the rate/burst units, the daily download
    /// count, the daily byte budget (including in-flight reservations), and a
    /// static sliding-window quota.
    pub(crate) fn blocked_until(&mut self, now: Instant, est_bytes: u64) -> Option<Instant> {
        self.prune(now);

        let mut until: Option<Instant> = None;
        let mut add = |budget: Option<Budget>, needed: u64| {
            if let Some(budget) = budget
                && budget.remaining < needed
            {
                until = Some(until.map_or(budget.reset_at, |t| t.max(budget.reset_at)));
            }
        };
        add(self.rate, self.policy.rate_cost);
        add(self.burst, 1);
        add(self.daily_downloads, 1);

        if let Some(budget) = self.daily_bytes {
            let available = budget.remaining.saturating_sub(self.in_flight_bytes);
            let needed = est_bytes.saturating_add(self.policy.byte_reserve);
            if available < needed {
                until = Some(until.map_or(budget.reset_at, |t| t.max(budget.reset_at)));
            }
        }

        if let Some(t) = self
            .static_quota
            .as_mut()
            .and_then(|quota| quota.blocked_until(now))
        {
            until = Some(until.map_or(t, |u| u.max(t)));
        }

        until
    }

    /// Record one request against the request-count budgets. Called for a
    /// granted download and for each extra request a mirror attempt makes
    /// (transient retries, size probes).
    pub(crate) fn note_request(&mut self, now: Instant) {
        self.prune(now);
        if let Some(budget) = &mut self.rate {
            budget.remaining = budget.remaining.saturating_sub(self.policy.rate_cost);
        }
        if let Some(budget) = &mut self.burst {
            budget.remaining = budget.remaining.saturating_sub(1);
        }
        if let Some(budget) = &mut self.daily_downloads {
            budget.remaining = budget.remaining.saturating_sub(1);
        }
        if let Some(quota) = &mut self.static_quota {
            quota.note_request(now);
        }
    }

    /// Reserve estimated bytes for a request that is about to start. Returns
    /// the reservation, to be handed back to [`finish_attempt`](Self::finish_attempt).
    pub(crate) fn reserve_bytes(&mut self, est_bytes: u64) -> u64 {
        if self.daily_bytes.is_none() {
            return 0;
        }
        self.in_flight_bytes = self.in_flight_bytes.saturating_add(est_bytes);
        est_bytes
    }

    /// Release a byte reservation and fold the completed set's size into the
    /// running estimate. `actual_bytes` is `None` for an attempt that produced
    /// no archive (404, 429, cancellation).
    pub(crate) fn finish_attempt(&mut self, reserved_bytes: u64, actual_bytes: Option<u64>) {
        self.in_flight_bytes = self.in_flight_bytes.saturating_sub(reserved_bytes);
        let Some(actual) = actual_bytes.filter(|bytes| *bytes > 0) else {
            return;
        };
        self.est_bytes = if self.est_bytes == 0 {
            actual
        } else {
            self.est_bytes.saturating_mul(3).saturating_add(actual) / 4
        };
        if let Some(budget) = &mut self.daily_bytes {
            budget.remaining = budget.remaining.saturating_sub(actual);
        }
    }

    pub(crate) fn estimated_bytes(&self) -> u64 {
        self.est_bytes
    }

    fn prune(&mut self, now: Instant) {
        for budget in [
            &mut self.rate,
            &mut self.burst,
            &mut self.daily_downloads,
            &mut self.daily_bytes,
        ] {
            if let Some(b) = budget.as_ref()
                && b.reset_at <= now
            {
                *budget = None;
            }
        }
        if let Some(quota) = &mut self.static_quota {
            quota.prune(now);
        }
    }
}

/// Read one budget header family. The remaining header is required; the reset
/// falls back to the previous live reset, then to [`HEADER_RESET_DEFAULT`].
fn read_budget(
    headers: &HeaderMap,
    remaining_name: &str,
    reset_name: &str,
    previous: Option<Budget>,
    now: Instant,
    now_unix: u64,
) -> Option<Budget> {
    let remaining = header_u64(headers, remaining_name)?;
    let reset_at = header_reset(headers, reset_name, now, now_unix)
        .or_else(|| previous.map(|b| b.reset_at).filter(|reset| *reset > now))
        .unwrap_or(now + HEADER_RESET_DEFAULT);
    Some(Budget {
        remaining,
        reset_at,
    })
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    if let Ok(value) = value.parse::<u64>() {
        return Some(value);
    }
    let value = value.parse::<f64>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value as u64)
}

/// Parse a reset header. Small values are seconds from now; values at or above
/// 10^9 are Unix seconds, and at or above 10^12 are Unix milliseconds. The
/// result is clamped to `[now, now + HEADER_RESET_MAX]` so a bogus value cannot
/// block a mirror for longer than a week.
fn header_reset(headers: &HeaderMap, name: &str, now: Instant, now_unix: u64) -> Option<Instant> {
    let value = headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let seconds = if value >= 1_000_000_000_000.0 {
        value / 1000.0 - now_unix as f64
    } else if value >= 1_000_000_000.0 {
        value - now_unix as f64
    } else {
        value
    };
    let seconds = seconds.clamp(0.0, HEADER_RESET_MAX.as_secs_f64());
    Some(now + Duration::from_secs_f64(seconds))
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
#[path = "../../tests/limits.rs"]
mod tests;
