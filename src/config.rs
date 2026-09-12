//! User configuration: `${XDG_CONFIG_HOME:-~/.config}/bjorn/config.toml`.
//!
//! Every key is optional and the file is shared with the Python Bjorn, so the
//! keys, defaults and leniency match it exactly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use toml::Value;

use crate::actions::{Action, DEFAULT_TIMEOUT_SECONDS};
use crate::util::{expand_tilde, home_dir, which};

pub const APP_NAME: &str = "bjorn";
pub const DEFAULT_EDITOR: &str = "vim";
pub const DEFAULT_POLL_SECONDS: u64 = 5;

pub fn config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
    let base = base.trim();
    let root = if base.is_empty() {
        home_dir().join(".config")
    } else {
        expand_tilde(base)
    };
    root.join(APP_NAME)
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// `${XDG_CACHE_HOME:-~/.cache}/bjorn`: derived state only, safe to delete.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_default();
    let base = base.trim();
    let root = if base.is_empty() {
        home_dir().join(".cache")
    } else {
        expand_tilde(base)
    };
    root.join(APP_NAME)
}

/// Previews kept between runs, so a launch takes the warm path. The Python
/// Bjorn writes the same file in the same shape, so either one warms the other;
/// it is versioned and keyed by the bearcli it came from, and anything that
/// does not match is ignored and overwritten.
pub fn preview_cache_path() -> PathBuf {
    cache_dir().join("previews.json")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemindersConfig {
    pub enabled: bool,
    pub list: String,
    pub due: String,
    pub remctl: String,
}

impl Default for RemindersConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            list: String::new(),
            due: "today".into(),
            remctl: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub editor: String,
    pub export_dir: PathBuf,
    pub export_format: String,
    pub poll_seconds: u64,
    pub workspace: String,
    pub bearcli: String,
    pub icon_style: String,
    pub icons: BTreeMap<String, String>,
    /// Palette name; see `ui::theme::THEMES`. An unknown name falls back to
    /// the default, so a typo never stops the app.
    pub theme: String,
    /// Accepted for compatibility with the Python Bjorn's config file. The
    /// Rust build never negotiates pixel mouse reporting, so it has no effect.
    pub mouse_pixels: bool,
    pub reminders: RemindersConfig,
    /// `[[actions]]` from the config file, in the order they are written.
    pub actions: Vec<Action>,
    pub path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: String::new(),
            export_dir: home_dir().join("Downloads"),
            export_format: "md".into(),
            poll_seconds: DEFAULT_POLL_SECONDS,
            workspace: String::new(),
            bearcli: String::new(),
            icon_style: "auto".into(),
            icons: BTreeMap::new(),
            theme: crate::ui::theme::DEFAULT_THEME.into(),
            mouse_pixels: true,
            reminders: RemindersConfig::default(),
            actions: Vec::new(),
            path: None,
        }
    }
}

/// Python's `str(value or "")`: scalars become text, anything else the default.
fn text(value: Option<&Value>, default: &str) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Integer(0)) | Some(Value::Boolean(false)) | None => default.to_string(),
        Some(Value::Integer(i)) => i.to_string(),
        Some(Value::Float(f)) => f.to_string(),
        Some(Value::Boolean(true)) => "True".to_string(),
        Some(_) => default.to_string(),
    }
}

