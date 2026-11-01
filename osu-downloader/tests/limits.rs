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
