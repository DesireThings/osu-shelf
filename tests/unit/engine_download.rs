use super::{
    ABORTED, DownloadStatus, Downloads, PUSH_INTERVAL, Phase, Run, RunKind, mirrors_from_config,
};
use crate::app::{HomeTab, collection::FailureReason};
use crate::config::Config;
use crate::download::{
    BeatmapStage, DeferReason, DownloadConfig, DownloadEvent, DownloadHandle, DownloadStage,
    DownloadSummary, FailedMap, SelectiveDownloadRequest,
};
use crate::osu_db::OsuClient;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

fn mirror_names(mirrors: &[crate::mirrors::Mirror]) -> Vec<String> {
    mirrors.iter().map(|m| format!("{m:?}")).collect()
}

#[test]
fn mirrors_match_what_the_tui_builds() {
    for video in [false, true] {
        let mut config = Config::default();
        config.download.video = video;
        config.mirror.hinamizawa = true;
        config.mirror.nekoha = false;

        let tui = HomeTab::new(&config).build_mirror_list(false);

        assert_eq!(
            mirror_names(&mirrors_from_config(&config)),
            mirror_names(&tui)
        );
    }
}

#[test]
fn osu_official_is_never_used_without_the_tui_login() {
    let mut config = Config::default();
    config.mirror.osu_official = true;

    let names = mirror_names(&mirrors_from_config(&config)).join(" ");

    assert!(!names.contains("OsuApi"), "{names}");
}

fn status() -> DownloadStatus {
    DownloadStatus::queued(
        1,
        RunKind::Collection,
        Some(18951),
        "https://osucollector.com/collections/18951",
    )
}

fn beatmap(stage: BeatmapStage, set: u32) -> DownloadEvent {
    DownloadEvent::BeatmapStatus {
        id: 1,
        beatmapset_id: set,
        stage,
        message: String::new(),
        rate_limited: false,
        cooldown_until: None,
    }
}

#[test]
fn a_run_reports_name_progress_and_bytes() {
    let mut s = status();

    assert!(s.apply(&DownloadEvent::CollectionReady {
        id: 1,
        collection_name: "Tiebreakers".into(),
        uploader: "Sour Dough".into(),
        total_maps: 947,
        output_dir: "/dl/Tiebreakers-18951".into(),
    }));
    s.apply(&DownloadEvent::StageChanged {
        id: 1,
        stage: DownloadStage::Downloading,
    });
    s.apply(&beatmap(BeatmapStage::Downloading, 10));
    s.apply(&DownloadEvent::BeatmapProgress {
        id: 1,
        beatmapset_id: 10,
        downloaded: 400,
        total: 1000,
    });

    assert_eq!(s.name, "Tiebreakers");
    assert_eq!(s.total, 947);
    assert_eq!(s.phase, Phase::Downloading);
    assert_eq!(s.active, 1);
    assert_eq!(s.bytes_done, 400);

    s.apply(&beatmap(BeatmapStage::Success, 10));
    s.apply(&DownloadEvent::OverallProgress {
        id: 1,
        downloaded: 1,
        skipped: 5,
        failed: 0,
        unverified: 0,
    });

    assert_eq!(s.active, 0);
    assert_eq!(s.bytes_done, 1000, "a finished set counts at its full size");
    assert_eq!((s.downloaded, s.skipped), (1, 5));
}

fn deferred(set: u32, retry_in_secs: u64) -> DownloadEvent {
    DownloadEvent::BeatmapDeferred {
        id: 1,
        beatmapset_id: set,
        pass: 1,
        retry_in: Duration::from_secs(retry_in_secs),
        reason: DeferReason::Cooldown,
    }
}

#[test]
fn a_deferred_set_waits_instead_of_downloading() {
    let mut s = status();
    s.apply(&beatmap(BeatmapStage::Downloading, 10));
    s.apply(&beatmap(BeatmapStage::Downloading, 11));

    s.apply(&deferred(10, 30));

    assert_eq!((s.active, s.waiting), (1, 1));
    assert_eq!(s.retry_in_secs, Some(30));

    s.apply(&deferred(11, 90));

    assert_eq!((s.active, s.waiting), (0, 2));
    assert_eq!(s.retry_in_secs, Some(30), "the soonest retry");

    s.apply(&beatmap(BeatmapStage::Downloading, 10));
    s.apply(&beatmap(BeatmapStage::Failed, 11));

    assert_eq!((s.active, s.waiting), (1, 0));
    assert_eq!(s.retry_in_secs, None);
}

fn cooling(set: u32, secs: u64) -> DownloadEvent {
    DownloadEvent::BeatmapStatus {
        id: 1,
        beatmapset_id: set,
        stage: BeatmapStage::Downloading,
        message: String::new(),
        rate_limited: true,
        cooldown_until: Some(Instant::now() + Duration::from_secs(secs)),
    }
}