/// Python's `bool(value)` for the TOML values that can appear.
fn truthy(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(Value::Boolean(b)) => *b,
        Some(Value::Integer(i)) => *i != 0,
        Some(Value::Float(f)) => *f != 0.0,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Table(t)) => !t.is_empty(),
        Some(Value::Datetime(_)) => true,
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Config> {
        let target = match path {
            Some(p) => expand_tilde(&p.to_string_lossy()),
            None => default_config_path(),
        };
        let mut cfg = Config {
            path: Some(target.clone()),
            ..Config::default()
        };
        if !target.exists() {
            return Ok(cfg);
        }
        let raw = std::fs::read_to_string(&target)
            .with_context(|| format!("reading {}", target.display()))?;
        let data: toml::Table = raw
            .parse()
            .with_context(|| format!("parsing {}", target.display()))?;

        cfg.editor = text(data.get("editor"), "").trim().to_string();
        let export_dir = text(data.get("export_dir"), "").trim().to_string();
        if !export_dir.is_empty() {
            cfg.export_dir = expand_tilde(&export_dir);
        }
        cfg.export_format = text(data.get("export_format"), "md").trim().to_lowercase();
        cfg.poll_seconds = match data.get("poll_seconds") {
            None => DEFAULT_POLL_SECONDS,
            Some(Value::Integer(i)) => (*i).max(0) as u64,
            Some(Value::Float(f)) => f.trunc().max(0.0) as u64,
            Some(Value::Boolean(b)) => u64::from(*b),
            Some(Value::String(s)) => s
                .trim()
                .parse::<i64>()
                .map(|i| i.max(0) as u64)
                .unwrap_or(DEFAULT_POLL_SECONDS),
            Some(_) => DEFAULT_POLL_SECONDS,
        };
        cfg.workspace = text(data.get("workspace"), "")
            .trim()
            .trim_matches('#')
            .to_string();
        cfg.bearcli = text(data.get("bearcli"), "").trim().to_string();
        cfg.icon_style = text(data.get("icon_style"), "auto").trim().to_lowercase();
        if let Some(Value::Table(icons)) = data.get("icons") {
            cfg.icons = icons
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
        }
        cfg.theme = text(data.get("theme"), crate::ui::theme::DEFAULT_THEME)
            .trim()
            .to_lowercase();
        if cfg.theme.is_empty() {
            cfg.theme = crate::ui::theme::DEFAULT_THEME.into();
        }
        cfg.mouse_pixels = truthy(data.get("mouse_pixels"), true);
        if let Some(Value::Table(section)) = data.get("reminders") {
            cfg.reminders = RemindersConfig {
                enabled: truthy(section.get("enabled"), false),
                list: text(section.get("list"), "").trim().to_string(),
                due: match section.get("due") {
                    None => "today".to_string(),
                    Some(Value::String(s)) => s.trim().to_string(),
                    Some(other) => other.to_string().trim().to_string(),
                },
                remctl: text(section.get("remctl"), "").trim().to_string(),
            };
        }
        cfg.actions = parse_actions(data.get("actions"));
        Ok(cfg)
    }
}

/// `[[actions]]`: a name and a shell command, plus the optional `format`,
/// `confirm`, `timeout` and `default`. An entry without a name or a command is
/// dropped rather than raised, so a half-written action never stops the app.
fn parse_actions(value: Option<&Value>) -> Vec<Action> {
    let Some(Value::Array(entries)) = value else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    for entry in entries {
        let Value::Table(entry) = entry else { continue };
        let command = text(entry.get("command"), "").trim().to_string();
        if command.is_empty() {
            continue;
        }
        let name = text(entry.get("name"), "").trim().to_string();
        let format = text(entry.get("format"), crate::export::DEFAULT_FORMAT)
            .trim()
            .to_lowercase();
        let timeout = match entry.get("timeout") {
            None => DEFAULT_TIMEOUT_SECONDS,
            Some(Value::Integer(i)) => (*i).max(1) as u64,
            Some(Value::Float(f)) => f.trunc().max(1.0) as u64,
            Some(Value::String(s)) => s
                .trim()
                .parse::<i64>()
                .map(|i| i.max(1) as u64)
                .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
            Some(_) => DEFAULT_TIMEOUT_SECONDS,
        };
        actions.push(Action {
            name: if name.is_empty() {
                command.clone()
            } else {
                name
            },
            command,
            // An unknown format falls back to Markdown, as `export_format` does.
            format: crate::export::format_by_id(&format).id.to_string(),
            confirm: truthy(entry.get("confirm"), false),
            timeout: std::time::Duration::from_secs(timeout),
            default: truthy(entry.get("default"), false),
        });
    }
    actions
}

