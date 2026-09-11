//! Shared fixtures: a fake bearcli with its own state file, and a harness.
#![allow(dead_code)]

use std::sync::Arc;

use bjorn::bear::BearClient;
use bjorn::config::Config;
use bjorn::harness::Harness;

pub struct Fake {
    pub dir: tempfile::TempDir,
}

impl Fake {
    pub fn new() -> Fake {
        Fake {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn state(&self) -> std::path::PathBuf {
        self.dir.path().join("bear.json")
    }

    pub fn client(&self) -> BearClient {
        BearClient::with_env(
            vec![env!("CARGO_BIN_EXE_fake-bearcli").to_string()],
            vec![(
                "BJORN_FAKE_BEAR_STATE".to_string(),
                self.state().to_string_lossy().into_owned(),
            )],
        )
    }

    pub fn config(&self) -> Config {
        Config {
            poll_seconds: 0,
            export_dir: self.dir.path().join("exports"),
            icon_style: "none".into(),
            ..Config::default()
        }
    }

    pub fn harness(&self) -> Harness {
        Harness::new(self.config(), Arc::new(self.client()), None, (120, 40))
    }

    pub fn harness_with(&self, config: Config, workspace: Option<&str>) -> Harness {
        Harness::new(config, Arc::new(self.client()), workspace, (120, 40))
    }

    pub fn harness_env(&self, environ: std::collections::HashMap<String, String>) -> Harness {
        Harness::with_env(
            self.config(),
            Arc::new(self.client()),
            None,
            (120, 40),
            environ,
        )
    }
}

pub fn titles(h: &Harness) -> Vec<String> {
    h.app.notes.titles()
}