#[test]
fn a_set_on_a_mirror_cooldown_waits_instead_of_downloading() {
    let mut s = status();
    s.apply(&beatmap(BeatmapStage::Downloading, 10));

    s.apply(&cooling(10, 20));

    assert_eq!((s.active, s.waiting), (0, 1));
    assert!(
        matches!(s.retry_in_secs, Some(19 | 20)),
        "{:?}",
        s.retry_in_secs
    );

    s.apply(&beatmap(BeatmapStage::Downloading, 10));

    assert_eq!((s.active, s.waiting), (1, 0));
}

#[test]
fn a_healthy_set_waiting_its_turn_is_not_held_back_by_a_busy_mirror() {
    let mut s = status();
    s.apply(&beatmap(BeatmapStage::Downloading, 10));

    s.apply(&DownloadEvent::BeatmapDeferred {
        id: 1,
        beatmapset_id: 10,
        pass: 0,
        retry_in: Duration::from_secs(2),
        reason: DeferReason::Spacing,
    });

    assert_eq!((s.active, s.waiting), (0, 0));
}

#[test]
fn a_budget_deferred_set_at_pass_zero_waits() {
    let mut s = status();
    s.apply(&beatmap(BeatmapStage::Downloading, 10));

    s.apply(&DownloadEvent::BeatmapDeferred {
        id: 1,
        beatmapset_id: 10,
        pass: 0,
        retry_in: Duration::from_secs(600),
        reason: DeferReason::Budget,
    });

    assert_eq!((s.active, s.waiting), (0, 1));
    assert_eq!(s.retry_in_secs, Some(600));
}

#[test]
fn a_set_skipped_while_the_mirrors_were_busy_is_not_one_you_already_had() {
    let mut s = status();
    s.apply(&deferred(10, 60));
    s.apply(&cooling(11, 20));

    s.apply(&beatmap(BeatmapStage::Skipped, 10));
    s.apply(&beatmap(BeatmapStage::Skipped, 11));
    s.apply(&beatmap(BeatmapStage::Skipped, 12));

    assert_eq!(s.skipped_busy, 2);
    assert_eq!(s.waiting, 0);
}

#[tokio::test]
async fn skipping_waiting_sets_reaches_the_pipeline_only_when_some_are_waiting() {
    let downloads = Downloads::new(Arc::new(|_| {}));
    let (cancel, _) = watch::channel(false);
    let (defer, _) = watch::channel(0u64);
    let (skip, skips) = watch::channel(0u64);
    let handle = DownloadHandle::new(cancel, defer, skip, tokio::spawn(async {}));
    downloads.inner.lock().unwrap().runs.insert(
        1,
        Run {
            status: status(),
            request: None,
            handle: Some(handle),
        },
    );

    downloads.skip_waiting(1);
    assert_eq!(*skips.borrow(), 0, "nothing is waiting");

    downloads
        .inner
        .lock()
        .unwrap()
        .runs
        .get_mut(&1)
        .unwrap()
        .status
        .apply(&deferred(10, 60));
    downloads.skip_waiting(1);
    downloads.skip_waiting(2);

    assert_eq!(*skips.borrow(), 1);
}

#[tokio::test]
async fn an_update_held_back_by_the_throttle_still_goes_out() {
    let pushed: Arc<Mutex<Vec<DownloadStatus>>> = Arc::default();
    let downloads = Downloads::new({
        let pushed = pushed.clone();
        Arc::new(move |s: &DownloadStatus| pushed.lock().unwrap().push(s.clone()))
    });
    downloads.inner.lock().unwrap().runs.insert(
        1,
        Run {
            status: status(),
            request: None,
            handle: None,
        },
    );
    let (tx, rx) = mpsc::unbounded_channel();
    let pump = tokio::spawn(downloads.clone().pump(1, rx));

    // The second event lands inside the throttle window, then the pipeline
    // goes quiet without closing: every mirror is rate-limiting.
    tx.send(beatmap(BeatmapStage::Downloading, 10)).unwrap();
    tx.send(deferred(10, 60)).unwrap();
    tokio::time::sleep(PUSH_INTERVAL * 3).await;

    let last = pushed.lock().unwrap().last().cloned().expect("a push");
    assert_eq!((last.active, last.waiting), (0, 1));

    drop(tx);
    pump.await.unwrap();
}

#[test]
fn a_failed_set_does_not_count_its_partial_bytes() {
    let mut s = status();
    s.apply(&beatmap(BeatmapStage::Downloading, 10));
    s.apply(&DownloadEvent::BeatmapProgress {
        id: 1,
        beatmapset_id: 10,
        downloaded: 400,
        total: 1000,
    });

    s.apply(&beatmap(BeatmapStage::Failed, 10));

    assert_eq!(s.bytes_done, 0);
}

#[test]
fn finishing_records_failures_and_the_summary() {
    let mut s = status();
    s.apply(&DownloadEvent::FailedMaps {
        id: 1,
        failures: vec![FailedMap {
            beatmapset_id: 7,
            title: Some("Some Song".into()),
            reason: FailureReason::NotFound,
        }],
    });
    s.apply(&DownloadEvent::Finished {
        id: 1,
        summary: DownloadSummary {
            downloaded: 900,
            skipped: 46,
            failed: 1,
            unverified: 0,
        },
    });

    assert_eq!(s.phase, Phase::Done);
    assert_eq!((s.downloaded, s.skipped, s.failed), (900, 46, 1));
    assert_eq!(s.failures[0].reason, "not on any mirror");
}

