//! `bjorn --list-themes` and `--theme` print straight to the terminal, so
//! what a theme file or its name holds must not reach it as an escape
//! sequence.

use std::path::Path;
use std::process::Command;

/// A config file in a temp directory, with a `themes/` directory beside it
/// holding a file whose name and accent color carry control characters.
fn setup(dir: &Path) -> std::path::PathBuf {
    let config = dir.join("config.toml");
    std::fs::write(&config, "").unwrap();
    let themes = dir.join("themes");
    std::fs::create_dir(&themes).unwrap();
    let accent = serde_json::to_string("$\u{1b}]52;c;aGVsbG8=\u{7}").unwrap();
    std::fs::write(
        themes.join("Clip\u{1b}[2J\u{202e}.theme"),
        format!(
            r##"{{"base": {{"text color": "#111111", "background color": "#FFFFFF",
                "accent color": {accent}}}}}"##
        ),
    )
    .unwrap();
    config
}

fn assert_clean(text: &str) {
    assert!(
        !text
            .chars()
            .any(|c| (c.is_control() && c != '\n') || ('\u{202a}'..='\u{202e}').contains(&c)),
        "{text:?}"
    );
}

#[cfg(unix)]
#[test]
fn list_themes_escapes_what_a_skipped_file_holds() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_bjorn"))
        .arg("--config")
        .arg(&config)
        .arg("--list-themes")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_clean(&stderr);
    assert!(stderr.contains(r"Clip\u{1b}[2J\u{202e}.theme"), "{stderr}");
    assert!(stderr.contains(r"\u{1b}]52;c;aGVsbG8=\u{7}"), "{stderr}");
    assert_clean(&String::from_utf8(out.stdout).unwrap());
}

#[cfg(unix)]
#[test]
fn theme_escapes_why_a_file_did_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let config = setup(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_bjorn"))
        .arg("--config")
        .arg(&config)
        .arg("--theme")
        .arg("clip-2j")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_clean(&stderr);
    assert!(stderr.contains(r"\u{1b}]52;c;aGVsbG8=\u{7}"), "{stderr}");
}
