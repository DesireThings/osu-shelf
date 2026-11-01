// Typed wrappers over the Tauri commands in src-tauri/src/lib.rs. Errors come
// back as plain sentences, ready to show.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type OsuClient = "lazer" | "stable";

export interface Installs {
  /** Every lazer data folder found, newest first. */
  lazer: string[];
  stable: string | null;
}

export interface Settings {
  client: OsuClient;
  osu_path: string | null;
  download_dir: string;
}

export interface CollectionPreview {
  id: number;
  name: string;
  uploader: string;
  beatmapsets: number;
  beatmaps: number;
}

export interface ImportFolderReport {
  runs: number;
  unpacked: number;
  already_unpacked: number;
  failed: [string, string][];
  collections: number;
  beatmap_hashes: number;
}

export interface LazerImportResult {
  folder: string;
  report: ImportFolderReport;
}

export interface MirrorSetting {
  host: string;
  enabled: boolean;
  name: string;
  note: string | null;
}

export type ArchiveValidation = "off" | "magic" | "eocd";

export interface DownloadOptions {
  /** Built-in mirrors in the order they're tried. */
  mirrors: MirrorSetting[];
  custom_mirrors: string[];
  video: boolean;
  /** Maps downloaded at once; null picks default_threads. */
  threads: number | null;
  default_threads: number;
  skip_already_imported: boolean;
  auto_skip_rate_limited: boolean;
  rate_limit_secs: number;
  archive_validation: ArchiveValidation;
}

export const loadDownloadOptions = () => invoke<DownloadOptions>("load_download_options");
export const saveDownloadOptions = (options: DownloadOptions) =>
  invoke<void>("save_download_options", { options });
/** Client id of the saved osu! API app, or null. */
export const savedApiApp = () => invoke<string | null>("saved_api_app");
export const saveApiApp = (clientId: string, clientSecret: string) =>
  invoke<string>("save_api_app", { clientId, clientSecret });
export const removeApiApp = () => invoke<void>("remove_api_app");

/** The licence list bundled with release builds; rejects in dev builds. */
export const thirdPartyLicences = () => invoke<string>("third_party_licences");

export const detectInstalls = () => invoke<Installs>("detect_installs");
export const loadSettings = () => invoke<Settings>("load_settings");
export const saveSettings = (settings: Settings) =>
  invoke<void>("save_settings", { settings });
export const previewCollection = (input: string) =>
  invoke<CollectionPreview>("preview_collection", { input });
export const buildLazerImport = (downloadDir: string) =>
  invoke<LazerImportResult>("build_lazer_import", { downloadDir });

export type Phase = "queued" | "preparing" | "downloading" | "done" | "failed" | "cancelled";

export interface Failure {
  beatmapset_id: number;
  title: string | null;
  reason: string;
}

/** A whole collection, the missing sets an update scan found, or picked search results. */
export type RunKind = "collection" | "update" | "find";

export interface DownloadStatus {
  id: number;
  kind: RunKind;
  /** Set for a collection run; null for the other kinds. */
  collection_id: number | null;
  name: string;
  phase: Phase;
  total: number;
  downloaded: number;
  skipped: number;
  failed: number;
  already_in_osu: number;
  bytes_done: number;
  bytes_total: number | null;
  active: number;
  /** Sets waiting because every mirror that has them is rate-limiting. */
  waiting: number;
  /** Seconds until the first waiting set retries, as of this update. */
  retry_in_secs: number | null;
  /** Sets skipped because the mirrors stayed busy (part of `skipped`). */
  skipped_busy: number;
  failures: Failure[];
  error: string | null;
  warning: string | null;
  folder: string | null;
}

export const startDownload = (input: string) => invoke<number>("start_download", { input });
export const cancelDownload = (id: number) => invoke<void>("cancel_download", { id });
/** Skip the sets a download is holding back for busy mirrors. */
export const skipWaiting = (id: number) => invoke<void>("skip_waiting", { id });
export const listDownloads = () => invoke<DownloadStatus[]>("list_downloads");
export const onDownloadStatus = (handler: (status: DownloadStatus) => void) =>
  listen<DownloadStatus>("download-status", (event) => handler(event.payload));