#[test]
fn a_cancelled_run_stays_cancelled() {
    let mut s = status();
    s.apply(&DownloadEvent::Failed {
        id: 1,
        message: ABORTED.into(),
    });
    s.apply(&DownloadEvent::StageChanged {
        id: 1,
        stage: DownloadStage::Completed,
    });

    assert_eq!(s.phase, Phase::Cancelled);
    assert_eq!(s.error, None);
}

#[test]
fn a_pipeline_error_is_kept_for_the_user() {
    let mut s = status();
    s.apply(&DownloadEvent::Failed {
        id: 1,
        message: "collection not found (404)".into(),
    });

    assert_eq!(s.phase, Phase::Failed);
    assert_eq!(s.error.as_deref(), Some("collection not found (404)"));
}

#[test]
fn queueing_rejects_something_that_is_not_a_collection_link() {
    let downloads = Downloads::new(Arc::new(|_| {}));

    let err = downloads.enqueue("hello").unwrap_err();

    assert_eq!(err.to_string(), super::NOT_A_COLLECTION_LINK);
    assert!(downloads.list().is_empty());
}

fn update_request(beatmapset_ids: Vec<u32>) -> SelectiveDownloadRequest {
    SelectiveDownloadRequest {
        collection_ids: vec![18951],
        beatmapset_ids,
        collections: Vec::new(),
        config: DownloadConfig {
            directory: "/dl".into(),
            mirrors: Vec::new(),
            concurrent: 1,
            archive_validation: Default::default(),
            auto_skip_rate_limited: false,
            rate_limit_skip_secs: 60,
        },
        snapshot_dir: None,
        snapshots: Vec::new(),
        skip_already_imported: false,
        osu_client: OsuClient::Lazer,
        osu_path: String::new(),
        prefetched: HashMap::new(),
    }
}

fn running(downloads: &Downloads, id: u64, status: DownloadStatus) {
    let mut status = status;
    status.id = id;
    status.phase = Phase::Downloading;
    let mut inner = downloads.inner.lock().unwrap();
    inner.next_id = inner.next_id.max(id + 1);
    inner.runs.insert(
        id,
        Run {
            status,
            request: None,
            handle: None,
        },
    );
}

#[test]
fn an_update_with_nothing_picked_is_refused() {
    let downloads = Downloads::new(Arc::new(|_| {}));

    let err = downloads
        .enqueue_update(update_request(Vec::new()), "Update")
        .unwrap_err();

    assert!(err.to_string().starts_with("Nothing to download"), "{err}");
    assert!(downloads.list().is_empty());
}

#[test]
fn only_one_update_downloads_at_a_time() {
    let downloads = Downloads::new(Arc::new(|_| {}));
    running(
        &downloads,
        1,
        DownloadStatus::queued(1, RunKind::Update, None, "Update"),
    );

    let err = downloads
        .enqueue_update(update_request(vec![10]), "Update")
        .unwrap_err();

    assert_eq!(err.to_string(), "An update is already downloading.");
    assert_eq!(downloads.list().len(), 1);
    assert!(downloads.is_busy());
}

#[tokio::test]
async fn a_running_collection_does_not_block_an_update() {
    let downloads = Downloads::new(Arc::new(|_| {}));
    running(&downloads, 1, status());
    // The collection run holds the download slot, so the update only queues.
    let (cancel, _) = watch::channel(false);
    let (defer, _) = watch::channel(0u64);
    let (skip, _) = watch::channel(0u64);
    downloads
        .inner
        .lock()
        .unwrap()
        .runs
        .get_mut(&1)
        .unwrap()
        .handle = Some(DownloadHandle::new(
        cancel,
        defer,
        skip,
        tokio::spawn(async {}),
    ));

    let id = downloads
        .enqueue_update(update_request(vec![10]), "Update (1 collection)")
        .unwrap();

    let queued = downloads.list().into_iter().find(|s| s.id == id).unwrap();
    assert_eq!(queued.phase, Phase::Queued);
    assert_eq!((queued.kind, queued.collection_id), (RunKind::Update, None));
    assert_eq!(queued.name, "Update (1 collection)");
}

#[test]
fn a_finished_update_does_not_block_the_next_one() {
    let downloads = Downloads::new(Arc::new(|_| {}));
    let mut done = DownloadStatus::queued(1, RunKind::Update, None, "Update");
    done.phase = Phase::Done;
    downloads.inner.lock().unwrap().runs.insert(
        1,
        Run {
            status: done,
            request: None,
            handle: None,
        },
    );

    assert!(!downloads.is_busy());
}

#[test]
fn the_window_sees_the_run_kind() {
    let update = DownloadStatus::queued(3, RunKind::Update, None, "Update");

    let json = serde_json::to_value(&update).unwrap();

    assert_eq!(json["kind"], "update");
    assert_eq!(json["collection_id"], serde_json::Value::Null);
    assert_eq!(
        serde_json::to_value(status()).unwrap()["kind"],
        "collection"
    );
}
