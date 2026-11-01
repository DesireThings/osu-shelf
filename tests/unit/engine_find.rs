#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{FindCriteria, FindPick, Route, build_find_request, check, nzbasic_rows, options};
use crate::app::find_source::{FindChoices, FindPlan, FindSource};
use crate::download::{DownloadConfig, IdsRunSource};
use crate::engine::{Downloads, download::RunSettings};
use crate::osu_db::OsuClient;
use osu_downloader::filter::{BeatmapDetails, FilterDirection, FilterSort};
use std::collections::HashMap;
use std::sync::Arc;

fn criteria() -> FindCriteria {
    FindCriteria::default()
}

fn run() -> RunSettings {
    RunSettings {
        config: DownloadConfig {
            directory: "/dl".into(),
            mirrors: Vec::new(),
            concurrent: 4,
            archive_validation: Default::default(),
            auto_skip_rate_limited: false,
            rate_limit_skip_secs: 60,
            scheduler: None,
        },
        skip_already_imported: true,
        osu_client: OsuClient::Lazer,
        osu_path: "/games/lazer".into(),
    }
}

fn position(labels: &[&str], label: &str) -> usize {
    labels.iter().position(|l| *l == label).unwrap()
}

#[test]
fn the_defaults_are_the_tuis_untouched_form_sorted_by_relevance() {
    let options = options();

    assert_eq!(options.defaults, criteria());
    assert_eq!(options.statuses[criteria().status], "has leaderboard");
    assert_eq!(options.sorts[criteria().sort], "relevance");
    let tui = FindSource::new().choices();
    let gui = criteria().to_source().unwrap().choices();
    assert_eq!(
        options.sorts[tui.sort], "ranked ↓",
        "the terminal app keeps its default"
    );
    assert_eq!(
        FindChoices {
            sort: tui.sort,
            ..gui
        },
        tui
    );
}

#[test]
fn relevance_never_clashes_with_a_nzbasic_filter() {
    let options = options();
    let special = FindCriteria {
        special: 1,
        ..criteria()
    };
    let checked = check(&special);
    assert_eq!(checked.route, Some(Route::Nzbasic));
    assert_eq!(checked.problem, None);
    let Ok(FindPlan::Nzbasic(query)) = special.to_source().unwrap().build_plan(None) else {
        panic!("a Special filter searches nzbasic");
    };
    // No search text to rank by there: newest ranked first.
    assert_eq!(
        query.sort,
        Some((FilterSort::ApprovedDate, FilterDirection::Desc))
    );

    let with_text = FindCriteria {
        query: "freedom dive".into(),
        ..criteria()
    };
    assert_eq!(check(&with_text).route, Some(Route::Osu));
    assert_eq!(options.sorts[with_text.sort], "relevance");
}

#[test]
fn a_preset_without_its_own_sort_starts_on_relevance() {
    let options = options();
    for (i, label) in options.presets.iter().enumerate() {
        let preset = FindCriteria::with_preset(i).unwrap();
        let checked = check(&preset);
        assert_eq!(checked.problem, None, "preset {label}");
        assert!(checked.route.is_some(), "preset {label}");
    }
    let farm = FindCriteria::with_preset(position(&options.presets, "farm")).unwrap();
    assert_eq!(options.sorts[farm.sort], "relevance");
    assert_eq!(check(&farm).route, Some(Route::Nzbasic));
}

#[test]
fn criteria_survive_the_round_trip_through_the_form() {
    let options = options();
    let mut c = criteria();
    c.mode = position(&options.modes, "osu!mania");
    c.genre = position(&options.genres, "anime");
    c.extra = vec![1];
    c.keys = "4..7".into();
    c.title = "Freedom Dive".into();

    let back = FindCriteria::from_source(&c.to_source().unwrap());

    assert_eq!(back, c);
}

#[test]
fn an_untouched_form_searches_osu() {
    let result = check(&criteria());

    assert_eq!(result.route, Some(Route::Osu));
    assert_eq!(result.problem, None);
}

#[test]
fn a_special_flag_sends_the_search_to_nzbasic() {
    let options = options();
    let mut c = criteria();
    c.special = position(&options.specials, "farm");

    assert_eq!(check(&c).route, Some(Route::Nzbasic));
}

#[test]
fn criteria_that_need_both_services_name_the_clash() {
    let options = options();
    let mut c = criteria();
    c.special = position(&options.specials, "stream");
    c.query = "camellia".into();

    let result = check(&c);

    assert_eq!(result.route, None);
    let problem = result.problem.unwrap();
    assert!(
        problem.starts_with("Stream only works with nzbasic"),
        "{problem}"
    );
    assert!(
        problem.contains("free text only with the osu! API"),
        "{problem}"
    );
}

#[test]
fn a_field_that_does_not_parse_is_reported() {
    let mut c = criteria();
    c.stars = "lots".into();

    let result = check(&c);

    assert_eq!(result.route, Some(Route::Osu));
    assert!(result.problem.unwrap().contains("stars"));
}

#[test]
fn an_option_past_its_list_is_refused_not_clamped() {
    let mut c = criteria();
    c.mode = 99;

    assert_eq!(check(&c).problem.as_deref(), Some("mode: no option 99"));
    c.mode = 0;
    c.extra = vec![7];
    assert_eq!(check(&c).problem.as_deref(), Some("extra: no option 7"));
}

