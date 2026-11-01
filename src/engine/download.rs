//! Collection downloads for the GUI.
//!
//! Builds the same request the TUI's form does, but from the shared config,
//! runs it on the download pipeline, and folds the pipeline's event stream into
//! one serialisable [`DownloadStatus`] per run. A run is a whole collection,
//! the missing sets an update scan found, or map sets picked from a search. Runs queue and download one at a
//! time: each already uses every configured thread, and ten runs at once would
//! hit the mirrors ten times as hard.

use super::{NOT_A_COLLECTION_LINK, Settings};
use crate::{
    app::{collection::FailureReason, custom_mirrors::CustomMirrorList},
    config::{Config, MirrorConfig, load_config_or_default},
    download::{
        BeatmapStage, DeferReason, DownloadConfig, DownloadEvent, DownloadHandle, DownloadId,
        DownloadRequest, DownloadStage, IdsDownloadRequest, SchedulerHandle,
        SelectiveDownloadRequest, spawn_download, spawn_ids_download, spawn_selective_download,
    },
    mirrors::{Mirror, MirrorKind},
    osu_db::{BeatmapReader, LazerReader, OsuClient, StableReader},
    utils::{self, AppError, Result},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// The message the pipeline fails a run with when it was cancelled.
const ABORTED: &str = "download aborted by user";
/// Progress pushes per run are capped to this rate; phase changes and the end
/// of a run always go out at once.
const PUSH_INTERVAL: Duration = Duration::from_millis(150);

/// Where a collection's download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Waiting for the run ahead of it.
    Queued,
    /// Fetching the collection and checking what's already on disk.
    Preparing,
    Downloading,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// What a run downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    /// A whole osu!collector collection.
    Collection,
    /// The missing sets an update scan found, across the collections picked.
    Update,
    /// Map sets picked from a search.
    Find,
}

/// One map set that couldn't be downloaded.
#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub beatmapset_id: u32,
    pub title: Option<String>,
    pub reason: &'static str,
}

/// One mirror's wait behind the download forecast, as the window sees it.
/// The library's raw schedule type stays internal to the engine.
#[derive(Debug, Clone, Serialize)]
pub struct MirrorForecast {
    /// Mirror's display label: the provider name, or a custom mirror's host.
    pub mirror: String,
    /// Seconds until the mirror can send its next request, counting request
    /// spacing, a live 429 cooldown, and any live budget.
    pub next_free_secs: u64,
    /// Seconds left on a reactive 429 cooldown, when one is live.
    pub cooldown_secs: Option<u64>,
    /// Seconds until a live request or byte budget admits the next request,
    /// when the mirror is budget-blocked.
    pub budget_secs: Option<u64>,
    /// Fewest requests still affordable against the mirror's live request
    /// budgets; `None` when it publishes none (anonymous mirrors).
    pub remaining_requests: Option<u64>,
}

impl MirrorForecast {
    fn from_schedule(entry: &osu_downloader::MirrorSchedule) -> Self {
        let secs = |duration: Duration| duration.as_secs_f64().ceil() as u64;
        Self {
            mirror: entry.mirror.label().to_string(),
            next_free_secs: secs(entry.next_free_in),
            cooldown_secs: entry.cooldown_in.map(secs),
            budget_secs: entry.budget_in.map(secs),
            remaining_requests: entry.remaining_requests,
        }
    }
}

