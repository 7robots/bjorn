//! Actions: the palette, the `!` default, confirmation, and what a command
//! actually receives.

mod common;

use std::time::Duration;

use bjorn::actions::Action;
use bjorn::config::Config;
use common::Fake;

/// An action that writes what it was given into `receipt` beside the state file.
fn recording(dir: &std::path::Path, name: &str) -> Action {
    let receipt = dir.join(format!("{name}.receipt"));
    Action {
        name: name.to_string(),
        command: format!(
            "{{ printf '%s\\n%s\\n%s\\n' \"$BJORN_ACTION\" \"$BJORN_NOTE_TITLE\" \"$BJORN_NOTE_FILE\"; cat; }} > {}; echo sent",
            shell_quote(&receipt.to_string_lossy())
        ),
        ..Action::default()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn config_with(fake: &Fake, actions: Vec<Action>) -> Config {
    Config {
        actions,
        ..fake.config()
    }
}

fn receipt(fake: &Fake, name: &str) -> String {
    std::fs::read_to_string(fake.dir.path().join(format!("{name}.receipt"))).unwrap()
}

#[tokio::test]
async fn the_palette_filters_and_runs_the_highlighted_action() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![
            recording(fake.dir.path(), "Copy"),
            recording(fake.dir.path(), "Publish"),
        ],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;

    h.press("a");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));
    let screen = h.text();
    assert!(
        screen.contains("Copy") && screen.contains("Publish"),
        "{screen}"
    );

    h.type_text("pub");
    let screen = h.text();
    assert!(screen.contains("Publish"), "{screen}");
    assert!(!screen.contains("▸ Copy"), "{screen}");

    h.press("enter");
    assert!(h.app.overlay.is_none());
    h.until(|app| app.toast_messages().iter().any(|m| m.contains("sent")))
        .await;

    let receipt = receipt(&fake, "Publish");
    let mut lines = receipt.lines();
    assert_eq!(lines.next(), Some("Publish"));
    assert_eq!(lines.next(), Some("Sprint Planning"));
    assert!(
        lines.next().unwrap().ends_with("Sprint Planning.md"),
        "{receipt}"
    );
    assert!(
        receipt.contains("- [ ] write the release notes"),
        "{receipt}"
    );
    assert!(
        !fake.dir.path().join("Copy.receipt").exists(),
        "only the chosen action ran"
    );
}

#[tokio::test]
async fn bang_runs_the_default_action_without_the_palette() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![
            recording(fake.dir.path(), "Copy"),
            Action {
                default: true,
                ..recording(fake.dir.path(), "Publish")
            },
        ],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    assert!(h.app.overlay.is_none(), "no palette for a default action");
    h.until(|_| fake.dir.path().join("Publish.receipt").exists())
        .await;
    assert!(receipt(&fake, "Publish").starts_with("Publish\nSprint Planning\n"));
}

#[tokio::test]
async fn bang_opens_the_palette_when_no_default_is_marked() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![
            recording(fake.dir.path(), "Copy"),
            recording(fake.dir.path(), "Publish"),
        ],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));
    h.press("escape");
    assert!(h.app.overlay.is_none());
}

#[tokio::test]
async fn a_lone_action_is_the_default() {
    let fake = Fake::new();
    let config = config_with(&fake, vec![recording(fake.dir.path(), "Copy")]);
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|_| fake.dir.path().join("Copy.receipt").exists())
        .await;
}

#[tokio::test]
async fn a_confirm_action_asks_first_and_can_be_cancelled() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            confirm: true,
            default: true,
            ..recording(fake.dir.path(), "Publish")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    assert!(h.text().contains("Run “Publish” on “Sprint Planning”?"));
    h.press("escape");
    h.settle().await;
    assert!(!fake.dir.path().join("Publish.receipt").exists());

    h.press("!");
    h.press("y");
    h.until(|_| fake.dir.path().join("Publish.receipt").exists())
        .await;
}

#[tokio::test]
async fn a_failing_action_reports_the_exit_code_and_message() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            default: true,
            command: "echo 'no credentials' >&2; exit 3".into(),
            ..Action {
                name: "Publish".into(),
                ..Action::default()
            }
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("exit 3: no credentials"))
    })
    .await;
    assert!(h.text().contains("Publish failed"));
}

#[tokio::test]
async fn html_actions_get_the_rendered_note() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            default: true,
            format: "html".into(),
            ..recording(fake.dir.path(), "Web")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|_| fake.dir.path().join("Web.receipt").exists())
        .await;
    let receipt = receipt(&fake, "Web");
    assert!(receipt.contains("Sprint Planning.html"), "{receipt}");
    assert!(receipt.contains("<!DOCTYPE html>"), "{receipt}");
}

#[tokio::test]
async fn without_actions_the_keys_say_where_to_configure_them() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("a");
    assert!(h.app.overlay.is_none());
    assert!(h.text().contains("No actions configured"), "{}", h.text());
    h.press("!");
    assert!(h.app.overlay.is_none());
}

#[tokio::test]
async fn the_palette_does_not_open_on_a_locked_note() {
    let fake = Fake::new();
    let config = config_with(&fake, vec![recording(fake.dir.path(), "Copy")]);
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.app.notes.notes[0].locked = true;
    h.press("a");
    assert!(h.app.overlay.is_none());
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("Locked notes cannot be sent"))
    })
    .await;
}

/// The command must not still be able to reach the payload afterwards.
#[tokio::test]
async fn the_payload_is_removed_when_the_command_ends() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            default: true,
            command: format!(
                "printf '%s' \"$BJORN_NOTE_FILE\" > {}",
                shell_quote(&fake.dir.path().join("path").to_string_lossy())
            ),
            name: "Where".into(),
            ..Action::default()
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|_| fake.dir.path().join("path").exists()).await;
    // The toast is the signal that the run is over, not just the write.
    h.until(|app| !app.toast_messages().iter().any(|m| m.contains("Running")))
        .await;
    let payload = std::fs::read_to_string(fake.dir.path().join("path")).unwrap();
    assert!(!std::path::Path::new(&payload).exists(), "{payload}");
}

/// A slow action must not block the interface.
#[tokio::test]
async fn a_running_action_leaves_the_app_usable() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            default: true,
            name: "Slow".into(),
            command: "sleep 0.4".into(),
            timeout: Duration::from_secs(5),
            ..Action::default()
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.press("j");
    assert_eq!(h.app.notes.cursor, Some(1), "the list still moves");
    h.until(|app| app.toast_messages().iter().any(|m| m == "Done."))
        .await;
}

/// The whole path from a config file on disk to a command that ran.
#[tokio::test]
async fn actions_come_from_the_config_file() {
    let fake = Fake::new();
    let receipt = fake.dir.path().join("from-config");
    let path = fake.dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[[actions]]\nname = \"Save it\"\ndefault = true\nformat = \"txt\"\n\
             command = \"cp \\\"$BJORN_NOTE_FILE\\\" {}\"\n",
            shell_quote(&receipt.to_string_lossy())
        ),
    )
    .unwrap();
    let loaded = Config::load(Some(&path)).unwrap();
    let config = Config {
        poll_seconds: 0,
        icon_style: "none".into(),
        export_dir: fake.dir.path().join("exports"),
        ..loaded
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|_| receipt.exists()).await;
    let text = std::fs::read_to_string(&receipt).unwrap();
    assert!(text.starts_with("Sprint Planning\n"), "{text}");
    assert!(!text.contains("- [ ]"), "txt renders the checkbox: {text}");
}