#[test]
fn a_preset_fills_the_form_like_the_tui() {
    let options = options();
    let farm = position(&options.presets, "farm");

    let c = FindCriteria::with_preset(farm).unwrap();

    assert_eq!(options.modes[c.mode], "osu!");
    assert_eq!(options.specials[c.special], "farm");
    assert_eq!(c.preset, farm);
    assert_eq!(check(&c).route, Some(Route::Nzbasic));
    assert!(FindCriteria::with_preset(99).is_err());
}

#[test]
fn a_find_download_lands_where_the_tuis_does() {
    let mut c = criteria();
    c.query = "camellia".into();
    let mut tui = FindSource::new();
    tui.query.set_value("camellia");

    let request = build_find_request(
        &FindPick {
            criteria: c,
            route: Route::Osu,
            beatmapset_ids: vec![3, 1, 3],
            sizes: HashMap::from([(1, 100), (9, 900)]),
        },
        run(),
    )
    .unwrap();

    assert_eq!(request.label, tui.run_label());
    assert_eq!(request.folder_tag, tui.folder_tag());
    assert_eq!(request.source, IdsRunSource::Search);
    assert_eq!(request.beatmapset_ids, [3, 1]);
    assert_eq!(request.known_sizes, HashMap::from([(1, 100)]));
    assert_eq!(request.osu_path, "/games/lazer");
}

#[test]
fn an_nzbasic_download_goes_in_its_filter_folder() {
    let options = options();
    let preset = FindCriteria::with_preset(position(&options.presets, "stream")).unwrap();

    let request = build_find_request(
        &FindPick {
            criteria: preset,
            route: Route::Nzbasic,
            beatmapset_ids: vec![5],
            sizes: HashMap::new(),
        },
        run(),
    )
    .unwrap();

    assert_eq!(request.source, IdsRunSource::Filter);
    assert_eq!(request.folder_tag, "stream");
}

fn diff(id: u32, set_id: u32, title: &str, stars: f64) -> BeatmapDetails {
    serde_json::from_value(serde_json::json!({
        "Id": id, "SetId": set_id, "Title": title, "Artist": "xi", "Creator": "someone",
        "Version": "Extra", "Stars": stars, "Bpm": 200.0, "Ar": 9.0, "Cs": 4.0, "Od": 8.0,
        "Hp": 6.0, "Approved": "ranked", "Mode": "osu", "TotalLength": 120,
    }))
    .unwrap()
}

#[test]
fn nzbasic_rows_follow_its_set_order_and_fill_what_details_came_back() {
    let details = [
        diff(11, 1, "Blue Zenith", 6.2),
        diff(12, 1, "Blue Zenith", 4.1),
        diff(21, 2, "Freedom Dive", 7.0),
    ];
    let sizes = HashMap::from([(2, 12_000_000)]);

    let rows = nzbasic_rows(&[2, 1, 3, 2], &details, &sizes);

    let summary: Vec<_> = rows
        .iter()
        .map(|r| (r.id, r.title.as_str(), r.stars, r.difficulties, r.size))
        .collect();
    assert_eq!(
        summary,
        [
            (2, "Freedom Dive", Some((7.0, 7.0)), 1, Some(12_000_000)),
            (1, "Blue Zenith", Some((4.1, 6.2)), 2, None),
            // No details came back for 3: it still shows, by id.
            (3, "", None, 0, None),
        ]
    );
    assert_eq!(rows[0].status, "ranked");
}

#[test]
fn a_find_download_with_nothing_picked_is_refused() {
    let downloads = Downloads::new(Arc::new(|_| {}));
    let request = build_find_request(
        &FindPick {
            criteria: criteria(),
            route: Route::Osu,
            beatmapset_ids: Vec::new(),
            sizes: HashMap::new(),
        },
        run(),
    )
    .unwrap();

    let err = downloads.enqueue_find(request).unwrap_err();

    assert!(err.to_string().starts_with("Nothing to download"), "{err}");
}

#[test]
fn downloaded_sets_come_from_every_run_folder_but_the_import_one() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::write(base.join("7.osz"), b"").unwrap();
    for (folder, file) in [
        ("search-freedom dive", "39804.osz"),
        (
            "stream 200-220-23907",
            "1011576 BABYMETAL - Road of Resistance.osz",
        ),
        ("stream 200-220-23907", "notes.txt"),
        ("stream 200-220-23907", "55.osz.download-1-2.tmp"),
        (super::IMPORT_FOLDER_NAME, "999.osz"),
    ] {
        std::fs::create_dir_all(base.join(folder)).unwrap();
        std::fs::write(base.join(folder).join(file), b"").unwrap();
    }

    let mut ids = super::downloaded_set_ids(base);
    ids.sort();
    assert_eq!(ids, vec![7, 39804, 1011576]);
    assert!(super::downloaded_set_ids(&base.join("missing")).is_empty());
}

#[test]
fn an_unreadable_library_still_marks_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("downloads");
    std::fs::create_dir_all(downloads.join("run")).unwrap();
    std::fs::write(downloads.join("run").join("12.osz"), b"").unwrap();
    let settings = crate::engine::Settings {
        client: OsuClient::Stable,
        // An install folder with no osu!.db in it.
        osu_path: Some(dir.path().join("osu")),
        download_dir: downloads,
    };

    let owned = super::owned(&settings);
    assert!(owned.in_osu.is_empty());
    assert!(owned.library_error.is_some());
    assert_eq!(owned.downloaded, vec![12]);
}
