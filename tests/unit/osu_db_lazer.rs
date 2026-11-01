use super::{find_lazer_install, find_lazer_installs};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// A lazer data folder whose `client.realm` was last changed `age_secs` ago.
fn install(root: &Path, name: &str, age_secs: u64) -> PathBuf {
    let folder = root.join(name);
    fs::create_dir_all(&folder).unwrap();
    let realm = folder.join("client.realm");
    fs::write(&realm, b"realm").unwrap();
    File::options()
        .write(true)
        .open(&realm)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
    folder
}

fn redirect(folder: &Path, target: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(folder.join("storage.ini"), format!("FullPath = {target}\n")).unwrap();
}

#[test]
fn the_most_recently_changed_realm_wins() {
    let root = tempfile::tempdir().unwrap();
    let official = install(root.path(), "osu", 3600);
    let torii = install(root.path(), "osu-torii", 60);

    assert_eq!(
        find_lazer_install([official.clone(), torii.clone()]),
        Some(torii.clone())
    );
    assert_eq!(find_lazer_install([torii.clone(), official]), Some(torii));
}

#[test]
fn every_detected_install_is_listed_newest_first() {
    let root = tempfile::tempdir().unwrap();
    let official = install(root.path(), "osu", 3600);
    let torii = install(root.path(), "osu-torii", 60);

    assert_eq!(
        find_lazer_installs([official.clone(), torii.clone()]),
        vec![torii, official]
    );
}

#[test]
fn folders_without_a_realm_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("osu");
    fs::create_dir_all(&empty).unwrap();
    let torii = install(root.path(), "osu-torii", 60);

    assert_eq!(
        find_lazer_install([empty, root.path().join("missing"), torii.clone()]),
        Some(torii)
    );
    assert_eq!(find_lazer_install([root.path().join("missing")]), None);
}

#[test]
fn storage_ini_redirects_to_the_moved_folder() {
    let root = tempfile::tempdir().unwrap();
    // The leftover realm in the default folder is newer than the real one,
    // but lazer reads the folder storage.ini points at.
    let moved = install(root.path(), "moved", 3600);
    let default = install(root.path(), "osu", 60);
    redirect(&default, moved.to_str().unwrap());

    assert_eq!(find_lazer_install([default]), Some(moved));
}

#[test]
fn a_redirect_to_a_missing_or_relative_folder_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let default = install(root.path(), "osu", 60);

    redirect(&default, root.path().join("gone").to_str().unwrap());
    assert_eq!(find_lazer_install([default.clone()]), Some(default.clone()));

    redirect(&default, "moved");
    assert_eq!(find_lazer_install([default.clone()]), Some(default));
}

#[test]
fn a_folder_reached_twice_is_found_once() {
    let root = tempfile::tempdir().unwrap();
    let torii = install(root.path(), "osu-torii", 60);
    let official = root.path().join("osu");
    redirect(&official, torii.to_str().unwrap());

    assert_eq!(
        find_lazer_installs([official.clone(), torii.clone()]),
        vec![torii.clone()]
    );
    assert_eq!(find_lazer_install([official, torii.clone()]), Some(torii));
}
