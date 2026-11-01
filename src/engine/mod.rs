//! UI-agnostic entry points for frontends other than the TUI (the GUI).
//!
//! Thin wrappers over the engine modules (`core`, `download`, `osu_db`,
//! `config`), grown one screen at a time as the GUI needs them. Nothing here
//! touches terminal or TUI state, and settings are the same `config.toml` the
//! TUI reads, so both frontends stay in step.

use crate::{
    config::{
        Config,
        constants::{APP_SUBDIR, LEGACY_APP_SUBDIR, USER_AGENT},
        load_config_or_default, save_config,
    },
    core::collection::{CollectionService, HttpCollectionService},
    osu_db::{BeatmapReader, LazerReader, OsuClient, StableReader},
    utils::{self, AppError, Result},
};
use osu_downloader::collection::CollectionClient;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod download;
pub mod find;
pub mod options;
pub mod updates;

pub use crate::download::lazer_import::{
    IMPORT_FOLDER_NAME, ImportFolderReport, build_import_folder,
};
pub use download::{DownloadStatus, Downloads, Failure, OnUpdate, Phase, RunKind};
pub use find::{FindCheck, FindCriteria, FindOptions, FindPage, FindPick, Owned};
pub use options::{
    DownloadOptions, MirrorSetting, load_download_options, remove_api_app, save_api_app,
    save_download_options, saved_api_app,
};
pub use updates::{ScanProgress, UpdatePick, UpdateReport, Updates};

/// osu! installs found in their default locations.
#[derive(Debug, Clone, Serialize)]
pub struct Installs {
    /// Folders holding lazer's `client.realm`, newest first.
    pub lazer: Vec<PathBuf>,
    /// Folder holding stable's `osu!.db`.
    pub stable: Option<PathBuf>,
}

pub fn detect_installs() -> Installs {
    Installs {
        lazer: LazerReader::detected_paths(),
        stable: StableReader::default_path(),
    }
}

/// The settings the GUI's setup screen edits; stored in the shared config's
/// `[recent]` table, the same keys the TUI's form persists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub client: OsuClient,
    /// osu! install folder; `None` means auto-detect.
    pub osu_path: Option<PathBuf>,
    pub download_dir: PathBuf,
}

impl Settings {
    fn from_config(config: &Config) -> Self {
        Self {
            client: config.recent.osu_client.unwrap_or_default(),
            osu_path: config.recent.osu_path.as_ref().map(PathBuf::from),
            download_dir: config
                .recent
                .directory
                .as_ref()
                .map(|dir| PathBuf::from(utils::expand_tilde(dir)))
                .unwrap_or_else(default_download_dir),
        }
    }

    fn apply_to(&self, config: &mut Config) {
        config.recent.osu_client = Some(self.client);
        config.recent.osu_path = self
            .osu_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        config.recent.directory = Some(self.download_dir.to_string_lossy().into_owned());
    }
}

/// `~/Downloads/osu-shelf` (or the home folder when there is no Downloads).
/// While the bridge release is in use, an existing `~/Downloads/osu-collect`
/// wins over a missing new folder so upgrades keep seeing their downloads.
fn default_download_dir() -> PathBuf {
    let base = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    default_download_dir_in(base)
}

fn default_download_dir_in(base: PathBuf) -> PathBuf {
    let current = base.join(APP_SUBDIR);
    let legacy = base.join(LEGACY_APP_SUBDIR);
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

pub fn load_settings() -> Settings {
    Settings::from_config(&load_config_or_default())
}

/// Write `settings` into the shared config, keeping every other key as is.
pub fn save_settings(settings: &Settings) -> Result<()> {
    let mut config = load_config_or_default();
    settings.apply_to(&mut config);
    save_config(&config).map(|_| ())
}

/// What the GUI shows for a pasted osu!collector link before downloading.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionPreview {
    pub id: u32,
    pub name: String,
    pub uploader: String,
    pub beatmapsets: usize,
    pub beatmaps: usize,
}

/// Shown instead of the URL parser's error, which names URL rules rather than
/// telling the user what to paste.
pub const NOT_A_COLLECTION_LINK: &str = "That doesn't look like an osu!collector link. \
     It should look like https://osucollector.com/collections/12345";

/// Resolve an osu!collector URL or bare id into its preview.
pub async fn preview_collection(input: &str) -> Result<CollectionPreview> {
    let id = utils::parse_collection_id(input.trim())
        .map_err(|_| AppError::other(NOT_A_COLLECTION_LINK))?;
    let collection = HttpCollectionService::new(CollectionClient::with_user_agent(USER_AGENT))
        .fetch_collection(id)
        .await?;
    Ok(CollectionPreview {
        id: collection.id,
        beatmaps: collection
            .beatmapsets
            .iter()
            .map(|s| s.beatmaps.len())
            .sum(),
        beatmapsets: collection.beatmapsets.len(),
        name: collection.name,
        uploader: collection.uploader.username,
    })
}

#[cfg(test)]
#[path = "../../tests/unit/engine.rs"]
mod tests;