export interface MissingSet {
  beatmapset_id: number;
  /** Deleted from osu! after downloading; held back unless picked again. */
  previously_deleted: boolean;
}

export interface UpdateCollection {
  id: number;
  /** The name on osu!collector, else the name in osu!. */
  name: string;
  /** Maps the collection holds in osu!. */
  local_maps: number;
  missing: MissingSet[];
  /** Maps in osu! the collection on osu!collector no longer has. */
  removed_upstream: number;
}

export interface UpdateReport {
  client: OsuClient;
  library: string;
  local_sets: number;
  collections: UpdateCollection[];
  /** Sets left out because downloading them failed before. */
  known_bad: number;
  added_since_last_scan: number;
}

export type ScanProgress =
  | { step: "reading_library" }
  | { step: "checking"; local_sets: number; collections: number };

/** A previously deleted set picked again, in one collection. */
export interface SetRef {
  collection_id: number;
  beatmapset_id: number;
}

export interface UpdatePick {
  collection_ids: number[];
  reincluded: SetRef[];
}

export const scanUpdates = () => invoke<UpdateReport>("scan_updates");
export const downloadUpdates = (pick: UpdatePick) => invoke<number>("download_updates", { pick });
export const onScanProgress = (handler: (progress: ScanProgress) => void) =>
  listen<ScanProgress>("update-scan-progress", (event) => handler(event.payload));

/** The find form. Choice fields are positions in the FindOptions lists. */
export interface FindCriteria {
  preset: number;
  special: number;
  mode: number;
  status: number;
  sort: number;
  explicit: number;
  genre: number;
  language: number;
  extra: number[];
  query: string;
  stars: string;
  ar: string;
  cs: string;
  od: string;
  hp: string;
  bpm: string;
  length: string;
  keys: string;
  favourites: string;
  ranked: string;
  artist: string;
  creator: string;
  title: string;
  limit: string;
}

export interface FindOptions {
  presets: string[];
  specials: string[];
  modes: string[];
  statuses: string[];
  sorts: string[];
  explicit: string[];
  genres: string[];
  languages: string[];
  extras: string[];
  defaults: FindCriteria;
}

/** osu!'s own search (needs the user's API app), or nzbasic's map database. */
export type FindRoute = "osu" | "nzbasic";

export interface FindCheck {
  /** null when two criteria need different services. */
  route: FindRoute | null;
  problem: string | null;
}

export interface FindRow {
  id: number;
  /** Empty when nothing could be looked up for the set. */
  title: string;
  artist: string;
  creator: string;
  status: string;
  stars: [number, number] | null;
  difficulties: number;
  favourites: number | null;
  plays: number | null;
  size: number | null;
}

export interface FindPage {
  route: FindRoute;
  rows: FindRow[];
  total: number;
  next_cursor: string | null;
}

export interface FindPick {
  criteria: FindCriteria;
  route: FindRoute;
  beatmapset_ids: number[];
  sizes: Record<number, number>;
}

export const findOptions = () => invoke<FindOptions>("find_options");
export const findPreset = (preset: number) => invoke<FindCriteria>("find_preset", { preset });
export const findCheck = (criteria: FindCriteria) => invoke<FindCheck>("find_check", { criteria });
export const findMaps = (criteria: FindCriteria, cursor: string | null = null) =>
  invoke<FindPage>("find_maps", { criteria, cursor });
export const downloadFound = (pick: FindPick) => invoke<number>("download_found", { pick });

/** Map sets the user already has. */
export interface Owned {
  /** In the osu! library from Setup. */
  in_osu: number[];
  /** Downloaded into the download folder, imported or not. */
  downloaded: number[];
  /** Why the library couldn't be read; `in_osu` is empty then. */
  library_error: string | null;
}

export const findOwned = () => invoke<Owned>("find_owned");