/// Everything the GUI shows for one collection's download.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadStatus {
    pub id: DownloadId,
    pub kind: RunKind,
    /// The collection a [`RunKind::Collection`] run downloads; `None` for the
    /// other kinds.
    pub collection_id: Option<u32>,
    /// Collection name once fetched; the pasted link (or the update's label)
    /// until then.
    pub name: String,
    pub phase: Phase,
    /// Map sets in the collection.
    pub total: usize,
    pub downloaded: u32,
    /// Already on disk or already in osu! (includes `already_in_osu`).
    pub skipped: u32,
    pub failed: u32,
    pub already_in_osu: usize,
    /// Bytes of finished sets plus the sets in flight.
    pub bytes_done: u64,
    /// Estimated size of what's left to download, when known.
    pub bytes_total: Option<u64>,
    /// Sets downloading right now.
    pub active: usize,
    /// Sets held back because every mirror that has them is rate-limiting,
    /// either waiting out a short cooldown or put back in the queue; they retry
    /// on their own, or [`Downloads::skip_waiting`] drops them.
    pub waiting: usize,
    /// Seconds until the first waiting set retries, as of this update.
    pub retry_in_secs: Option<u64>,
    /// Sets skipped because the mirrors stayed busy (included in `skipped`);
    /// the next run of the collection tries them again.
    pub skipped_busy: u32,
    pub failures: Vec<Failure>,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// Folder the collection's maps go into, once known.
    pub folder: Option<String>,
    /// Seconds until every outstanding request can have been sent, from the
    /// latest mirror-schedule forecast: a lower bound on the time left, since
    /// bytes for sent requests still have to arrive. `None` until the first
    /// forecast arrives and after the run ends; `Some(0)` means the mirrors can
    /// send everything still queued at once.
    pub eta_secs: Option<u64>,
    /// Per-mirror waits behind [`Self::eta_secs`], in mirror-list order.
    pub mirrors: Vec<MirrorForecast>,
    #[serde(skip)]
    finished_bytes: u64,
    #[serde(skip)]
    in_flight: HashMap<u32, (u64, u64)>,
    #[serde(skip)]
    active_sets: HashSet<u32>,
    /// Waiting sets and when each one retries.
    #[serde(skip)]
    deferred: HashMap<u32, Instant>,
    /// Every set a mirror rate-limited this run, so a later skip of one counts
    /// as `skipped_busy` rather than "already had it".
    #[serde(skip)]
    rate_limited: HashSet<u32>,
}

impl DownloadStatus {
    fn queued(id: DownloadId, kind: RunKind, collection_id: Option<u32>, name: &str) -> Self {
        Self {
            id,
            kind,
            collection_id,
            name: name.to_string(),
            phase: Phase::Queued,
            total: 0,
            downloaded: 0,
            skipped: 0,
            failed: 0,
            already_in_osu: 0,
            bytes_done: 0,
            bytes_total: None,
            active: 0,
            waiting: 0,
            retry_in_secs: None,
            skipped_busy: 0,
            failures: Vec::new(),
            error: None,
            warning: None,
            folder: None,
            eta_secs: None,
            mirrors: Vec::new(),
            finished_bytes: 0,
            in_flight: HashMap::new(),
            active_sets: HashSet::new(),
            deferred: HashMap::new(),
            rate_limited: HashSet::new(),
        }
    }

