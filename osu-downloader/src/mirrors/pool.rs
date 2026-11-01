use super::{
    Mirror,
    limits::{LimitState, unix_now},
};
use crate::event::{MirrorSchedule, Schedule};
use reqwest::header::HeaderMap;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Ceiling on a slot's adaptive request spacing. A run of 429s doubles the
/// interval up to this cap; it never grows past it.
const INTERVAL_CEILING: Duration = Duration::from_secs(5);

/// Amount a slot's spacing shrinks per successful attempt, decaying back toward
/// the per-kind [`base_request_interval`](super::MirrorKind::base_request_interval).
const INTERVAL_DECAY: Duration = Duration::from_millis(25);

/// A 429 landing within this window of the previous cooldown's expiry escalates
/// (doubles) the cooldown; a 429 arriving later resets it to the per-kind base.
const ESCALATION_WINDOW: Duration = Duration::from_secs(30);

/// Hard cap on an escalated cooldown.
const COOLDOWN_CAP: Duration = Duration::from_secs(10 * 60);

/// Lower clamp for a server-supplied `Retry-After`. A hostile or buggy value
/// below this is floored so the pool never busy-retries.
#[cfg(not(test))]
const RETRY_AFTER_MIN: Duration = Duration::from_secs(1);
#[cfg(test)]
const RETRY_AFTER_MIN: Duration = Duration::from_millis(1);

/// Upper clamp for a server-supplied `Retry-After`.
const RETRY_AFTER_MAX: Duration = Duration::from_secs(15 * 60);

/// Longest schedule forecast horizon. The binary search never looks further
/// ahead than this, so a permanently blocked pool reports a seven-day ETA
/// instead of searching forever.
const FORECAST_HORIZON: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Identity of a scheduler slot: a mirror's kind and display host, plus the
/// full URL template for custom mirrors.
///
/// Keying by identity rather than by position in one run's mirror list lets a
/// shared [`SchedulerHandle`] carry a mirror's spacing and budget state into
/// the next run, and lets concurrently running pools pace each other. Built-ins
/// have one host per kind; custom mirrors share a host when they point at
/// different paths on the same server, so their template keeps them apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MirrorKey {
    kind: super::MirrorKind,
    host: Box<str>,
    template: Option<Box<str>>,
}

impl MirrorKey {
    fn of(mirror: &Mirror) -> Self {
        Self {
            kind: mirror.kind(),
            host: mirror.host().into(),
            template: (mirror.kind() == super::MirrorKind::Custom)
                .then(|| mirror.template().into()),
        }
    }
}

/// Handle to scheduler state shared across download runs.
///
/// A run's mirror pool normally starts with fresh
/// spacing and budget state. Passing the same handle to every run — via
/// [`DownloaderBuilder::scheduler`](crate::DownloaderBuilder::scheduler) —
/// keeps that state warm between runs and across concurrently running
/// sessions, because slots are keyed by mirror identity rather than by the
/// position in any one run's mirror list.
///
/// Cloning is cheap; clones share the same state.
#[derive(Debug, Clone, Default)]
pub struct SchedulerHandle {
    slots: Arc<Mutex<HashMap<MirrorKey, SlotState>>>,
}

impl SchedulerHandle {
    /// Create a handle with no mirror state yet.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Why a candidate slot is not admissible yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitKind {
    /// The slot's send token is spent: a healthy map waiting its turn.
    Spacing,
    /// A 429 penalty is still live.
    Cooldown,
    /// A request or byte budget has no room until its reset.
    Budget,
}

impl WaitKind {
    /// Whether the wait is rate-limit sourced (a cooldown or a budget) rather
    /// than plain request spacing. Drives the caller's deferral accounting and
    /// the UI's rate-limit status.
    pub(crate) fn is_limited(self) -> bool {
        !matches!(self, Self::Spacing)
    }
}

