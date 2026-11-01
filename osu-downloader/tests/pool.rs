use super::{
    Acquire, INTERVAL_DECAY, MirrorPool, RETRY_AFTER_MAX, RETRY_AFTER_MIN, SchedulerHandle,
    WaitKind,
};
use crate::{Mirror, MirrorKind};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::time::{Duration, Instant};

fn granted(a: Acquire) -> usize {
    match a {
        Acquire::Granted { idx, .. } => idx,
        Acquire::Wait { .. } => panic!("expected a grant, got wait"),
    }
}

fn waited(a: Acquire) -> (Instant, WaitKind) {
    match a {
        Acquire::Wait { until, kind } => (until, kind),
        Acquire::Granted { idx, .. } => panic!("expected a wait, got grant of {idx}"),
    }
}

fn headers(entries: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in entries {
        map.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    map
}

/// Arbitrary fixed Unix time; only used by tests whose reset values are
/// seconds-from-now, so the absolute value does not matter.
const TEST_UNIX: u64 = 1_700_000_000;

fn base_cooldown() -> Duration {
    MirrorKind::Nerinyan.rate_limit_backoff()
}

#[test]
fn first_acquire_grants_first_admissible_in_order() {
    let pool = MirrorPool::new(vec![
        Mirror::nerinyan(),
        Mirror::osu_direct(),
        Mirror::sayobot(),
    ]);
    let now = Instant::now();
    assert_eq!(granted(pool.acquire_at(&[0, 1, 2], now)), 0);
}

#[test]
fn grant_consumes_token_for_the_interval_window() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let base = MirrorKind::Nerinyan.base_request_interval();
    let now = Instant::now();
    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
    let (until, kind) = waited(pool.acquire_at(&[0], now));
    assert_eq!(kind, WaitKind::Spacing, "a spent token is a spacing wait");
    assert_eq!(until, now + base);
}

#[test]
fn concurrent_acquire_at_one_instant_grants_one_token() {
    // Simulated wake-together crowd: only a single grant is handed out for the
    // slot per interval window; the rest are told to wait.
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    let grants = (0..16)
        .filter(|_| matches!(pool.acquire_at(&[0], now), Acquire::Granted { .. }))
        .count();
    assert_eq!(grants, 1);
}

#[test]
fn acquire_spills_to_next_slot_when_first_token_is_spent() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan(), Mirror::osu_direct()]);
    let now = Instant::now();
    assert_eq!(granted(pool.acquire_at(&[0, 1], now)), 0);
    assert_eq!(granted(pool.acquire_at(&[0, 1], now)), 1);
    // Both tokens spent at this instant.
    let (_until, kind) = waited(pool.acquire_at(&[0, 1], now));
    assert_eq!(kind, WaitKind::Spacing);
}

#[test]
fn cooldown_blocks_admission_and_reports_cooling() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);
    let (until, kind) = waited(pool.acquire_at(&[0], now));
    assert_eq!(kind, WaitKind::Cooldown);
    assert_eq!(until, now + base_cooldown());
}

#[test]
fn cooldown_expires_then_grants() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);
    let after = now + base_cooldown() + Duration::from_millis(1);
    assert_eq!(granted(pool.acquire_at(&[0], after)), 0);
}

#[test]
fn remark_during_live_cooldown_is_a_noop() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);
    // A second 429 mid-cooldown must not extend the original deadline.
    pool.mark_rate_limited_at(0, None, now + base_cooldown() / 2);
    let (until, _) = waited(pool.acquire_at(&[0], now + base_cooldown() / 2));
    assert_eq!(until, now + base_cooldown());
}

#[test]
fn escalation_within_window_doubles_the_cooldown() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let t0 = Instant::now();
    pool.mark_rate_limited_at(0, None, t0);
    // Cooldown has expired but the next 429 lands inside the escalation window.
    let t1 = t0 + base_cooldown() + Duration::from_secs(1);
    pool.mark_rate_limited_at(0, None, t1);
    let (until, _) = waited(pool.acquire_at(&[0], t1));
    assert_eq!(until, t1 + base_cooldown() * 2);
}