    /// Fold one pipeline event in. Returns whether it should be pushed now
    /// rather than waiting for the next throttled push.
    fn apply(&mut self, event: &DownloadEvent) -> bool {
        match event {
            DownloadEvent::CollectionReady {
                collection_name,
                total_maps,
                output_dir,
                ..
            } => {
                self.name = collection_name.clone();
                self.total = *total_maps;
                self.folder = Some(output_dir.clone());
                true
            }
            DownloadEvent::CollectionSizeResolved { total_bytes, .. } => {
                self.bytes_total = Some(*total_bytes);
                false
            }
            DownloadEvent::LowDiskSpace {
                available_bytes, ..
            } => {
                self.warning = Some(format!(
                    "Low disk space: {} MB free",
                    available_bytes / 1_000_000
                ));
                true
            }
            DownloadEvent::BeatmapProgress {
                beatmapset_id,
                downloaded,
                total,
                ..
            } => {
                self.in_flight.insert(*beatmapset_id, (*downloaded, *total));
                self.refresh_bytes();
                false
            }
            DownloadEvent::BeatmapStatus {
                beatmapset_id,
                rate_limited: true,
                cooldown_until,
                ..
            } => {
                // Every mirror that has it is cooling down; it holds its worker
                // while it waits, but it isn't downloading.
                self.active_sets.remove(beatmapset_id);
                self.rate_limited.insert(*beatmapset_id);
                self.deferred
                    .insert(*beatmapset_id, cooldown_until.unwrap_or_else(Instant::now));
                self.active = self.active_sets.len();
                self.refresh_waiting();
                false
            }
            DownloadEvent::BeatmapStatus {
                beatmapset_id,
                stage,
                ..
            } => {
                match stage {
                    BeatmapStage::Downloading | BeatmapStage::Verifying => {
                        self.active_sets.insert(*beatmapset_id);
                        self.deferred.remove(beatmapset_id);
                    }
                    BeatmapStage::Pending => {
                        self.active_sets.remove(beatmapset_id);
                    }
                    terminal => {
                        self.active_sets.remove(beatmapset_id);
                        self.deferred.remove(beatmapset_id);
                        if *terminal == BeatmapStage::Skipped
                            && self.rate_limited.contains(beatmapset_id)
                        {
                            self.skipped_busy += 1;
                        }
                        if let Some((done, size)) = self.in_flight.remove(beatmapset_id)
                            && *terminal == BeatmapStage::Success
                        {
                            self.finished_bytes += size.max(done);
                        }
                    }
                }
                self.active = self.active_sets.len();
                self.refresh_bytes();
                self.refresh_waiting();
                false
            }
            DownloadEvent::BeatmapDeferred {
                beatmapset_id,
                pass,
                retry_in,
                reason,
                ..
            } => {
                self.active_sets.remove(beatmapset_id);
                self.in_flight.remove(beatmapset_id);
                // A spacing requeue at pass 0 is a healthy set waiting its turn
                // on a mirror's request spacing: it's just queued, and a skip
                // wouldn't drop it. Cooldown and budget waits are genuine waits.
                if *pass == 0 && matches!(*reason, DeferReason::Spacing) {
                    self.deferred.remove(beatmapset_id);
                } else {
                    self.rate_limited.insert(*beatmapset_id);
                    self.deferred
                        .insert(*beatmapset_id, Instant::now() + *retry_in);
                }
                self.active = self.active_sets.len();
                self.refresh_bytes();
                self.refresh_waiting();
                false
            }
            DownloadEvent::Schedule { schedule, .. } => {
                self.eta_secs = Some(schedule.request_eta.as_secs_f64().ceil() as u64);
                self.mirrors = schedule
                    .mirrors
                    .iter()
                    .map(MirrorForecast::from_schedule)
                    .collect();
                false
            }
            DownloadEvent::OverallProgress {
                downloaded,
                skipped,
                failed,
                ..
            } => {
                self.downloaded = *downloaded;
                self.skipped = *skipped;
                self.failed = *failed;
                false
            }
            DownloadEvent::SkippedImported { count, .. } => {
                self.already_in_osu = *count;
                false
            }
            DownloadEvent::LibraryReadFailed { message, .. } => {
                self.warning = Some(message.clone());
                true
            }
            DownloadEvent::FailedMaps { failures, .. } => {
                self.failures = failures
                    .iter()
                    .map(|f| Failure {
                        beatmapset_id: f.beatmapset_id,
                        title: f.title.clone(),
                        reason: failure_reason(f.reason),
                    })
                    .collect();
                true
            }
            DownloadEvent::StageChanged { stage, .. } => {
                let next = match stage {
                    DownloadStage::Pending
                    | DownloadStage::Resolving
                    | DownloadStage::Rechecking => Phase::Preparing,
                    DownloadStage::Downloading => Phase::Downloading,
                    DownloadStage::Completed => Phase::Done,
                    DownloadStage::Failed => Phase::Failed,
                };
                // A cancelled or failed run may still report a stage change on
                // its way out; it stays cancelled/failed.
                if !self.phase.is_finished() {
                    self.phase = next;
                }
                true
            }
            DownloadEvent::Finished { summary, .. } => {
                self.downloaded = summary.downloaded;
                self.skipped = summary.skipped;
                self.failed = summary.failed;
                self.phase = Phase::Done;
                self.clear_in_flight();
                true
            }
            DownloadEvent::Failed { message, .. } => {
                if message == ABORTED {
                    self.phase = Phase::Cancelled;
                } else {
                    self.phase = Phase::Failed;
                    self.error = Some(message.clone());
                }
                self.clear_in_flight();
                true
            }
            _ => false,
        }
    }

    fn refresh_bytes(&mut self) {
        self.bytes_done =
            self.finished_bytes + self.in_flight.values().map(|(done, _)| done).sum::<u64>();
    }

    fn refresh_waiting(&mut self) {
        let now = Instant::now();
        self.waiting = self.deferred.len();
        self.retry_in_secs = self
            .deferred
            .values()
            .min()
            .map(|at| at.saturating_duration_since(now).as_secs_f64().ceil() as u64);
    }

    fn clear_in_flight(&mut self) {
        self.in_flight.clear();
        self.active_sets.clear();
        self.deferred.clear();
        self.eta_secs = None;
        self.mirrors.clear();
        self.active = 0;
        self.refresh_bytes();
        self.refresh_waiting();
    }
}

fn failure_reason(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::NotFound => "not on any mirror",
        FailureReason::RateLimited => "mirrors were rate-limiting",
        FailureReason::NetworkError => "network error",
        FailureReason::ValidationFailed => "download was corrupt",
        FailureReason::Unknown => "unknown error",
    }
}