/// Outcome of a [`MirrorPool::acquire`] call.
pub(crate) enum Acquire {
    /// Slot index granted a send token; the token has been consumed.
    Granted {
        /// Slot that may send.
        idx: usize,
        /// Bytes reserved against the slot's byte budget, to be handed back to
        /// [`MirrorPool::finish_attempt`].
        reserved_bytes: u64,
    },
    /// No candidate is admissible yet.
    Wait {
        /// Earliest instant any candidate becomes admissible.
        until: Instant,
        /// Why the earliest candidate is waiting.
        kind: WaitKind,
    },
}

/// Per-slot scheduler state. Keyed by mirror identity (kind + host), never by
/// position in one pool's mirror list: two custom mirrors share the `Custom`
/// kind but must be paced and penalized independently, while the same mirror
/// appearing in a later run shares the state seeded by an earlier one.
#[derive(Debug)]
struct SlotState {
    /// AIMD floor this slot's `interval` decays back toward.
    base_interval: Duration,
    /// Live request spacing: doubled on a 429, decayed on success.
    interval: Duration,
    /// Send token: the slot may be granted again once `now >= next_free`.
    next_free: Instant,
    /// Active rate-limit cooldown end, if the slot is penalized.
    cooldown_until: Option<Instant>,
    /// Duration of the most recent cooldown; the escalation multiplier's input.
    last_cooldown: Duration,
    /// Expiry anchor of the most recent cooldown; a fresh 429 within
    /// [`ESCALATION_WINDOW`] of this instant escalates instead of resetting.
    cooldown_expired_at: Option<Instant>,
    /// Header-driven and fixed request/byte budgets for this slot.
    limits: LimitState,
}

impl SlotState {
    fn new(base_interval: Duration, limits: LimitState, now: Instant) -> Self {
        Self {
            base_interval,
            interval: base_interval,
            next_free: now,
            cooldown_until: None,
            last_cooldown: Duration::ZERO,
            cooldown_expired_at: None,
            limits,
        }
    }

    /// Per-mirror forecast fields at `now`.
    fn schedule(&mut self, mirror: crate::MirrorRef, now: Instant) -> MirrorSchedule {
        let est_bytes = self.limits.estimated_bytes();
        let view = self.limits.schedule_view(now, est_bytes);
        MirrorSchedule {
            mirror,
            next_free_in: self.next_free.saturating_duration_since(now),
            cooldown_in: self
                .cooldown_until
                .filter(|&until| until > now)
                .map(|until| until - now),
            budget_in: view.budget_in,
            remaining_requests: view.remaining_requests,
            reset_in: view.reset_in,
        }
    }

    /// How many send tokens this slot can hand out by `until`, capped by its
    /// live request and byte budgets.
    fn capacity_at(&mut self, now: Instant, until: Instant) -> u64 {
        let cooldown = self.cooldown_until.filter(|&c| c > now);
        let first = self.next_free.max(cooldown.unwrap_or(now)).max(now);
        let spacing = spacing_tokens(first, self.interval, until);
        let est_bytes = self.limits.estimated_bytes();
        spacing.min(self.limits.tokens_by(now, until, est_bytes))
    }
}

/// Admission-control scheduler over the configured mirror list.
///
/// Workers do not free-fire: they [`acquire`](MirrorPool::acquire) a send token
/// for one of their candidate slots. Granting consumes the token
/// (`next_free = now + interval`), so concurrent acquirers self-space and spill
/// to the next mirror in the caller's configured order instead of bursting the
/// same slot. A 429 [`marks`](MirrorPool::mark_rate_limited) the slot with an
/// (escalating) cooldown and widens its spacing; a success narrows it. A slot's
/// request/byte budget, fed by response headers, gates it before a 429 happens.
pub(crate) struct MirrorPool {
    mirrors: Vec<Mirror>,
    /// Mirror identity for each position in `mirrors`, parallel to it.
    keys: Vec<MirrorKey>,
    slots: Arc<Mutex<HashMap<MirrorKey, SlotState>>>,
}

