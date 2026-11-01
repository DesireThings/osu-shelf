use super::{DownloadOptions, MirrorSetting, saved_api_app};
use crate::auth::{AUTH_ENV_PATH, StoredAuth};
use crate::config::Config;
use crate::test_env::TempEnvVar;
use osu_downloader::{ArchiveValidation, MirrorKind};

fn hosts(options: &DownloadOptions) -> Vec<&str> {
    options.mirrors.iter().map(|m| m.host.as_str()).collect()
}

fn setting<'a>(options: &'a mut DownloadOptions, host: &str) -> &'a mut MirrorSetting {
    options.mirrors.iter_mut().find(|m| m.host == host).unwrap()
}

#[test]
fn the_default_config_lists_every_offered_mirror_in_try_order() {
    let options = DownloadOptions::from_config(&Config::default());
    let expected: Vec<&str> = MirrorKind::BUILTINS
        .iter()
        .filter(|kind| **kind != MirrorKind::OsuApi)
        .map(|kind| kind.host())
        .collect();
    assert_eq!(hosts(&options), expected);
    assert!(
        !hosts(&options).contains(&MirrorKind::OsuApi.host()),
        "osu! official needs a login this app can't make"
    );
    let nzbasic = options
        .mirrors
        .iter()
        .find(|m| m.host == "direct.nzbasic.com")
        .unwrap();
    assert!(!nzbasic.enabled, "off by default");
    assert!(nzbasic.note.is_some());
    assert_eq!(nzbasic.limits, None, "publishes no limits");
    let osu_direct = options
        .mirrors
        .iter()
        .find(|m| m.host == "osu.direct")
        .unwrap();
    assert_eq!(osu_direct.limits.as_deref(), Some("120 req/min, 8 GiB/day"));
    assert_eq!(options.threads, None);
    assert!(options.default_threads >= 1);
}

#[test]
fn options_round_trip_through_the_config() {
    let mut options = DownloadOptions::from_config(&Config::default());
    options.mirrors.swap(0, 2);
    setting(&mut options, "catboy.best").enabled = false;
    setting(&mut options, "mirror.hinamizawa.ai").enabled = true;
    options.custom_mirrors = vec!["  https://example.com/d/{id}  ".into(), "   ".into()];
    options.video = false;
    options.threads = Some(8);
    options.skip_already_imported = false;
    options.auto_skip_rate_limited = false;
    options.rate_limit_secs = 90;
    options.archive_validation = ArchiveValidation::Eocd;

    let mut config = Config::default();
    options.apply_to(&mut config).unwrap();
    assert!(!config.mirror.catboy);
    assert!(config.mirror.hinamizawa);
    assert_eq!(
        config.mirror.urls,
        vec!["https://example.com/d/{id}".into()]
    );
    assert_eq!(config.download.concurrent, Some(8));

    let back = DownloadOptions::from_config(&config);
    assert_eq!(hosts(&back), hosts(&options), "the order survives");
    assert_eq!(back.custom_mirrors, ["https://example.com/d/{id}"]);
    let mut expected = options.clone();
    expected.custom_mirrors = back.custom_mirrors.clone();
    assert_eq!(back, expected);
}

#[test]
fn an_unchanged_order_writes_no_order_key() {
    let options = DownloadOptions::from_config(&Config::default());
    let mut config = Config::default();
    options.apply_to(&mut config).unwrap();
    assert!(config.mirror.order.is_empty());
}

#[test]
fn turning_every_mirror_off_is_refused() {
    let mut options = DownloadOptions::from_config(&Config::default());
    for mirror in &mut options.mirrors {
        mirror.enabled = false;
    }
    let err = options.apply_to(&mut Config::default()).unwrap_err();
    assert!(err.to_string().contains("at least one mirror"), "{err}");

    // A custom mirror on its own is enough.
    options.custom_mirrors = vec!["https://example.com/d/{id}".into()];
    options.apply_to(&mut Config::default()).unwrap();
}

#[test]
fn a_custom_mirror_without_an_id_slot_is_refused() {
    let mut options = DownloadOptions::from_config(&Config::default());
    options.custom_mirrors = vec!["https://example.com/download".into()];
    assert!(options.apply_to(&mut Config::default()).is_err());
}

#[test]
fn thread_counts_outside_1_to_100_are_refused() {
    let mut options = DownloadOptions::from_config(&Config::default());
    for bad in [0, 101] {
        options.threads = Some(bad);
        let err = options.apply_to(&mut Config::default()).unwrap_err();
        assert!(
            err.to_string().contains("between 1 and 100"),
            "{bad}: {err}"
        );
    }
}

#[test]
fn an_unknown_or_hidden_mirror_is_refused() {
    let mut options = DownloadOptions::from_config(&Config::default());
    options.mirrors[0].host = "example.org".into();
    assert!(options.apply_to(&mut Config::default()).is_err());
    options.mirrors[0].host = MirrorKind::OsuApi.host().into();
    assert!(options.apply_to(&mut Config::default()).is_err());
}

#[test]
fn a_zero_second_rate_limit_wait_is_floored_at_one() {
    let mut options = DownloadOptions::from_config(&Config::default());
    options.rate_limit_secs = 0;
    let mut config = Config::default();
    options.apply_to(&mut config).unwrap();
    assert_eq!(config.download.rate_limit_skip_secs, 1);
}

fn stored(refresh_token: Option<&str>) -> StoredAuth {
    StoredAuth {
        client_id: "12345".into(),
        client_secret: "secret".into(),
        redirect_uri: String::new(),
        access_token: "tok".into(),
        refresh_token: refresh_token.map(Into::into),
        expires_at: u64::MAX,
        scopes: vec!["public".into()],
        supporter: None,
    }
}

#[test]
fn only_an_api_app_reads_as_the_saved_app() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let _auth = TempEnvVar::set(AUTH_ENV_PATH, path.to_str().unwrap());
    assert_eq!(saved_api_app(), None, "nothing saved");

    std::fs::write(&path, serde_json::to_string(&stored(None)).unwrap()).unwrap();
    assert_eq!(saved_api_app().as_deref(), Some("12345"));

    std::fs::write(&path, serde_json::to_string(&stored(Some("rt"))).unwrap()).unwrap();
    assert_eq!(
        saved_api_app(),
        None,
        "an older user login isn't an API app"
    );
}