#[test]
fn escalation_resets_outside_window() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let t0 = Instant::now();
    pool.mark_rate_limited_at(0, None, t0);
    // Well past the escalation window: back to the per-kind base.
    let t1 = t0 + base_cooldown() + Duration::from_secs(31);
    pool.mark_rate_limited_at(0, None, t1);
    let (until, _) = waited(pool.acquire_at(&[0], t1));
    assert_eq!(until, t1 + base_cooldown());
}

#[test]
fn retry_after_is_clamped_to_the_ceiling() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, Some(Duration::from_secs(3600)), now);
    let (until, _) = waited(pool.acquire_at(&[0], now));
    assert_eq!(until, now + RETRY_AFTER_MAX);
}

#[test]
fn retry_after_is_clamped_to_the_floor() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, Some(Duration::ZERO), now);
    let (until, _) = waited(pool.acquire_at(&[0], now));
    assert_eq!(until, now + RETRY_AFTER_MIN);
}

#[test]
fn retry_after_floors_but_does_not_cap_escalation() {
    // A repeat offender answering 429 + a tiny Retry-After must still escalate:
    // the server hint is a floor, so the escalated cooldown wins over it.
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let t0 = Instant::now();
    pool.mark_rate_limited_at(0, None, t0);
    let t1 = t0 + base_cooldown() + Duration::from_secs(1);
    pool.mark_rate_limited_at(0, Some(Duration::from_millis(1)), t1);
    let (until, _) = waited(pool.acquire_at(&[0], t1));
    assert_eq!(
        until,
        t1 + base_cooldown() * 2,
        "escalation must win over a short Retry-After inside the window"
    );
}

#[test]
fn cooling_is_reported_when_cooldown_is_out_sat_by_spacing() {
    // A live cooldown whose tail is shorter than a grown send-token spacing must
    // still report a cooling wait, so the countdown / auto-defer budget / skip
    // path stays reachable for that slot.
    let base = Duration::from_millis(1500);
    let pool = MirrorPool::new_with_base_interval(
        vec![Mirror::custom("https://a.example/d/{id}").unwrap()],
        base,
    );
    let now = Instant::now();
    // The grant pushes next_free out to now + 1.5 s (the spacing token).
    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
    // A 0.5 s cooldown expires before that spacing token frees.
    pool.mark_rate_limited_at(0, Some(Duration::from_millis(500)), now);
    let (until, kind) = waited(pool.acquire_at(&[0], now));
    assert_eq!(
        kind,
        WaitKind::Cooldown,
        "a live cooldown makes the wait cooling even when spacing is the later bound"
    );
    assert_eq!(
        until,
        now + base,
        "the later of the cooldown tail and the spacing token binds"
    );
}

#[test]
fn interval_doubles_on_429_and_decays_on_success() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let base = MirrorKind::Nerinyan.base_request_interval();
    let t0 = Instant::now();
    assert_eq!(pool.interval(0), base);
    pool.mark_rate_limited_at(0, None, t0);
    assert_eq!(pool.interval(0), base * 2);
    // A second 429 after the cooldown expired doubles again.
    pool.mark_rate_limited_at(0, None, t0 + base_cooldown() + Duration::from_millis(1));
    assert_eq!(pool.interval(0), base * 4);
    pool.on_success(0);
    // Decay never dips below the per-kind base. With the shrunken test base
    // (1 ms) this exercises only the underflow-to-floor branch; the stepwise
    // AIMD decay is pinned separately below with a larger base.
    assert_eq!(pool.interval(0), base);
}

