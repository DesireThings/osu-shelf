//! The GUI's settings screen: download mirrors and options, read from and
//! written to the same `config.toml` the TUI's config tab edits, plus the
//! user's own osu! API app (stored in `auth.json`, see [`crate::auth`]).

use crate::{
    auth,
    config::{
        Config, MirrorConfig, constants::default_threads, load_config_or_default, save_config,
    },
    utils::{AppError, Result},
};
use osu_downloader::{ArchiveValidation, MirrorKind};
use serde::{Deserialize, Serialize};

/// One built-in mirror as the settings screen lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorSetting {
    /// The mirror's host, which is also its key in `[mirror].order`.
    pub host: String,
    pub enabled: bool,
    /// Display name; ignored on save.
    #[serde(default)]
    pub name: String,
    /// What to know before turning it on or off; ignored on save.
    #[serde(default)]
    pub note: Option<String>,
    /// One-line summary of its known limits, `None` when they aren't known;
    /// ignored on save.
    #[serde(default)]
    pub limits: Option<String>,
}

/// The download settings the GUI edits. Saving keeps every other key in
/// `config.toml` as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadOptions {
    /// Built-in mirrors in the order they're tried. osu! official is left out:
    /// it needs a login this app can't make.
    pub mirrors: Vec<MirrorSetting>,
    /// Custom mirror URL templates, each with `{id}`, tried after the built-ins.
    pub custom_mirrors: Vec<String>,
    pub video: bool,
    /// Maps downloaded at once; `None` picks [`default_threads`].
    pub threads: Option<u8>,
    /// What `threads: None` means on this machine; ignored on save.
    #[serde(default)]
    pub default_threads: u8,
    pub skip_already_imported: bool,
    /// Requeue maps stuck on a rate-limited mirror after `rate_limit_secs`.
    pub auto_skip_rate_limited: bool,
    pub rate_limit_secs: u32,
    pub archive_validation: ArchiveValidation,
}

/// Most maps downloaded at once, as the TUI's config tab allows.
const MAX_THREADS: u8 = 100;

impl DownloadOptions {
    fn from_config(config: &Config) -> Self {
        let download = &config.download;
        Self {
            mirrors: config
                .mirror
                .ordered_builtins()
                .into_iter()
                .filter_map(|kind| {
                    Some(MirrorSetting {
                        host: kind.host().to_string(),
                        enabled: *mirror_flag(&config.mirror, kind)?,
                        name: mirror_name(kind).to_string(),
                        note: mirror_note(kind).map(str::to_string),
                        limits: kind.limit_summary().map(str::to_string),
                    })
                })
                .collect(),
            custom_mirrors: config
                .mirror
                .custom_templates()
                .into_iter()
                .map(str::to_string)
                .collect(),
            video: download.video,
            threads: download.concurrent,
            default_threads: default_threads(),
            skip_already_imported: download.skip_already_imported,
            auto_skip_rate_limited: download.auto_skip_rate_limited,
            rate_limit_secs: download.rate_limit_skip_secs,
            archive_validation: download.archive_validation,
        }
    }

    /// Write these options into `config`, refusing what the downloader can't
    /// run with (no mirror on, a bad custom URL, a thread count out of range).
    fn apply_to(&self, config: &mut Config) -> Result<()> {
        if let Some(threads) = self.threads
            && !(1..=MAX_THREADS).contains(&threads)
        {
            return Err(AppError::config("maps at once must be between 1 and 100"));
        }

        let mut order = Vec::with_capacity(self.mirrors.len());
        for setting in &self.mirrors {
            let kind = MirrorKind::BUILTINS
                .iter()
                .copied()
                .find(|kind| kind.host() == setting.host)
                .ok_or_else(|| {
                    AppError::config_dynamic(format!("unknown mirror: {}", setting.host))
                })?;
            let flag = mirror_flag_mut(&mut config.mirror, kind).ok_or_else(|| {
                AppError::config_dynamic(format!("{} can't be set here", setting.host))
            })?;
            *flag = setting.enabled;
            order.push(kind);
        }
        // Only an order that differs from the built-in one is written, as the
        // TUI does; mirrors left out keep their place after the listed ones.
        let canonical: Vec<MirrorKind> = MirrorKind::BUILTINS
            .iter()
            .copied()
            .filter(|kind| order.contains(kind))
            .collect();
        config.mirror.order = if order == canonical {
            Vec::new()
        } else {
            order.iter().map(|kind| kind.host().into()).collect()
        };
        config.mirror.urls = self
            .custom_mirrors
            .iter()
            .map(|url| url.trim())
            .filter(|url| !url.is_empty())
            .map(Into::into)
            .collect();
        config.mirror.url = None;

        let download = &mut config.download;
        download.video = self.video;
        download.concurrent = self.threads;
        download.skip_already_imported = self.skip_already_imported;
        download.auto_skip_rate_limited = self.auto_skip_rate_limited;
        download.rate_limit_skip_secs = self.rate_limit_secs.max(1);
        download.archive_validation = self.archive_validation;

        config.validate()
    }
}