impl MirrorPool {
    pub(crate) fn new(mirrors: Vec<Mirror>) -> Self {
        Self::build(mirrors, Arc::new(Mutex::new(HashMap::new())), None)
    }

    /// Build a pool whose slot state lives in `scheduler`, so it is shared with
    /// every other pool built from the same handle. Slots already present keep
    /// their spacing, cooldowns and budgets; missing ones are seeded fresh.
    pub(crate) fn with_scheduler(mirrors: Vec<Mirror>, scheduler: SchedulerHandle) -> Self {
        Self::build(mirrors, scheduler.slots, None)
    }

    fn build(
        mirrors: Vec<Mirror>,
        slots: Arc<Mutex<HashMap<MirrorKey, SlotState>>>,
        base_override: Option<Duration>,
    ) -> Self {
        let now = Instant::now();
        let keys: Vec<MirrorKey> = mirrors.iter().map(MirrorKey::of).collect();
        {
            let mut map = slots.lock().unwrap();
            for (mirror, key) in mirrors.iter().zip(&keys) {
                map.entry(key.clone()).or_insert_with(|| {
                    SlotState::new(
                        base_override.unwrap_or_else(|| mirror.kind().base_request_interval()),
                        LimitState::new(mirror.kind().limit_policy()),
                        now,
                    )
                });
            }
        }
        Self {
            mirrors,
            keys,
            slots,
        }
    }

    /// Test-only constructor with an explicit per-slot base interval, so decay
    /// arithmetic can be asserted with a base larger than the shrunken test
    /// constants (where `base * n - INTERVAL_DECAY` would underflow to the
    /// floor and mask a hard reset).
    #[cfg(test)]
    pub(crate) fn new_with_base_interval(mirrors: Vec<Mirror>, base: Duration) -> Self {
        Self::build(mirrors, Arc::new(Mutex::new(HashMap::new())), Some(base))
    }

    /// Grant a send token for the first admissible slot in `candidates`
    /// (caller's configured order), or report when the earliest candidate frees.
    pub(crate) fn acquire(&self, candidates: &[usize]) -> Acquire {
        self.acquire_at(candidates, Instant::now())
    }

    /// [`acquire`](Self::acquire) with an injected clock for deterministic tests.
    pub(crate) fn acquire_at(&self, candidates: &[usize], now: Instant) -> Acquire {
        let mut slots = self.slots.lock().unwrap();
        let mut best: Option<(Instant, WaitKind)> = None;

        for &idx in candidates {
            let Some(key) = self.keys.get(idx) else {
                continue;
            };
            let Some(s) = slots.get_mut(key) else {
                continue;
            };
            let cooldown = s.cooldown_until.filter(|&c| c > now);
            let est_bytes = s.limits.estimated_bytes();
            let budget = s.limits.blocked_until(now, est_bytes);
            let free_at = s
                .next_free
                .max(cooldown.unwrap_or(now))
                .max(budget.unwrap_or(now));

            if free_at <= now {
                s.limits.note_request(now);
                let reserved_bytes = s.limits.reserve_bytes(est_bytes);
                s.next_free = now + s.interval;
                return Acquire::Granted {
                    idx,
                    reserved_bytes,
                };
            }

            // A live cooldown makes this a rate-limit wait regardless of
            // whether the cooldown tail or a grown spacing token is the later
            // bound; a cooldown out-sat by spacing must not lose its
            // countdown/budget/skip. The same applies to a live budget.
            let budget_binds = budget.is_some_and(|b| cooldown.is_none_or(|c| b > c));
            let kind = if budget_binds {
                WaitKind::Budget
            } else if cooldown.is_some() {
                WaitKind::Cooldown
            } else {
                WaitKind::Spacing
            };
            if best.is_none_or(|(t, _)| free_at < t) {
                best = Some((free_at, kind));
            }
        }

        match best {
            Some((until, kind)) => Acquire::Wait { until, kind },
            None => Acquire::Wait {
                until: now,
                kind: WaitKind::Spacing,
            },
        }
    }