#[test]
fn success_decays_interval_by_exactly_one_step() {
    // Base chosen so base * 4 - INTERVAL_DECAY > base: one success must shave
    // exactly INTERVAL_DECAY off the interval, not hard-reset it to the base.
    let base = Duration::from_millis(40);
    let pool = MirrorPool::new_with_base_interval(vec![Mirror::nerinyan()], base);
    let t0 = Instant::now();
    pool.mark_rate_limited_at(0, None, t0);
    pool.mark_rate_limited_at(0, None, t0 + base_cooldown() + Duration::from_millis(1));
    assert_eq!(pool.interval(0), base * 4);
    pool.on_success(0);
    assert_eq!(pool.interval(0), base * 4 - INTERVAL_DECAY);
    // Repeated successes keep stepping down and only floor-clamp at the base.
    for _ in 0..16 {
        pool.on_success(0);
    }
    assert_eq!(pool.interval(0), base);
}

#[test]
fn cooldowns_are_independent_across_custom_mirrors() {
    // Two custom mirrors share `MirrorKind::Custom`; per-slot keying keeps their
    // cooldowns apart so a 429 on one does not sideline the other.
    let pool = MirrorPool::new(vec![
        Mirror::custom("https://a.example/d/{id}").unwrap(),
        Mirror::custom("https://b.example/d/{id}").unwrap(),
    ]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);
    assert_eq!(granted(pool.acquire_at(&[0, 1], now)), 1);
}

#[test]
fn out_of_range_index_is_ignored() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(5, None, now);
    // Candidate 5 does not exist and is skipped; slot 0 is still granted.
    assert_eq!(granted(pool.acquire_at(&[5, 0], now)), 0);
}

#[test]
fn mirrors_preserves_order_and_duplicates() {
    let pool = MirrorPool::new(vec![
        Mirror::nerinyan(),
        Mirror::osu_direct(),
        Mirror::nerinyan(),
    ]);
    let kinds: Vec<MirrorKind> = pool.mirrors().iter().map(Mirror::kind).collect();
    assert_eq!(
        kinds,
        vec![
            MirrorKind::Nerinyan,
            MirrorKind::OsuDirect,
            MirrorKind::Nerinyan,
        ]
    );
}