pub fn load_download_options() -> DownloadOptions {
    DownloadOptions::from_config(&load_config_or_default())
}

/// Validate and save `options`; on an error nothing is written.
pub fn save_download_options(options: &DownloadOptions) -> Result<()> {
    let mut config = load_config_or_default();
    options.apply_to(&mut config)?;
    save_config(&config).map(|_| ())
}

/// The mirror's switch in `[mirror]`, or `None` for one the GUI doesn't offer.
fn mirror_flag(mirror: &MirrorConfig, kind: MirrorKind) -> Option<&bool> {
    Some(match kind {
        MirrorKind::Nerinyan => &mirror.nerinyan,
        MirrorKind::OsuDirect => &mirror.osu_direct,
        MirrorKind::Sayobot => &mirror.sayobot,
        MirrorKind::Nekoha => &mirror.nekoha,
        MirrorKind::Beatconnect => &mirror.beatconnect,
        MirrorKind::Osudl => &mirror.osudl,
        MirrorKind::Catboy => &mirror.catboy,
        MirrorKind::Hinamizawa => &mirror.hinamizawa,
        MirrorKind::Nzbasic => &mirror.nzbasic,
        MirrorKind::OsuApi | MirrorKind::Custom => return None,
    })
}

fn mirror_flag_mut(mirror: &mut MirrorConfig, kind: MirrorKind) -> Option<&mut bool> {
    Some(match kind {
        MirrorKind::Nerinyan => &mut mirror.nerinyan,
        MirrorKind::OsuDirect => &mut mirror.osu_direct,
        MirrorKind::Sayobot => &mut mirror.sayobot,
        MirrorKind::Nekoha => &mut mirror.nekoha,
        MirrorKind::Beatconnect => &mut mirror.beatconnect,
        MirrorKind::Osudl => &mut mirror.osudl,
        MirrorKind::Catboy => &mut mirror.catboy,
        MirrorKind::Hinamizawa => &mut mirror.hinamizawa,
        MirrorKind::Nzbasic => &mut mirror.nzbasic,
        MirrorKind::OsuApi | MirrorKind::Custom => return None,
    })
}

fn mirror_name(kind: MirrorKind) -> &'static str {
    match kind {
        MirrorKind::OsuDirect => "osu!direct",
        MirrorKind::Nerinyan => "Nerinyan",
        MirrorKind::Sayobot => "Sayobot",
        MirrorKind::Nekoha => "Nekoha",
        MirrorKind::Beatconnect => "Beatconnect",
        MirrorKind::Osudl => "osu!dl",
        MirrorKind::Catboy => "catboy.best",
        MirrorKind::Hinamizawa => "Hinamizawa",
        MirrorKind::Nzbasic => "nzbasic",
        MirrorKind::OsuApi => "osu! official",
        MirrorKind::Custom => "custom",
    }
}

/// Facts from the mirror config's own docs, for the mirrors that are off by
/// default or only cover part of osu!.
fn mirror_note(kind: MirrorKind) -> Option<&'static str> {
    Some(match kind {
        MirrorKind::Osudl => "Only has ranked, approved and loved maps.",
        MirrorKind::Hinamizawa => {
            "Tries the other mirrors itself, so it's usually not needed alongside them."
        }
        MirrorKind::Nzbasic => "A free server run by one person; only has the maps it has cached.",
        _ => return None,
    })
}

/// Client id of the saved osu! API app, or `None` when none is saved. An
/// older user login isn't an API app, so it reads as none.
pub fn saved_api_app() -> Option<String> {
    auth::load()
        .filter(|stored| !stored.is_user_login())
        .map(|stored| stored.client_id)
}

/// Check the user's own osu! API app with osu! and save it; returns the client
/// id as saved. Nothing is saved when osu! refuses it.
pub async fn save_api_app(client_id: &str, client_secret: &str) -> Result<String> {
    auth::save_app_credentials(&reqwest::Client::new(), client_id, client_secret).await?;
    Ok(client_id.trim().to_string())
}

/// Forget the saved osu! API app.
pub fn remove_api_app() -> Result<()> {
    auth::delete()
}

#[cfg(test)]
#[path = "../../tests/unit/engine_options.rs"]
mod tests;
