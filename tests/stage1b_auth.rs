use maxfetch::auth::{auth_status, login, stage1b_login_hint};
use maxfetch::config::{Config, Credentials};
use maxfetch::model::SourceId;
use maxfetch::sources::{enabled_in, registry};

#[test]
fn stage1b_sources_are_registered_but_disabled_by_the_default_config() {
    let cfg = Config::default();
    let all = registry(&cfg, &Credentials::default());
    let ids: Vec<SourceId> = all.iter().map(|s| s.id()).collect();
    // `registry` is a pure constructor: every implemented source is present,
    // whatever `config.sources.enabled` says.
    assert!(ids.contains(&SourceId::Bandcamp));
    assert!(ids.contains(&SourceId::SoundCloud));
    // `enabled_in` is the only filter; with the default config SoundCloud is off.
    let enabled = enabled_in(&ids, &cfg);
    assert!(enabled.contains(&SourceId::Bandcamp));
    assert!(!enabled.contains(&SourceId::SoundCloud));
}

#[test]
fn an_enabled_soundcloud_is_registered() {
    let mut cfg = Config::default();
    cfg.sources.enabled.push("soundcloud".into());
    let ids: Vec<SourceId> = registry(&cfg, &Credentials::default())
        .iter()
        .map(|s| s.id())
        .collect();
    assert!(ids.contains(&SourceId::Bandcamp));
    assert!(ids.contains(&SourceId::SoundCloud));
    let enabled = enabled_in(&ids, &cfg);
    assert!(enabled.contains(&SourceId::Bandcamp));
    assert!(enabled.contains(&SourceId::SoundCloud));
}

#[test]
fn neither_source_needs_credentials_in_stage1b() {
    let statuses = auth_status(&Credentials::default());
    let bandcamp = statuses
        .iter()
        .find(|s| s.id == SourceId::Bandcamp)
        .unwrap();
    let soundcloud = statuses
        .iter()
        .find(|s| s.id == SourceId::SoundCloud)
        .unwrap();
    assert_eq!(bandcamp.auth, maxfetch::sources::AuthState::NotNeeded);
    assert_eq!(soundcloud.auth, maxfetch::sources::AuthState::NotNeeded);
    assert_eq!(
        bandcamp.detail.as_deref(),
        Some("без учётной записи: только бесплатное промо")
    );
    assert_eq!(
        soundcloud.detail.as_deref(),
        Some("без учётной записи: только AAC 160k")
    );
}

#[tokio::test]
async fn the_login_hint_explains_the_stage_two_limit() {
    assert!(stage1b_login_hint("bandcamp").unwrap().contains("этапе 2"));
    assert!(
        stage1b_login_hint("soundcloud")
            .unwrap()
            .contains("AAC 160k")
    );
    let mut creds = Credentials::default();
    let err = login("soundcloud", &mut creds).await.unwrap_err();
    assert!(format!("{err}").contains("этапе 2"));
}