/// Config `editor`, then `$VISUAL`, then `$EDITOR`, then `vim`.
pub fn resolve_editor(config: &Config, environ: &dyn Fn(&str) -> Option<String>) -> String {
    let candidates = [
        Some(config.editor.clone()),
        environ("VISUAL"),
        environ("EDITOR"),
    ];
    for candidate in candidates.into_iter().flatten() {
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_EDITOR.to_string()
}

/// The process environment as the lookup `resolve_editor` wants.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The first word of the editor command must be runnable.
pub fn editor_available(command: &str) -> bool {
    let Some(head) = command.split_whitespace().next() else {
        return false;
    };
    let path = expand_tilde(head);
    if path.is_absolute() {
        return crate::util::is_executable(&path);
    }
    which(head).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn missing_config_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(Some(&dir.path().join("nope.toml"))).unwrap();
        assert_eq!(cfg.editor, "");
        assert_eq!(cfg.poll_seconds, 5);
        assert_eq!(cfg.workspace, "");
        assert_eq!(cfg.export_dir, home_dir().join("Downloads"));
        assert_eq!(cfg.theme, crate::ui::theme::DEFAULT_THEME);
    }

    #[test]
    fn theme_is_read_and_normalised() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "theme = \"Red-Graphite\"\n");
        assert_eq!(Config::load(Some(&path)).unwrap().theme, "red-graphite");
        let path = write(&dir, "theme = \"\"\n");
        assert_eq!(
            Config::load(Some(&path)).unwrap().theme,
            crate::ui::theme::DEFAULT_THEME
        );
    }

    #[test]
    fn config_values_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "editor = \"nvim\"\nexport_dir = \"~/exports\"\npoll_seconds = 0\nworkspace = \"#work\"\nbearcli = \"/opt/bearcli\"\n\
             icon_style = \"Nerd\"\n[icons]\ntech = \"terminal\"\nschool = \"emoji:🎓\"\nbad = 3\n",
        );
        let cfg = Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.editor, "nvim");
        assert_eq!(cfg.export_dir, home_dir().join("exports"));
        assert_eq!(cfg.poll_seconds, 0);
        assert_eq!(cfg.workspace, "work");
        assert_eq!(cfg.bearcli, "/opt/bearcli");
        assert_eq!(cfg.icon_style, "nerd");
        assert_eq!(cfg.theme, crate::ui::theme::DEFAULT_THEME);
        assert_eq!(cfg.icons.get("tech").unwrap(), "terminal");
        assert_eq!(cfg.icons.get("school").unwrap(), "emoji:🎓");
        assert!(!cfg.icons.contains_key("bad"));
        assert!(cfg.mouse_pixels);
        let path = write(&dir, "mouse_pixels = false\n");
        assert!(!Config::load(Some(&path)).unwrap().mouse_pixels);
    }

    #[test]
    fn bad_poll_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(Some(&write(&dir, "poll_seconds = \"soon\"\n")))
                .unwrap()
                .poll_seconds,
            5
        );
        assert_eq!(
            Config::load(Some(&write(&dir, "poll_seconds = -3\n")))
                .unwrap()
                .poll_seconds,
            0
        );
    }

    #[test]
    fn reminders_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "[reminders]\nenabled = true\nlist = \"Bear\"\ndue = \"\"\nremctl = \"/opt/remctl\"\n",
        );
        let cfg = Config::load(Some(&path)).unwrap();
        assert_eq!(
            cfg.reminders,
            RemindersConfig {
                enabled: true,
                list: "Bear".into(),
                due: String::new(),
                remctl: "/opt/remctl".into()
            }
        );
        assert_eq!(Config::default().reminders.due, "today");
    }

    #[test]
    fn actions_are_read_in_order_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "[[actions]]\nname = \"Publish to S3\"\ncommand = \"aws s3 cp \\\"$BJORN_NOTE_FILE\\\" s3://notes/\"\n\
             format = \"HTML\"\nconfirm = true\ntimeout = 300\ndefault = true\n\n\
             [[actions]]\ncommand = \"pbcopy\"\nformat = \"nonsense\"\n\n\
             [[actions]]\nname = \"No command\"\n",
        );
        let actions = Config::load(Some(&path)).unwrap().actions;
        assert_eq!(actions.len(), 2, "the entry without a command is dropped");
        assert_eq!(actions[0].name, "Publish to S3");
        assert_eq!(actions[0].format, "html");
        assert!(actions[0].confirm && actions[0].default);
        assert_eq!(actions[0].timeout, std::time::Duration::from_secs(300));
        assert_eq!(actions[1].name, "pbcopy", "the command names it");
        assert_eq!(actions[1].format, "md", "an unknown format falls back");
        assert!(!actions[1].confirm && !actions[1].default);
        assert_eq!(
            actions[1].timeout,
            std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECONDS)
        );
        assert!(
            Config::load(Some(&write(&dir, "actions = 3\n")))
                .unwrap()
                .actions
                .is_empty()
        );
    }

    #[test]
    fn xdg_config_home_is_honoured() {
        // Not parallel-safe to set env in tests; exercise the pure part instead.
        assert!(default_config_path().ends_with("bjorn/config.toml"));
    }

    #[test]
    fn editor_resolution_order() {
        let none = |_: &str| None::<String>;
        assert_eq!(resolve_editor(&Config::default(), &none), "vim");
        let editor_only = |k: &str| (k == "EDITOR").then(|| "nano".to_string());
        assert_eq!(resolve_editor(&Config::default(), &editor_only), "nano");
        let both = |k: &str| match k {
            "EDITOR" => Some("nano".to_string()),
            "VISUAL" => Some("code -w".to_string()),
            _ => None,
        };
        assert_eq!(resolve_editor(&Config::default(), &both), "code -w");
        let cfg = Config {
            editor: "hx".into(),
            ..Config::default()
        };
        assert_eq!(resolve_editor(&cfg, &both), "hx");
        assert!(editor_available("sh") && !editor_available("no-such-editor-xyz"));
        assert!(editor_available("sh -c true"));
        assert!(editor_available("/bin/sh"));
    }
}
