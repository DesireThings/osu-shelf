use super::*;
use reqwest::header::{HeaderMap, HeaderName};

/// Arbitrary fixed Unix time; tests using seconds-from-now resets do not care
/// about the absolute value.
const TEST_UNIX: u64 = 1_700_000_000;

fn headers(entries: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in entries {
        map.insert(HeaderName::from_static(name), value.parse().unwrap());
    }
    map
}

#[test]
fn reset_seconds_from_now_is_relative() {
    let now = Instant::now();
    let reset = header_reset(
        &headers(&[("x-ratelimit-reset", "30")]),
        "x-ratelimit-reset",
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(reset, now + Duration::from_secs(30));
}

#[test]
fn reset_unix_seconds_is_absolute() {
    let now = Instant::now();
    let value = (TEST_UNIX + 45).to_string();
    let reset = header_reset(
        &headers(&[("x-ratelimit-reset", &value)]),
        "x-ratelimit-reset",
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(reset, now + Duration::from_secs(45));
}

#[test]
fn reset_unix_milliseconds_is_absolute() {
    let now = Instant::now();
    let value = ((TEST_UNIX + 45) * 1000).to_string();
    let reset = header_reset(
        &headers(&[("x-ratelimit-reset", &value)]),
        "x-ratelimit-reset",
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(reset, now + Duration::from_secs(45));
}

#[test]
fn reset_in_the_past_clamps_to_now() {
    let now = Instant::now();
    let value = (TEST_UNIX - 10).to_string();
    let reset = header_reset(
        &headers(&[("x-ratelimit-reset", &value)]),
        "x-ratelimit-reset",
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(reset, now);
}

#[test]
fn reset_is_clamped_to_the_week_cap() {
    let now = Instant::now();
    let reset = header_reset(
        &headers(&[("x-ratelimit-reset", "999999999")]),
        "x-ratelimit-reset",
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(reset, now + HEADER_RESET_MAX);
}

#[test]
fn malformed_reset_is_ignored() {
    let now = Instant::now();
    assert_eq!(
        header_reset(
            &headers(&[("x-ratelimit-reset", "soon")]),
            "x-ratelimit-reset",
            now,
            TEST_UNIX,
        ),
        None
    );
}

#[test]
fn header_number_accepts_integers_and_floats() {
    let map = headers(&[("x-test", "42")]);
    assert_eq!(header_u64(&map, "x-test"), Some(42));
    let map = headers(&[("x-test", "42.9")]);
    assert_eq!(header_u64(&map, "x-test"), Some(42));
    let map = headers(&[("x-test", "-1")]);
    assert_eq!(header_u64(&map, "x-test"), None);
    let map = headers(&[("x-test", "lots")]);
    assert_eq!(header_u64(&map, "x-test"), None);
}

#[test]
fn missing_remaining_leaves_the_previous_budget_alone() {
    let now = Instant::now();
    let previous = Budget {
        remaining: 0,
        reset_at: now + Duration::from_secs(10),
    };
    assert!(
        read_budget(
            &headers(&[("x-ratelimit-reset", "5")]),
            "x-ratelimit-remaining",
            "x-ratelimit-reset",
            Some(previous),
            now,
            TEST_UNIX,
        )
        .is_none()
    );
}

#[test]
fn remaining_without_reset_keeps_the_previous_live_reset() {
    let now = Instant::now();
    let previous = Budget {
        remaining: 0,
        reset_at: now + Duration::from_secs(10),
    };
    let budget = read_budget(
        &headers(&[("x-ratelimit-remaining", "5")]),
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        Some(previous),
        now,
        TEST_UNIX,
    )
    .unwrap();
    assert_eq!(budget.remaining, 5);
    assert_eq!(budget.reset_at, now + Duration::from_secs(10));
}

#[test]
fn a_missing_header_family_keeps_the_previous_budget() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::simple());
    state.observe_headers_at(
        &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );
    state.observe_headers_at(
        &headers(&[("x-download-budget-remaining", "5")]),
        now,
        TEST_UNIX,
    );

    assert_eq!(
        state.blocked_until(now, 0),
        Some(now + Duration::from_secs(30))
    );
}

#[test]
fn finish_attempt_updates_the_size_estimate() {
    let mut state = LimitState::new(LimitPolicy::osu_direct());
    state.finish_attempt(0, Some(100));
    assert_eq!(state.estimated_bytes(), 100);
    state.finish_attempt(0, Some(300));
    assert_eq!(state.estimated_bytes(), 150);
}

#[test]
fn reserve_bytes_only_tracks_byte_budgets() {
    let mut simple = LimitState::new(LimitPolicy::simple());
    assert_eq!(simple.reserve_bytes(100), 0);

    let mut direct = LimitState::new(LimitPolicy::osu_direct());
    let now = Instant::now();
    direct.observe_headers_at(
        &headers(&[
            ("x-download-budget-bytes-remaining", "1000"),
            ("x-download-budget-bytes-reset", "3600"),
        ]),
        now,
        TEST_UNIX,
    );
    assert_eq!(direct.reserve_bytes(100), 100);
}

#[test]
fn schedule_view_reports_the_tightest_request_budget() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::catboy());
    state.observe_headers_at(
        &headers(&[("x-ratelimit-remaining", "60"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    let view = state.schedule_view(now, 0);
    assert_eq!(view.remaining_requests, Some(3));
    assert_eq!(view.reset_in, Some(Duration::from_secs(30)));
    assert_eq!(view.budget_in, None);
}

#[test]
fn schedule_view_reports_a_blocking_byte_budget() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::osu_direct());
    state.finish_attempt(0, Some(12 * 1024 * 1024));
    state.observe_headers_at(
        &headers(&[
            ("x-download-budget-bytes-remaining", "1"),
            ("x-download-budget-bytes-reset", "3600"),
        ]),
        now,
        TEST_UNIX,
    );

    let view = state.schedule_view(now, state.estimated_bytes());
    assert_eq!(view.budget_in, Some(Duration::from_secs(3600)));
    // A byte budget is not a request-count budget.
    assert_eq!(view.remaining_requests, None);
}

#[test]
fn tokens_by_counts_budget_resets_inside_the_horizon() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::catboy());
    state.observe_headers_at(
        &headers(&[("x-ratelimit-remaining", "60"), ("x-ratelimit-reset", "30")]),
        now,
        TEST_UNIX,
    );

    assert_eq!(
        state.tokens_by(now, now + Duration::from_secs(29), 0),
        3,
        "three downloads fit the live window"
    );
    assert_eq!(
        state.tokens_by(now, now + Duration::from_secs(30), 0),
        6,
        "the reset at the horizon refills the window once"
    );
    assert_eq!(
        state.tokens_by(now, now + Duration::from_secs(60), 0),
        9,
        "a later horizon adds one more window"
    );
}

#[test]
fn tokens_by_byte_budget_counts_resets() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::osu_direct());
    state.finish_attempt(0, Some(12 * 1024 * 1024));
    state.observe_headers_at(
        &headers(&[
            ("x-download-budget-bytes-remaining", "83886080"),
            ("x-download-budget-bytes-reset", "3600"),
        ]),
        now,
        TEST_UNIX,
    );

    let est = state.estimated_bytes();
    // 80 MiB available, 12 MiB estimate + 64 MiB reserve per request.
    assert_eq!(
        state.tokens_by(now, now + Duration::from_secs(3599), est),
        1
    );
    assert_eq!(
        state.tokens_by(now, now + Duration::from_secs(3600), est),
        2
    );
}