#[test]
fn catboy_units_gate_the_next_download_before_a_429() {
    // catboy charges 20 quota units per download. A response reporting 60
    // units therefore admits three downloads, not sixty, and the fourth waits
    // for the reset instead of provoking a 429.
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[("x-ratelimit-remaining", "60"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
    assert_eq!(
        granted(pool.acquire_at(&[0], now + Duration::from_millis(2))),
        0
    );
    assert_eq!(
        granted(pool.acquire_at(&[0], now + Duration::from_millis(4))),
        0
    );

    let (until, kind) = waited(pool.acquire_at(&[0], now + Duration::from_millis(6)));
    assert_eq!(kind, WaitKind::Budget);
    assert_eq!(until, now + Duration::from_secs(30));

    // The stale zero is discarded once the reset passes; the next request
    // probes the server again.
    assert_eq!(
        granted(pool.acquire_at(
            &[0],
            now + Duration::from_secs(30) + Duration::from_millis(1)
        )),
        0
    );
}

#[test]
fn catboy_stops_with_a_two_unit_remainder() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[("x-ratelimit-remaining", "19"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    let (until, kind) = waited(pool.acquire_at(&[0], now));
    assert_eq!(kind, WaitKind::Budget);
    assert_eq!(until, now + Duration::from_secs(30));
}

#[test]
fn burst_budget_is_a_separate_constraint() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[
            ("x-ratelimit-remaining", "1000"),
            ("x-ratelimit-reset", "60"),
            ("x-ratelimit-burst-remaining", "1"),
            ("x-ratelimit-burst-reset", "5"),
        ]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
    let (until, kind) = waited(pool.acquire_at(&[0], now + Duration::from_millis(2)));
    assert_eq!(kind, WaitKind::Budget);
    assert_eq!(until, now + Duration::from_secs(5));
}

#[test]
fn osu_direct_daily_budgets_block_before_the_last_block_of_bytes() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::OsuDirect,
        "http://osu.direct.test/d/{id}",
    )]);
    let now = Instant::now();
    // Seed the per-set estimate before the byte budget is known, then report
    // 80 MiB left. One 12 MiB set plus the 64 MiB reserve still fits; the next
    // one does not once the reservation is in flight.
    pool.finish_attempt(0, 0, Some(12 * 1024 * 1024));
    pool.observe_headers_at(
        0,
        &headers(&[
            ("x-download-budget-remaining", "10"),
            ("x-download-budget-reset", "3600"),
            ("x-download-budget-bytes-remaining", "83886080"),
            ("x-download-budget-bytes-reset", "3600"),
        ]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
    let (until, kind) = waited(pool.acquire_at(&[0], now + Duration::from_millis(2)));
    assert_eq!(kind, WaitKind::Budget);
    assert_eq!(until, now + Duration::from_secs(3600));

    // Releasing the reservation (the attempt produced no archive) lets the
    // next request through again.
    pool.finish_attempt(0, 12 * 1024 * 1024, None);
    assert_eq!(
        granted(pool.acquire_at(&[0], now + Duration::from_millis(4))),
        0
    );
}

#[test]
fn static_osu_api_quota_blocks_the_eleventh_hourly_download() {
    let pool = MirrorPool::new(vec![Mirror::osu_api()]);
    let now = Instant::now();
    for i in 0..10 {
        assert_eq!(
            granted(pool.acquire_at(&[0], now + Duration::from_millis(i * 2))),
            0
        );
    }

    let (until, kind) = waited(pool.acquire_at(&[0], now + Duration::from_millis(20)));
    assert_eq!(kind, WaitKind::Budget);
    assert_eq!(until, now + Duration::from_secs(3600));

    assert_eq!(
        granted(pool.acquire_at(
            &[0],
            now + Duration::from_secs(3600) + Duration::from_millis(1)
        )),
        0
    );
}

#[test]
fn a_reset_already_in_the_past_does_not_block() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "0")]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
}

#[test]
fn malformed_budget_headers_are_ignored() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[
            ("x-ratelimit-remaining", "lots"),
            ("x-ratelimit-reset", "soon"),
        ]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[0], now)), 0);
}

#[test]
fn header_budgets_are_per_slot() {
    let pool = MirrorPool::new(vec![
        Mirror::with_kind_and_template(MirrorKind::Catboy, "http://a.test/d/{id}"),
        Mirror::with_kind_and_template(MirrorKind::Catboy, "http://b.test/d/{id}"),
    ]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[("x-ratelimit-remaining", "19"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    assert_eq!(granted(pool.acquire_at(&[1], now)), 1);
    let (_until, kind) = waited(pool.acquire_at(&[0], now));
    assert_eq!(kind, WaitKind::Budget);
}

#[test]
fn schedule_of_an_idle_pool_needs_no_wait() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan(), Mirror::osu_direct()]);
    let now = Instant::now();
    let schedule = pool.schedule_at(2, now);

    assert_eq!(schedule.outstanding, 2);
    assert_eq!(schedule.request_eta, Duration::ZERO);
    assert_eq!(schedule.mirrors.len(), 2);
    assert!(
        schedule
            .mirrors
            .iter()
            .all(|mirror| mirror.next_free_in == Duration::ZERO)
    );
    // The anonymous built-ins publish no budget headers.
    assert!(
        schedule
            .mirrors
            .iter()
            .all(|mirror| mirror.remaining_requests.is_none())
    );
}

#[test]
fn schedule_eta_counts_request_spacing_tokens() {
    let base = Duration::from_secs(1);
    let pool = MirrorPool::new_with_base_interval(vec![Mirror::nerinyan()], base);
    let now = Instant::now();

    // One send now, then one per interval: the third needs two full intervals.
    let schedule = pool.schedule_at(3, now);
    assert_eq!(schedule.request_eta, Duration::from_secs(2));
    assert_eq!(schedule.mirrors[0].next_free_in, Duration::ZERO);
}

