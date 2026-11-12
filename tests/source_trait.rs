use std::path::Path;

use async_trait::async_trait;

use maxfetch::config::{Config, Credentials};
use maxfetch::fetch::ProgressSink;
use maxfetch::model::{Candidate, Release, SourceId};
use maxfetch::sources::{AuthState, Capabilities, Query, Source, enabled_in, is_enabled, registry};

#[test]
fn registry_is_empty_before_sources_are_added() {
    let sources = registry(&Config::default(), &Credentials::default());
    assert!(sources.is_empty());
}

#[test]
fn enabled_in_follows_the_configured_list() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["soulseek".into()];

    assert_eq!(
        enabled_in(&[SourceId::Qobuz, SourceId::Soulseek], &cfg),
        vec![SourceId::Soulseek]
    );
    assert!(enabled_in(&[], &cfg).is_empty());
}

#[test]
fn enabled_in_preserves_input_order() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["youtube".into(), "soulseek".into()];

    assert_eq!(
        enabled_in(&[SourceId::Soulseek, SourceId::Youtube], &cfg),
        vec![SourceId::Soulseek, SourceId::Youtube]
    );
}

#[test]
fn is_enabled_matches_the_source_id_string() {
    let cfg = Config::default();

    assert!(is_enabled(&SourceId::Soulseek, &cfg));
    assert!(!is_enabled(&SourceId::Qobuz, &cfg));
}

#[test]
fn unknown_name_in_config_is_ignored() {
    let mut cfg = Config::default();
    cfg.sources.enabled = vec!["myspace".into()];

    assert!(enabled_in(&[SourceId::Soulseek], &cfg).is_empty());
}

struct Fake;

#[async_trait]
impl Source for Fake {
    fn id(&self) -> SourceId {
        SourceId::Soulseek
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            audio: false,
            metadata_only: false,
            lossless: false,
            needs_auth: false,
        }
    }

    fn auth_state(&self) -> AuthState {
        AuthState::NotNeeded
    }

    async fn search(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(vec![])
    }

    async fn resolve(&self, _q: &Query) -> anyhow::Result<Vec<Release>> {
        Ok(vec![])
    }

    async fn candidates(&self, _release: &Release) -> anyhow::Result<Vec<Candidate>> {
        Ok(vec![])
    }

    async fn download(
        &self,
        _candidate: &Candidate,
        _dest: &Path,
        _sink: &dyn ProgressSink,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn a_fake_source_is_object_safe() {
    let sources: Vec<Box<dyn Source>> = vec![Box::new(Fake)];
    assert_eq!(sources[0].id(), SourceId::Soulseek);
}