    /// Fold one response's budget headers into the slot at `idx`. Called for
    /// every response, including 3xx redirects and 429s, so the headers on a
    /// mirror's own redirect response are not lost.
    pub(crate) fn observe_headers(&self, idx: usize, headers: &HeaderMap) {
        self.observe_headers_at(idx, headers, Instant::now(), unix_now());
    }

    /// [`observe_headers`](Self::observe_headers) with an injected clock.
    pub(crate) fn observe_headers_at(
        &self,
        idx: usize,
        headers: &HeaderMap,
        now: Instant,
        now_unix: u64,
    ) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(s) = self.keys.get(idx).and_then(|key| slots.get_mut(key)) {
            s.limits.observe_headers_at(headers, now, now_unix);
        }
    }

    /// Record an extra request against the slot's request budgets without
    /// taking a send token. Used for transient retries and size probes, which
    /// are real HTTP requests the mirror counts.
    pub(crate) fn note_request(&self, idx: usize) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(s) = self.keys.get(idx).and_then(|key| slots.get_mut(key)) {
            s.limits.note_request(Instant::now());
        }
    }

    /// Release the byte reservation taken by [`Acquire::Granted`] and fold a
    /// completed archive's size into the slot's running estimate.
    pub(crate) fn finish_attempt(
        &self,
        idx: usize,
        reserved_bytes: u64,
        actual_bytes: Option<u64>,
    ) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(s) = self.keys.get(idx).and_then(|key| slots.get_mut(key)) {
            s.limits.finish_attempt(reserved_bytes, actual_bytes);
        }
    }

    /// Mark the slot at `idx` rate-limited. `retry_after` is the parsed 429
    /// `Retry-After` (clamped to `[1 s, 15 min]`); absent, the cooldown is the
    /// per-kind base. A 429 landing within [`ESCALATION_WINDOW`] of the last
    /// cooldown's expiry escalates (doubles) it; when `retry_after` is present the
    /// escalated value is a floor, so a short server hint cannot undercut a repeat
    /// offender's penalty. Widens the slot's spacing too. A no-op while a cooldown
    /// is live, so one burst of concurrent 429s is one penalty.
    pub(crate) fn mark_rate_limited(&self, idx: usize, retry_after: Option<Duration>) {
        self.mark_rate_limited_at(idx, retry_after, Instant::now());
    }

    /// [`mark_rate_limited`](Self::mark_rate_limited) with an injected clock.
    pub(crate) fn mark_rate_limited_at(
        &self,
        idx: usize,
        retry_after: Option<Duration>,
        now: Instant,
    ) {
        let base = match self.mirrors.get(idx) {
            Some(mirror) => mirror.kind().rate_limit_backoff(),
            None => return,
        };
        let mut slots = self.slots.lock().unwrap();
        let Some(s) = self.keys.get(idx).and_then(|key| slots.get_mut(key)) else {
            return;
        };
        if s.cooldown_until.is_some_and(|c| c > now) {
            return;
        }

        // A 429 landing inside the escalation window doubles the previous
        // cooldown; outside it (or on the first hit) there is no escalation.
        let escalating = s
            .cooldown_expired_at
            .is_some_and(|exp| now >= exp && now.duration_since(exp) <= ESCALATION_WINDOW);
        let escalated = escalating.then(|| {
            s.last_cooldown
                .checked_mul(2)
                .unwrap_or(COOLDOWN_CAP)
                .min(COOLDOWN_CAP)
        });

        let cooldown = match retry_after {
            // A repeat offender's server hint is a floor, never a ceiling: honor
            // Retry-After but never let a short one undercut the escalated penalty.
            Some(ra) => {
                let clamped = ra.clamp(RETRY_AFTER_MIN, RETRY_AFTER_MAX);
                escalated.map_or(clamped, |esc| clamped.max(esc))
            }
            None => escalated.unwrap_or(base),
        };

        s.last_cooldown = cooldown;
        let until = now + cooldown;
        s.cooldown_until = Some(until);
        s.cooldown_expired_at = Some(until);
        s.interval = s
            .interval
            .checked_mul(2)
            .unwrap_or(INTERVAL_CEILING)
            .min(INTERVAL_CEILING);
    }

    /// Narrow the slot's spacing after a successful attempt, decaying it toward
    /// the per-kind base.
    pub(crate) fn on_success(&self, idx: usize) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(s) = self.keys.get(idx).and_then(|key| slots.get_mut(key)) {
            s.interval = s
                .interval
                .checked_sub(INTERVAL_DECAY)
                .unwrap_or(s.base_interval)
                .max(s.base_interval);
        }
    }

    /// Current request spacing for the slot at `idx` (base if out of range).
    pub(crate) fn interval(&self, idx: usize) -> Duration {
        let slots = self.slots.lock().unwrap();
        self.keys
            .get(idx)
            .and_then(|key| slots.get(key))
            .map(|s| s.interval)
            .unwrap_or(super::BASE_INTERVAL)
    }

    /// Forecast when `outstanding` requests can all have been sent, and the
    /// per-mirror state behind the answer.
    pub(crate) fn schedule(&self, outstanding: usize) -> Schedule {
        self.schedule_at(outstanding, Instant::now())
    }

    /// [`schedule`](Self::schedule) with an injected clock.
    pub(crate) fn schedule_at(&self, outstanding: usize, now: Instant) -> Schedule {
        let mut slots = self.slots.lock().unwrap();

        // A pool may list the same mirror twice (or two mirrors may resolve to
        // the same identity); one shared slot must count once.
        let mut seen = HashSet::new();
        let mut entries: Vec<(&Mirror, &MirrorKey)> = Vec::with_capacity(self.keys.len());
        for (mirror, key) in self.mirrors.iter().zip(&self.keys) {
            if seen.insert(key) {
                entries.push((mirror, key));
            }
        }

        let mut mirrors = Vec::with_capacity(entries.len());
        for (mirror, key) in &entries {
            let Some(slot) = slots.get_mut(*key) else {
                continue;
            };
            mirrors.push(slot.schedule(mirror.mirror_ref(), now));
        }

        let request_eta = {
            let mut capacity = |until: Instant| {
                let mut total = 0u64;
                for (_, key) in &entries {
                    if let Some(slot) = slots.get_mut(*key) {
                        total = total.saturating_add(slot.capacity_at(now, until));
                    }
                }
                total
            };
            forecast_eta(outstanding, now, &mut capacity)
        };

        Schedule {
            outstanding,
            request_eta,
            mirrors,
        }
    }

    pub(crate) fn mirrors(&self) -> &[Mirror] {
        &self.mirrors
    }
}