#[test]
fn schedule_eta_includes_a_live_cooldown() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan()]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);

    let schedule = pool.schedule_at(1, now);
    assert_eq!(schedule.request_eta, base_cooldown());
    assert_eq!(schedule.mirrors[0].cooldown_in, Some(base_cooldown()));
}

#[test]
fn schedule_reports_budget_headroom_and_reset() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    // 60 rate units at 20 per download: three requests fit in this window.
    pool.observe_headers_at(
        0,
        &headers(&[("x-ratelimit-remaining", "60"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    let schedule = pool.schedule_at(4, now);
    assert_eq!(schedule.mirrors[0].remaining_requests, Some(3));
    assert_eq!(schedule.mirrors[0].reset_in, Some(Duration::from_secs(30)));
    assert_eq!(schedule.mirrors[0].budget_in, None);
    // Three fit before the reset, and the reset refills them: the fourth waits
    // for the reset instant.
    assert_eq!(schedule.request_eta, Duration::from_secs(30));
}

#[test]
fn schedule_caps_a_permanently_blocked_pool_at_seven_days() {
    let pool = MirrorPool::new(vec![Mirror::with_kind_and_template(
        MirrorKind::Catboy,
        "http://catboy.test/d/{id}",
    )]);
    let now = Instant::now();
    pool.observe_headers_at(
        0,
        &headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "604800"),
        ]),
        now,
        TEST_UNIX,
    );

    let schedule = pool.schedule_at(1, now);
    assert_eq!(schedule.request_eta, Duration::from_secs(7 * 24 * 60 * 60));
    assert_eq!(schedule.mirrors[0].remaining_requests, Some(0));
    assert_eq!(
        schedule.mirrors[0].reset_in,
        Some(Duration::from_secs(7 * 24 * 60 * 60))
    );
    assert_eq!(
        schedule.mirrors[0].budget_in,
        Some(Duration::from_secs(7 * 24 * 60 * 60))
    );
}

#[test]
fn a_shared_handle_carries_spacing_between_pools() {
    let scheduler = SchedulerHandle::new();
    let first = MirrorPool::with_scheduler(vec![Mirror::nerinyan()], scheduler.clone());
    let now = Instant::now();
    assert_eq!(granted(first.acquire_at(&[0], now)), 0);

    let second = MirrorPool::with_scheduler(vec![Mirror::nerinyan()], scheduler);
    // The grant in the first pool consumed the shared token...
    let (until, kind) = waited(second.acquire_at(&[0], now));
    assert_eq!(kind, WaitKind::Spacing);
    assert_eq!(until, now + MirrorKind::Nerinyan.base_request_interval());
    // ...so the second pool's forecast starts from the shared next-free time.
    assert_eq!(
        second.schedule_at(1, now).request_eta,
        MirrorKind::Nerinyan.base_request_interval()
    );
}

#[test]
fn custom_mirrors_on_one_host_keep_independent_slots() {
    // Same host, different paths: the template is part of the slot identity,
    // so a 429 on one endpoint must not sideline the other.
    let pool = MirrorPool::new(vec![
        Mirror::custom("http://127.0.0.1:9/bad/{id}").unwrap(),
        Mirror::custom("http://127.0.0.1:9/ok/{id}").unwrap(),
    ]);
    let now = Instant::now();
    pool.mark_rate_limited_at(0, None, now);
    assert_eq!(granted(pool.acquire_at(&[0, 1], now)), 1);
}

#[test]
fn a_duplicate_mirror_appears_once_in_the_schedule() {
    let pool = MirrorPool::new(vec![Mirror::nerinyan(), Mirror::nerinyan()]);
    let now = Instant::now();
    assert_eq!(pool.schedule_at(1, now).mirrors.len(), 1);
}