/// The mirrors a download tries, in the configured order, as the TUI builds
/// them. osu! official is left out: it needs the TUI's login.
pub fn mirrors_from_config(config: &Config) -> Vec<Mirror> {
    let video = config.download.video;
    let mut mirrors: Vec<Mirror> = config
        .mirror
        .ordered_builtins()
        .into_iter()
        .filter(|&kind| mirror_enabled(&config.mirror, kind))
        .filter_map(Mirror::builtin)
        .map(|mirror| if video { mirror } else { mirror.no_video() })
        .collect();
    mirrors.extend(
        CustomMirrorList::from_templates(&config.mirror.custom_templates()).build_mirrors(video),
    );
    mirrors
}

fn mirror_enabled(mirror: &MirrorConfig, kind: MirrorKind) -> bool {
    match kind {
        MirrorKind::Nerinyan => mirror.nerinyan,
        MirrorKind::OsuDirect => mirror.osu_direct,
        MirrorKind::Sayobot => mirror.sayobot,
        MirrorKind::Nekoha => mirror.nekoha,
        MirrorKind::Beatconnect => mirror.beatconnect,
        MirrorKind::Osudl => mirror.osudl,
        MirrorKind::Catboy => mirror.catboy,
        MirrorKind::Hinamizawa => mirror.hinamizawa,
        MirrorKind::Nzbasic => mirror.nzbasic,
        MirrorKind::OsuApi | MirrorKind::Custom => false,
    }
}

/// What every kind of run takes from the config and settings.
pub(super) struct RunSettings {
    pub config: DownloadConfig,
    pub skip_already_imported: bool,
    pub osu_client: OsuClient,
    /// The osu! install the run checks for maps you already have.
    pub osu_path: String,
}

pub(super) fn run_settings(config: &Config) -> Result<RunSettings> {
    let settings = Settings::from_config(config);
    let mirrors = mirrors_from_config(config);
    if mirrors.is_empty() {
        return Err(AppError::other(
            "No download mirrors are turned on. Turn one on in Settings.",
        ));
    }
    let osu_path = settings
        .osu_path
        .clone()
        .or_else(|| match settings.client {
            OsuClient::Lazer => LazerReader::default_path(),
            OsuClient::Stable => StableReader::default_path(),
        })
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(RunSettings {
        config: DownloadConfig {
            directory: settings.download_dir.to_string_lossy().into_owned(),
            mirrors,
            concurrent: config.download.resolved_concurrent(),
            archive_validation: config.download.archive_validation,
            auto_skip_rate_limited: config.download.auto_skip_rate_limited,
            rate_limit_skip_secs: config.download.rate_limit_skip_secs,
            // `Downloads` injects its shared handle when the job is queued.
            scheduler: None,
        },
        skip_already_imported: config.download.skip_already_imported,
        osu_client: settings.client,
        osu_path,
    })
}

/// The request for `input`, from the current config and settings.
fn build_request(input: &str, config: &Config) -> Result<DownloadRequest> {
    let run = run_settings(config)?;
    Ok(DownloadRequest {
        collection_input: input.to_string(),
        config: run.config,
        auto_overwrite: false,
        previously_failed_skipped: HashSet::new(),
        skip_already_imported: run.skip_already_imported,
        osu_client: run.osu_client,
        osu_path: run.osu_path,
        prefetched: None,
    })
}

/// A queued run's request, by kind.
enum Job {
    Collection(DownloadRequest),
    Update(SelectiveDownloadRequest),
    Find(IdsDownloadRequest),
}

impl Job {
    /// Point the run at the queue's shared scheduler, so mirror spacing and
    /// budgets carry across consecutive runs.
    fn with_scheduler(mut self, scheduler: &SchedulerHandle) -> Self {
        match &mut self {
            Self::Collection(request) => request.config.scheduler = Some(scheduler.clone()),
            Self::Update(request) => request.config.scheduler = Some(scheduler.clone()),
            Self::Find(request) => request.config.scheduler = Some(scheduler.clone()),
        }
        self
    }
}

/// Called with a run's status whenever it changes (throttled while downloading).
pub type OnUpdate = Arc<dyn Fn(&DownloadStatus) + Send + Sync>;

struct Run {
    status: DownloadStatus,
    /// Built at queue time; taken when the run starts.
    request: Option<Job>,
    handle: Option<DownloadHandle>,
}