/// Send tokens one slot can hand out by `until`, given its first free instant
/// and live spacing interval.
fn spacing_tokens(first: Instant, interval: Duration, until: Instant) -> u64 {
    if until < first {
        return 0;
    }
    let step = interval.as_nanos().max(1);
    let windows = (until - first).as_nanos() / step;
    u64::try_from(windows).unwrap_or(u64::MAX).saturating_add(1)
}

/// Smallest delay (in whole milliseconds) at which the pool's capacity covers
/// `outstanding` requests; [`FORECAST_HORIZON`] when even that far ahead does
/// not fit. Capacity is monotonic in time, so a binary search is exact.
fn forecast_eta(
    outstanding: usize,
    now: Instant,
    capacity: &mut impl FnMut(Instant) -> u64,
) -> Duration {
    if outstanding == 0 {
        return Duration::ZERO;
    }
    let target = outstanding as u64;
    if capacity(now + FORECAST_HORIZON) < target {
        return FORECAST_HORIZON;
    }

    let mut low = 0u64;
    let mut high = FORECAST_HORIZON.as_millis() as u64;
    while low < high {
        let mid = low + (high - low) / 2;
        if capacity(now + Duration::from_millis(mid)) >= target {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    Duration::from_millis(low)
}

#[cfg(test)]
#[path = "../../tests/pool.rs"]
mod tests;
