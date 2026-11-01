use super::{BeatmapReader, LocalBeatmap, LocalBeatmapset, LocalCollection, checksum, require_db};
use crate::realm_bridge::ffi;
use std::{fs, path::PathBuf, time::SystemTime};
use tracing::{debug, info, warn};

pub struct LazerReader {
    path: PathBuf,
}

impl LazerReader {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn realm_path(&self) -> Result<PathBuf, String> {
        require_db(&self.path, "client.realm")
    }
}

impl BeatmapReader for LazerReader {
    fn list_beatmapsets(&self) -> Result<Vec<LocalBeatmapset>, String> {
        let db_path = self.realm_path()?;
        let db_path_str = db_path.to_str().ok_or("Invalid path encoding")?;

        let realm =
            ffi::open_realm(db_path_str).map_err(|e| format!("Failed to open realm: {e}"))?;

        let ffi_sets = realm.list_beatmapsets();

        let sets = ffi_sets
            .into_iter()
            .map(|s| LocalBeatmapset {
                id: s.id,
                beatmaps: s
                    .beatmaps
                    .into_iter()
                    .map(|b| LocalBeatmap {
                        checksum: checksum::parse_hex(&b.checksum).unwrap_or_else(|| {
                            if !b.checksum.is_empty() {
                                warn!(hash = %b.checksum, "malformed beatmap checksum from realm");
                            }
                            checksum::EMPTY
                        }),
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            })
            .collect();

        Ok(sets)
    }

    fn list_collections(&self) -> Result<Vec<LocalCollection>, String> {
        let db_path = self.realm_path()?;
        info!(path = %db_path.display(), "Reading collections from Realm database");

        let db_path_str = db_path.to_str().ok_or("Invalid path encoding")?;

        let realm =
            ffi::open_realm(db_path_str).map_err(|e| format!("Failed to open realm: {e}"))?;

        debug!("Realm database opened successfully");

        let ffi_collections = realm.list_collections();
        info!(
            count = ffi_collections.len(),
            "Retrieved collections from Realm"
        );

        for (i, c) in ffi_collections.iter().enumerate() {
            debug!(
                index = i,
                name = %c.name,
                beatmap_count = c.beatmap_checksums.len(),
                "Collection from Realm"
            );
        }

        let collections = ffi_collections
            .into_iter()
            .map(|c| LocalCollection {
                name: c.name,
                beatmap_checksums: c
                    .beatmap_checksums
                    .into_iter()
                    .map(|h| {
                        checksum::parse_hex(&h).unwrap_or_else(|| {
                            if !h.is_empty() {
                                warn!(hash = %h, "malformed collection checksum from realm");
                            }
                            checksum::EMPTY
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            })
            .collect();

        Ok(collections)
    }

    fn default_path() -> Option<PathBuf> {
        find_lazer_install(Self::candidate_paths())
    }
}

/// Every lazer data folder found in `candidates`, newest `client.realm` first,
/// so a leftover realm or an install that isn't played any more doesn't beat
/// the one in use. Each one is first followed through its `storage.ini`
/// redirect; a folder reached twice is listed once. Ties keep the earlier
/// candidate.
fn find_lazer_installs(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for candidate in candidates {
        let folder = follow_storage_redirect(candidate);
        let Ok(meta) = fs::metadata(folder.join("client.realm")) else {
            continue;
        };
        if found.iter().any(|(_, path)| *path == folder) {
            continue;
        }
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((modified, folder));
    }
    // Stable sort: equal mtimes keep the earlier candidate's order.
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, folder)| folder).collect()
}

/// Picks lazer's data folder from `candidates`: the newest one.
fn find_lazer_install(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    find_lazer_installs(candidates).into_iter().next()
}

/// The folder lazer actually uses for `folder`: moving the data folder in
/// lazer's settings writes `FullPath = <new folder>` to `storage.ini` in the
/// default one. Like lazer, a redirect to a folder that doesn't exist is
/// ignored.
fn follow_storage_redirect(folder: PathBuf) -> PathBuf {
    let Ok(ini) = fs::read_to_string(folder.join("storage.ini")) else {
        return folder;
    };
    let target = ini.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("FullPath")
            .then(|| PathBuf::from(value.trim()))
    });
    match target {
        Some(target) if target.is_absolute() && target.is_dir() => target,
        _ => folder,
    }
}

impl LazerReader {
    /// Every lazer data folder found in the default locations, newest first.
    pub fn detected_paths() -> Vec<PathBuf> {
        find_lazer_installs(Self::candidate_paths())
    }

    fn candidate_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        #[cfg(target_os = "windows")]
        {
            // %appdata%\osu (default user data directory)
            if let Some(data) = dirs::data_dir() {
                paths.push(data.join("osu"));
            }
            // %appdata%\osu-torii (Torii)
            if let Some(data) = dirs::data_dir() {
                paths.push(data.join("osu-torii"));
            }
        }

        #[cfg(target_os = "linux")]
        {
            // ~/.local/share/osu (default)
            if let Some(data) = dirs::data_local_dir() {
                paths.push(data.join("osu"));
            }
            // ~/.local/share/osu-lazer (alternative naming)
            if let Some(data) = dirs::data_local_dir() {
                paths.push(data.join("osu-lazer"));
            }
            // ~/.local/share/osu-torii (Torii)
            if let Some(data) = dirs::data_local_dir() {
                paths.push(data.join("osu-torii"));
            }
        }

        #[cfg(target_os = "macos")]
        {
            // ~/Library/Application Support/osu
            if let Some(data) = dirs::data_dir() {
                paths.push(data.join("osu"));
            }
            // ~/Library/Application Support/osu-torii (Torii)
            if let Some(data) = dirs::data_dir() {
                paths.push(data.join("osu-torii"));
            }
        }

        paths
    }

    pub fn list_all_checksums(&self) -> Result<Vec<checksum::Md5>, String> {
        let db_path = self.realm_path()?;
        let db_path_str = db_path.to_str().ok_or("Invalid path encoding")?;

        let realm =
            ffi::open_realm(db_path_str).map_err(|e| format!("Failed to open realm: {e}"))?;

        Ok(realm
            .list_all_checksums()
            .into_iter()
            .map(|h| {
                checksum::parse_hex(&h).unwrap_or_else(|| {
                    if !h.is_empty() {
                        warn!(hash = %h, "malformed checksum from realm");
                    }
                    checksum::EMPTY
                })
            })
            .collect())
    }

    #[allow(clippy::type_complexity)]
    pub fn read_all(
        &self,
    ) -> Result<
        (
            Vec<LocalCollection>,
            Vec<LocalBeatmapset>,
            Vec<checksum::Md5>,
        ),
        String,
    > {
        let db_path = self.realm_path()?;
        info!(path = %db_path.display(), "Reading all data from Realm database");

        let db_path_str = db_path.to_str().ok_or("Invalid path encoding")?;

        let realm =
            ffi::open_realm(db_path_str).map_err(|e| format!("Failed to open realm: {e}"))?;

        debug!("Realm database opened successfully");

        let ffi_collections = realm.list_collections();
        info!(
            count = ffi_collections.len(),
            "Retrieved collections from Realm"
        );

        let ffi_sets = realm.list_beatmapsets();
        let all_checksums: Vec<checksum::Md5> = realm
            .list_all_checksums()
            .into_iter()
            .map(|h| {
                checksum::parse_hex(&h).unwrap_or_else(|| {
                    if !h.is_empty() {
                        warn!(hash = %h, "malformed checksum from realm");
                    }
                    checksum::EMPTY
                })
            })
            .collect();

        let collections = ffi_collections
            .into_iter()
            .map(|c| LocalCollection {
                name: c.name,
                beatmap_checksums: c
                    .beatmap_checksums
                    .into_iter()
                    .map(|h| {
                        checksum::parse_hex(&h).unwrap_or_else(|| {
                            if !h.is_empty() {
                                warn!(hash = %h, "malformed collection checksum from realm");
                            }
                            checksum::EMPTY
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            })
            .collect();

        let beatmapsets = ffi_sets
            .into_iter()
            .map(|s| LocalBeatmapset {
                id: s.id,
                beatmaps: s
                    .beatmaps
                    .into_iter()
                    .map(|b| LocalBeatmap {
                        checksum: checksum::parse_hex(&b.checksum).unwrap_or_else(|| {
                            if !b.checksum.is_empty() {
                                warn!(hash = %b.checksum, "malformed beatmap checksum from realm");
                            }
                            checksum::EMPTY
                        }),
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            })
            .collect();

        Ok((collections, beatmapsets, all_checksums))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/osu_db_lazer.rs"]
mod tests;