struct Inner {
    next_id: DownloadId,
    runs: BTreeMap<DownloadId, Run>,
}

/// The GUI's download queue. Cheap to clone; clones share the queue.
#[derive(Clone)]
pub struct Downloads {
    inner: Arc<Mutex<Inner>>,
    on_update: OnUpdate,
    /// One scheduler handle for every run this queue starts, so a mirror's
    /// spacing and budget state survives from one run to the next.
    scheduler: SchedulerHandle,
}

impl Downloads {
    pub fn new(on_update: OnUpdate) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_id: 1,
                runs: BTreeMap::new(),
            })),
            on_update,
            scheduler: SchedulerHandle::new(),
        }
    }

    /// Queue a collection (osu!collector link or id). Starts at once when
    /// nothing else is downloading. Must run inside a Tokio runtime.
    pub fn enqueue(&self, input: &str) -> Result<DownloadId> {
        let input = input.trim();
        let collection_id = utils::parse_collection_id(input)
            .map_err(|_| AppError::other(NOT_A_COLLECTION_LINK))?;
        let request = build_request(input, &load_config_or_default())?;

        let status = DownloadStatus::queued(0, RunKind::Collection, Some(collection_id), input);
        self.push(status, Job::Collection(request), |run| {
            (run.collection_id == Some(collection_id))
                .then_some("That collection is already downloading.")
        })
    }

    /// Queue the missing sets of an update scan; `label` names the run until
    /// the pipeline has resolved it. One update downloads at a time. Must run
    /// inside a Tokio runtime.
    pub fn enqueue_update(
        &self,
        request: SelectiveDownloadRequest,
        label: &str,
    ) -> Result<DownloadId> {
        if request.beatmapset_ids.is_empty() {
            return Err(AppError::other(
                "Nothing to download: no missing maps are picked.",
            ));
        }
        let status = DownloadStatus::queued(0, RunKind::Update, None, label);
        self.push(status, Job::Update(request), |run| {
            (run.kind == RunKind::Update).then_some("An update is already downloading.")
        })
    }

    /// Queue map sets picked from a search. A second run of the same search
    /// (its folder) waits for the first to finish. Must run inside a Tokio
    /// runtime.
    pub fn enqueue_find(&self, request: IdsDownloadRequest) -> Result<DownloadId> {
        if request.beatmapset_ids.is_empty() {
            return Err(AppError::other(
                "Nothing to download: no map sets are picked.",
            ));
        }
        let label = request.label.clone();
        let status = DownloadStatus::queued(0, RunKind::Find, None, &label);
        self.push(status, Job::Find(request), |run| {
            (run.kind == RunKind::Find && run.name == label)
                .then_some("That search is already downloading.")
        })
    }

    /// Add a run unless `clash` names an unfinished run it would duplicate,
    /// then start it if nothing else is downloading.
    fn push(
        &self,
        mut status: DownloadStatus,
        job: Job,
        clash: impl Fn(&DownloadStatus) -> Option<&'static str>,
    ) -> Result<DownloadId> {
        let job = job.with_scheduler(&self.scheduler);
        {
            let mut inner = self.inner.lock().expect("downloads lock");
            if let Some(message) = inner
                .runs
                .values()
                .filter(|run| !run.status.phase.is_finished())
                .find_map(|run| clash(&run.status))
            {
                return Err(AppError::other(message));
            }
            status.id = inner.next_id;
            inner.next_id += 1;
            inner.runs.insert(
                status.id,
                Run {
                    status: status.clone(),
                    request: Some(job),
                    handle: None,
                },
            );
        }
        (self.on_update)(&status);
        self.start_next();
        Ok(status.id)
    }

    /// Whether any run is queued or downloading.
    pub fn is_busy(&self) -> bool {
        let inner = self.inner.lock().expect("downloads lock");
        inner
            .runs
            .values()
            .any(|run| !run.status.phase.is_finished())
    }

    /// Cancel a queued or running download. Finished ones are left alone.
    pub fn cancel(&self, id: DownloadId) {
        let cancelled_queued = {
            let mut inner = self.inner.lock().expect("downloads lock");
            let Some(run) = inner.runs.get_mut(&id) else {
                return;
            };
            match (&run.handle, run.status.phase) {
                (Some(handle), phase) if !phase.is_finished() => {
                    handle.request_shutdown();
                    None
                }
                (None, Phase::Queued) => {
                    run.request = None;
                    run.status.phase = Phase::Cancelled;
                    Some(run.status.clone())
                }
                _ => None,
            }
        };
        if let Some(status) = cancelled_queued {
            (self.on_update)(&status);
        }
    }

    /// Skip the sets a running download is holding back for busy mirrors. They
    /// count as skipped, and running the collection again tries them.
    pub fn skip_waiting(&self, id: DownloadId) {
        let inner = self.inner.lock().expect("downloads lock");
        if let Some(Run {
            handle: Some(handle),
            status,
            ..
        }) = inner.runs.get(&id)
            && status.waiting > 0
        {
            handle.skip_rate_limited();
        }
    }

    /// Every download this session, oldest first.
    pub fn list(&self) -> Vec<DownloadStatus> {
        let inner = self.inner.lock().expect("downloads lock");
        inner.runs.values().map(|run| run.status.clone()).collect()
    }

    /// Start the oldest queued run when nothing is running.
    fn start_next(&self) {
        let (id, request) = {
            let mut inner = self.inner.lock().expect("downloads lock");
            if inner.runs.values().any(|run| run.handle.is_some()) {
                return;
            }
            let Some((id, run)) = inner
                .runs
                .iter_mut()
                .find(|(_, run)| run.status.phase == Phase::Queued && run.request.is_some())
            else {
                return;
            };
            run.status.phase = Phase::Preparing;
            (*id, run.request.take().expect("queued run has a request"))
        };

        let (tx, rx) = mpsc::unbounded_channel();
        let handle = match request {
            Job::Collection(request) => spawn_download(id, request, tx),
            Job::Update(request) => spawn_selective_download(id, request, tx),
            Job::Find(request) => spawn_ids_download(id, request, tx),
        };
        if let Some(run) = self.inner.lock().expect("downloads lock").runs.get_mut(&id) {
            run.handle = Some(handle);
        }
        let this = self.clone();
        tokio::spawn(async move { this.pump(id, rx).await });
    }

    fn status(&self, id: DownloadId) -> Option<DownloadStatus> {
        let inner = self.inner.lock().expect("downloads lock");
        inner.runs.get(&id).map(|run| run.status.clone())
    }

    /// Fold one run's events into its status until the pipeline drops its
    /// sender, then start the next queued run. An update the throttle holds
    /// back goes out once the interval is up, even if no event follows it:
    /// a run that goes quiet (every mirror rate-limiting, say) must not leave
    /// the window showing old numbers.
    async fn pump(self, id: DownloadId, mut rx: mpsc::UnboundedReceiver<DownloadEvent>) {
        let mut last_push: Option<Instant> = None;
        let mut held_back = false;
        loop {
            let event = match last_push.filter(|_| held_back) {
                Some(at) => {
                    let flush_at = tokio::time::Instant::from_std(at + PUSH_INTERVAL);
                    tokio::select! {
                        event = rx.recv() => event,
                        () = tokio::time::sleep_until(flush_at) => {
                            held_back = false;
                            last_push = Some(Instant::now());
                            if let Some(status) = self.status(id) {
                                (self.on_update)(&status);
                            }
                            continue;
                        }
                    }
                }
                None => rx.recv().await,
            };
            let Some(event) = event else {
                break;
            };

            let push = {
                let mut inner = self.inner.lock().expect("downloads lock");
                let Some(run) = inner.runs.get_mut(&id) else {
                    continue;
                };
                let urgent = run.status.apply(&event);
                let due = last_push.is_none_or(|at| at.elapsed() >= PUSH_INTERVAL);
                (urgent || due).then(|| run.status.clone())
            };
            match push {
                Some(status) => {
                    last_push = Some(Instant::now());
                    held_back = false;
                    (self.on_update)(&status);
                }
                None => held_back = true,
            }
        }

        let last = {
            let mut inner = self.inner.lock().expect("downloads lock");
            inner.runs.get_mut(&id).map(|run| {
                run.handle = None;
                if !run.status.phase.is_finished() {
                    // The pipeline ended without saying how; don't leave it spinning.
                    run.status.phase = Phase::Failed;
                    run.status.error = Some("The download stopped unexpectedly.".to_string());
                    run.status.clear_in_flight();
                }
                run.status.clone()
            })
        };
        if let Some(status) = last {
            (self.on_update)(&status);
        }
        self.start_next();
    }
}

#[cfg(test)]
#[path = "../../tests/unit/engine_download.rs"]
mod tests;