#[test]
fn tokens_by_static_quota_expands_by_full_windows() {
    let now = Instant::now();
    let mut state = LimitState::new(LimitPolicy::osu_api());
    for i in 0..10 {
        state.note_request(now + Duration::from_millis(i));
    }

    let start = now + Duration::from_secs(1);
    assert_eq!(
        state.tokens_by(start, start + Duration::from_secs(3599), 0),
        0,
        "the hour is spent"
    );
    assert_eq!(
        state.tokens_by(start, start + Duration::from_secs(3600), 0),
        10,
        "a full window refills the quota"
    );
}

#[test]
fn only_mirrors_with_known_limits_have_a_summary() {
    use crate::MirrorKind;

    for kind in MirrorKind::BUILTINS {
        let known = matches!(
            kind,
            MirrorKind::Catboy | MirrorKind::OsuDirect | MirrorKind::OsuApi
        );
        assert_eq!(kind.limit_summary().is_some(), known, "{kind:?}");
    }
    assert_eq!(MirrorKind::Custom.limit_summary(), None);
}

#[test]
fn the_catboy_summary_matches_its_download_cost() {
    let downloads_per_minute = 1200 / CATBOY_RATE_COST;
    assert_eq!(
        LimitPolicy::catboy().summary,
        Some(format!("{downloads_per_minute} downloads/min").as_str())
    );
}

#[test]
fn the_osu_summary_matches_its_static_quota() {
    assert_eq!(
        LimitPolicy::osu_api().summary,
        Some(format!("{OSU_API_DOWNLOAD_LIMIT} downloads/hour").as_str())
    );
}
