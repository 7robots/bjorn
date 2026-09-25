//! Shared fixtures: a fake bearcli with its own state file, and a harness.
#![allow(dead_code)]

use std::sync::Arc;

use bjorn::bear::BearClient;
use bjorn::config::Config;
use bjorn::harness::Harness;

pub struct Fake {
    pub dir: tempfile::TempDir,
    /// The day the fake's dated sections are seeded from. Read once here and
    /// handed to the fake, so a test and the library it reads always agree on
    /// which day "today" is, even across midnight.
    pub today: chrono::NaiveDate,
}

impl Fake {
    pub fn new() -> Fake {
        // One date for the whole test process: the app's clock, the fake
        // library's seed and the assertions all read it, so a suite that
        // starts a moment before midnight cannot disagree with itself.
        let today = bjorn::model::pin_today_once(chrono::Local::now().date_naive());
        Fake {
            dir: tempfile::tempdir().unwrap(),
            today,
        }
    }

    /// `today`, `n` days back.
    pub fn days_ago(&self, n: i64) -> chrono::NaiveDate {
        self.today - chrono::Duration::days(n)
    }

    pub fn templates(&self) -> std::path::PathBuf {
        self.dir.path().join("templates")
    }

    pub fn state(&self) -> std::path::PathBuf {
        self.dir.path().join("bear.json")
    }

    pub fn client(&self) -> BearClient {
        BearClient::with_env(
            vec![env!("CARGO_BIN_EXE_fake-bearcli").to_string()],
            vec![
                (
                    "BJORN_FAKE_BEAR_STATE".to_string(),
                    self.state().to_string_lossy().into_owned(),
                ),
                (
                    "BJORN_FAKE_BEAR_TODAY".to_string(),
                    self.today.format("%Y-%m-%d").to_string(),
                ),
            ],
        )
    }

    /// A config whose file lives in the fake's own folder. Left as `None`, the
    /// path resolves to the user's real `~/.config/bjorn/config.toml`, which
    /// any test that saves an action would then write to.
    pub fn config(&self) -> Config {
        Config {
            poll_seconds: 0,
            export_dir: self.dir.path().join("exports"),
            icon_style: "none".into(),
            path: Some(self.dir.path().join("config.toml")),
            // Never the real ~/.config/bjorn/templates.
            templates_dir: self.dir.path().join("templates"),
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
