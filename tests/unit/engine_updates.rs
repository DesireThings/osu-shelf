#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{RunSettings, Scan, SetRef, UpdatePick, UpdateReport, Updates, build_update_request};
use crate::app::runtime::FetchMissingResult;
use crate::app::snapshots::{CollectionSnapshot, CollectionSnapshotFile, SnapshotDiff};
use crate::app::update_source::{MissingBeatmapset, MissingStatus};
use crate::download::DownloadConfig;
use crate::engine::Downloads;
use crate::osu_db::{LocalBeatmapset, LocalCollection, OsuClient};
use osu_downloader::collection::Collection;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn missing(id: u32, collection_id: u32, previously_deleted: bool) -> MissingBeatmapset {
    MissingBeatmapset {
        id,
        status: MissingStatus::NotInstalled,
        collection_id,
        collection_name: format!("col {collection_id}"),
        included: !previously_deleted,
        previously_deleted,
        checksums: Box::new([]),
        enrich_diff_id: None,
    }
}

fn local(name: &str, maps: usize) -> LocalCollection {
    LocalCollection {
        name: name.to_string(),
        beatmap_checksums: vec![[1u8; 16]; maps].into_boxed_slice(),
    }
}

fn upstream(id: u32, name: &str) -> Collection {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "uploader": { "id": 1, "username": "someone" },
        "beatmapsets": [],
    }))
    .unwrap()
}

/// Two downloaded collections plus one osu! collection that isn't from
/// osu!collector. Set 7 is missing from both; 8 was deleted on purpose from
/// 100 and 9 from 200.
fn scan() -> Scan {
    Scan {
        client: OsuClient::Lazer,
        db_path: PathBuf::from("/games/lazer"),
        local_collections: vec![
            local("Tiebreakers-100", 12),
            local("favourites", 3),
            local("Stream practice-200", 5),
        ],
        local_beatmapsets: vec![LocalBeatmapset {
            id: 1,
            beatmaps: Box::new([]),
        }],
        collection_ids: vec![100, 200],
        current_snapshots: HashMap::from([
            (
                100,
                CollectionSnapshotFile::new(
                    100,
                    "Tiebreakers-100".into(),
                    CollectionSnapshot::default(),
                ),
            ),
            (
                200,
                CollectionSnapshotFile::new(
                    200,
                    "Stream practice-200".into(),
                    CollectionSnapshot::default(),
                ),
            ),
        ]),
        snapshot_diffs: HashMap::from([
            (
                100,
                SnapshotDiff {
                    manually_deleted: CollectionSnapshot {
                        stable_hashes: Vec::new(),
                        lazer_ids: vec![8],
                    },
                    manually_added: CollectionSnapshot::default(),
                },
            ),
            (
                200,
                SnapshotDiff {
                    manually_deleted: CollectionSnapshot {
                        stable_hashes: Vec::new(),
                        lazer_ids: vec![9],
                    },
                    manually_added: CollectionSnapshot::default(),
                },
            ),
        ]),
        fetched: FetchMissingResult {
            missing: vec![
                missing(7, 100, false),
                missing(8, 100, true),
                missing(7, 200, false),
                missing(9, 200, true),
                missing(10, 200, false),
            ],
            collection_seen: HashMap::new(),
            collection_removed_counts: HashMap::from([(200, 2)]),
            collections: vec![upstream(100, "Tiebreakers")],
            hidden_failed_count: 4,
        },
        added_since_last_scan: 1,
        db_read: Duration::ZERO,
        fetch: Duration::ZERO,
    }
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
        osu_client: OsuClient::Stable,
        osu_path: "/somewhere/else".into(),
    }
}

#[test]
fn the_report_lists_each_downloaded_collection_once() {
    let report = UpdateReport::from_scan(&scan());

    let names: Vec<_> = report.collections.iter().map(|c| c.name.as_str()).collect();
    // osu!collector's name where the scan fetched it, else the name in osu!.
    assert_eq!(names, ["Tiebreakers", "Stream practice-200"]);
    let streams = &report.collections[1];
    assert_eq!(streams.local_maps, 5);
    assert_eq!(streams.removed_upstream, 2);
    let missing: Vec<_> = streams
        .missing
        .iter()
        .map(|m| (m.beatmapset_id, m.previously_deleted))
        .collect();
    assert_eq!(missing, [(7, false), (9, true), (10, false)]);
    assert_eq!((report.known_bad, report.added_since_last_scan), (4, 1));
    assert_eq!(report.local_sets, 1);
}

#[test]
fn an_update_downloads_the_picked_collections_missing_sets_once_each() {
    let request = build_update_request(
        &scan(),
        &UpdatePick {
            collection_ids: vec![200, 100],
            reincluded: Vec::new(),
        },
        run(),
    );

    // Library order, not pick order; 7 is missing from both but fetched once,
    // and the deleted sets 8 and 9 stay held back.
    assert_eq!(request.collection_ids, [100, 200]);
    assert_eq!(request.beatmapset_ids, [7, 10]);
    let per_collection: Vec<_> = request
        .collections
        .iter()
        .map(|c| (c.id, c.name.as_str(), c.beatmapset_ids.clone()))
        .collect();
    assert_eq!(
        per_collection,
        [
            (100, "Tiebreakers-100", vec![7]),
            (200, "Stream practice-200", vec![7, 10]),
        ]
    );
}

#[test]
fn an_update_reads_the_library_the_scan_read() {
    let request = build_update_request(
        &scan(),
        &UpdatePick {
            collection_ids: vec![100],
            reincluded: Vec::new(),
        },
        run(),
    );

    assert_eq!(request.osu_client, OsuClient::Lazer);
    assert_eq!(request.osu_path, "/games/lazer");
    assert!(request.skip_already_imported);
    assert_eq!(request.config.directory, "/dl");
    assert_eq!(
        request.prefetched.keys().copied().collect::<Vec<_>>(),
        [100]
    );
}

#[test]
fn a_deleted_set_picked_again_downloads_and_stops_being_held_back() {
    let request = build_update_request(
        &scan(),
        &UpdatePick {
            collection_ids: vec![100, 200],
            reincluded: vec![SetRef {
                collection_id: 100,
                beatmapset_id: 8,
            }],
        },
        run(),
    );

    assert_eq!(request.beatmapset_ids, [7, 8, 10]);
    // The baselines the run writes: 8 is no longer recorded as deleted from
    // 100, while 9, still held back, stays recorded as deleted from 200.
    let deleted: HashMap<u32, Vec<u64>> = request
        .snapshots
        .iter()
        .map(|file| {
            (
                file.collection_id.parse().unwrap(),
                file.snapshot.lazer_ids.clone(),
            )
        })
        .collect();
    assert_eq!(deleted[&100], Vec::<u64>::new());
    assert_eq!(deleted[&200], [9]);
}

#[test]
fn picking_a_deleted_set_in_one_collection_leaves_it_held_back_in_another() {
    let mut scan = scan();
    scan.fetched.missing.push(missing(8, 200, true));

    let request = build_update_request(
        &scan,
        &UpdatePick {
            collection_ids: vec![200],
            reincluded: vec![SetRef {
                collection_id: 100,
                beatmapset_id: 8,
            }],
        },
        run(),
    );

    assert_eq!(request.beatmapset_ids, [7, 10]);
}

#[test]
fn downloading_before_any_scan_says_to_scan() {
    let updates = Updates::default();
    let downloads = Downloads::new(Arc::new(|_| {}));

    let err = updates
        .download(&downloads, &UpdatePick::default())
        .unwrap_err();

    assert_eq!(err.to_string(), "Scan for updates first.");
}
