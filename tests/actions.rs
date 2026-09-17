//! Actions: the palette, the `!` default, confirmation, and what a command
//! actually receives.

mod common;

use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};

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

    // A whole word, not "pub": both commands quote a path under a temp
    // directory whose name is random, and a short query can turn up inside
    // one by luck.
    h.type_text("publish");
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
async fn without_actions_the_menu_offers_to_add_one() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("a");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));
    assert!(h.text().contains("+ New action…"), "{}", h.text());
    h.press("escape");
    h.press("!");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Actions"),
        "with nothing to run, ! opens the menu"
    );
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

#[tokio::test]
async fn the_menu_marks_the_default_and_shows_the_highlighted_command() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![
            Action {
                name: "Copy".into(),
                command: "pbcopy".into(),
                ..Action::default()
            },
            Action {
                name: "Publish".into(),
                command: "aws s3 cp \"$BJORN_NOTE_FILE\" s3://notes/".into(),
                format: "html".into(),
                confirm: true,
                prompt: None,
                interactive: false,
                default: true,
                timeout: Duration::from_secs(300),
            },
        ],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    let screen = h.text();
    assert!(screen.contains("on “Sprint Planning”"), "{screen}");
    assert_eq!(
        screen.matches("★ default").count(),
        2,
        "the row's badge and the hint: {screen}"
    );
    assert!(
        screen.contains("★ default · ! runs it without opening this menu"),
        "{screen}"
    );
    assert!(
        screen.contains("$ pbcopy"),
        "the first row's command: {screen}"
    );

    h.press("down");
    let screen = h.text();
    assert!(screen.contains("$ aws s3 cp"), "{screen}");
    assert!(
        screen.contains("renders as HTML · stops after 300 s · asks before running · runs on !"),
        "{screen}"
    );
}

#[tokio::test]
async fn the_menu_says_how_to_set_a_default_when_there_is_none() {
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
    let screen = h.text();
    assert!(!screen.contains("★ default"), "{screen}");
    assert!(screen.contains("no default yet"), "{screen}");
}

/// A harness whose config comes from a real file, so saving has somewhere to
/// write that is not the user's own config.
fn file_config(fake: &Fake, body: &str) -> (Config, std::path::PathBuf) {
    let path = fake.dir.path().join("config.toml");
    std::fs::write(&path, body).unwrap();
    let loaded = Config::load(Some(&path)).unwrap();
    let config = Config {
        poll_seconds: 0,
        icon_style: "none".into(),
        export_dir: fake.dir.path().join("exports"),
        ..loaded
    };
    (config, path)
}

#[tokio::test]
async fn a_new_action_from_the_menu_is_saved_and_runs() {
    let fake = Fake::new();
    let receipt = fake.dir.path().join("saved");
    let (config, path) = file_config(
        &fake,
        "# my notes config\nexport_format = \"md\"\n\n[[actions]]\nname = \"Copy\"\ncommand = \"true\"\ndefault = true  # keep me\n",
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;

    h.press("a");
    h.type_text("Save");
    assert!(h.text().contains("+ New action “Save”"), "{}", h.text());
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    h.type_text(&format!(
        "cp \"$BJORN_NOTE_FILE\" {}",
        shell_quote(&receipt.to_string_lossy())
    ));
    for _ in 0..3 {
        h.press("tab");
    }
    h.press("space");
    assert!(
        h.text().contains("“Copy” stops being the default"),
        "{}",
        h.text()
    );
    h.press("enter");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Actions"),
        "saving goes back to the menu"
    );
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("“Save” is saved in")),
        "{:?}",
        h.app.toast_messages()
    );

    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.starts_with("# my notes config\nexport_format = \"md\"\n"),
        "{body}"
    );
    assert!(body.contains("default = false  # keep me"), "{body}");
    let actions = Config::load(Some(&path)).unwrap().actions;
    assert_eq!(actions.len(), 2, "{body}");
    assert_eq!(
        bjorn::actions::default_action(&actions).unwrap().name,
        "Save"
    );
    assert_eq!(
        h.app.config.actions, actions,
        "the menu has it without a restart"
    );

    h.press("escape");
    h.press("!");
    h.until(|_| receipt.exists()).await;
}

#[tokio::test]
async fn the_new_action_form_wants_a_name_and_a_command() {
    let fake = Fake::new();
    let (config, path) = file_config(&fake, "export_format = \"md\"\n");
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    assert!(h.text().contains("+ New action…"), "{}", h.text());
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    h.press("enter");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("NewAction"),
        "no name yet"
    );
    h.type_text("Nothing");
    h.press("enter");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("NewAction"),
        "no command yet"
    );
    h.press("escape");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Actions"),
        "esc goes back to the menu"
    );
    h.press("escape");
    assert!(h.app.overlay.is_none());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "export_format = \"md\"\n"
    );
}

#[tokio::test]
async fn ctrl_e_edits_the_highlighted_action_in_place() {
    let fake = Fake::new();
    let receipt = fake.dir.path().join("edited");
    let (config, path) = file_config(
        &fake,
        "# my notes config\n[[actions]]\nname = \"Copy\"  # clipboard\ncommand = \"true\"\n\n\
         [[actions]]\nname = \"Upload\"\ncommand = \"false\"\ntimeout = 300\nprompt = \"Range\"\n",
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    h.press("down");
    h.key(KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    assert!(h.text().contains("Edit action"), "{}", h.text());

    h.type_text(" v2");
    h.press("tab");
    for _ in 0.."false".len() {
        h.press("backspace");
    }
    h.type_text(&format!(
        "cp \"$BJORN_NOTE_FILE\" {}",
        shell_quote(&receipt.to_string_lossy())
    ));
    h.press("enter");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Actions"),
        "saving goes back to the menu"
    );
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("“Upload v2” is updated in")),
        "{:?}",
        h.app.toast_messages()
    );

    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.starts_with(
            "# my notes config\n[[actions]]\nname = \"Copy\"  # clipboard\ncommand = \"true\"\n"
        ),
        "the other entry is untouched: {body}"
    );
    assert!(body.contains("name = \"Upload v2\"\n"), "{body}");
    assert!(
        body.contains("timeout = 300\n") && body.contains("prompt = \"Range\"\n"),
        "keys the form does not show survive: {body}"
    );
    let actions = Config::load(Some(&path)).unwrap().actions;
    assert_eq!(actions.len(), 2, "{body}");
    assert_eq!(
        h.app.config.actions, actions,
        "the menu has it without a restart"
    );

    h.press("escape");
    h.press("a");
    h.press("down");
    h.press("enter");
    // The entry kept its `prompt`, so running it asks before the command goes.
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Text"));
    h.press("enter");
    h.until(|_| receipt.exists()).await;
}

#[tokio::test]
async fn ctrl_d_deletes_an_action_after_asking() {
    let fake = Fake::new();
    let body = "# my notes config\n[[actions]]\nname = \"Copy\"\ncommand = \"true\"\n\n\
                [[actions]]\nname = \"Upload\"\ncommand = \"false\"\n\n# templates below\n";
    let (config, path) = file_config(&fake, body);
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    h.press("down");
    h.key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    assert!(
        h.text().contains("Delete “Upload” from the config?"),
        "{}",
        h.text()
    );

    h.press("escape");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Actions"),
        "cancelling goes back to the menu"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        body,
        "nothing is written on cancel"
    );

    h.key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    h.press("y");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("“Upload” is gone from")),
        "{:?}",
        h.app.toast_messages()
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# my notes config\n[[actions]]\nname = \"Copy\"\ncommand = \"true\"\n\n# templates below\n"
    );
    assert_eq!(h.app.config.actions.len(), 1);
    assert_eq!(h.app.config.actions[0].name, "Copy");
}

/// An action that records `$BJORN_ACTION_INPUT`, and nothing else, so an empty
/// answer is visible as an empty file rather than a missing one.
fn recording_input(dir: &std::path::Path, name: &str) -> Action {
    let receipt = dir.join(format!("{name}.receipt"));
    Action {
        name: name.to_string(),
        command: format!(
            "printf '%s' \"$BJORN_ACTION_INPUT\" > {}; echo sent",
            shell_quote(&receipt.to_string_lossy())
        ),
        ..Action::default()
    }
}

#[tokio::test]
async fn a_prompt_action_asks_for_a_line_and_passes_it_to_the_command() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            prompt: Some("Range".into()),
            default: true,
            ..recording_input(fake.dir.path(), "Sync")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Text"));
    assert!(h.text().contains("Range"), "{}", h.text());
    for key in ["l", "a", "s", "t", "-", "w", "e", "e", "k"] {
        h.press(key);
    }
    h.press("enter");
    h.until(|_| fake.dir.path().join("Sync.receipt").exists())
        .await;
    assert_eq!(receipt(&fake, "Sync"), "last-week");
}

#[tokio::test]
async fn an_empty_answer_still_runs_and_escape_cancels() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            prompt: Some("Range".into()),
            default: true,
            ..recording_input(fake.dir.path(), "Sync")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;

    h.press("!");
    h.press("escape");
    h.settle().await;
    assert!(h.app.overlay.is_none());
    assert!(!fake.dir.path().join("Sync.receipt").exists());

    // Enter on an empty field is an answer: the command decides what it means.
    h.press("!");
    h.press("enter");
    h.until(|_| fake.dir.path().join("Sync.receipt").exists())
        .await;
    assert_eq!(receipt(&fake, "Sync"), "");
}

#[tokio::test]
async fn a_prompt_action_that_confirms_quotes_the_answer() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            prompt: Some("Range".into()),
            confirm: true,
            default: true,
            ..recording_input(fake.dir.path(), "Sync")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.press("2");
    h.press("w");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    assert!(h.text().contains("Run “Sync” on “2w”?"), "{}", h.text());
    h.press("y");
    h.until(|_| fake.dir.path().join("Sync.receipt").exists())
        .await;
    assert_eq!(receipt(&fake, "Sync"), "2w");
}

#[tokio::test]
async fn an_action_without_a_prompt_still_gets_the_variable_set_and_empty() {
    // Documented in docs/actions.md: `$BJORN_ACTION_INPUT` is always defined, so
    // a command under `set -u` can read it without guarding.
    let fake = Fake::new();
    let receipt = fake.dir.path().join("Plain.receipt");
    let config = config_with(
        &fake,
        vec![Action {
            name: "Plain".into(),
            command: format!(
                "set -u; printf '[%s]' \"$BJORN_ACTION_INPUT\" > {}; echo sent",
                shell_quote(&receipt.to_string_lossy())
            ),
            default: true,
            ..Action::default()
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.until(|_| receipt.exists()).await;
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "[]");
}

#[tokio::test]
async fn an_empty_answer_is_not_confirmed_as_the_note_title() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            prompt: Some("Range".into()),
            confirm: true,
            default: true,
            ..recording_input(fake.dir.path(), "Sync")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    // The note under the cursor is "Sprint Planning"; naming it here would point
    // at something this command never touches.
    let text = h.text();
    assert!(text.contains("Run “Sync” on no input?"), "{text}");
    assert!(!text.contains("on “Sprint Planning”?"), "{text}");
}

#[tokio::test]
async fn the_menu_says_an_action_asks_and_what_it_asks() {
    let fake = Fake::new();
    let config = config_with(
        &fake,
        vec![Action {
            prompt: Some("Range".into()),
            ..recording_input(fake.dir.path(), "Sync")
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    assert!(h.text().contains("asks: Range"), "{}", h.text());
}

#[tokio::test]
async fn an_interactive_action_takes_the_window_and_the_keyboard() {
    let fake = Fake::new();
    let receipt = fake.dir.path().join("typed");
    // Draws a banner, waits for a line, writes it out: a stand-in for anything
    // that talks back.
    let config = config_with(
        &fake,
        vec![Action {
            name: "Session".into(),
            command: format!(
                "printf 'SESSION UP %s' \"$BJORN_ACTION_INPUT\"; read -r line; printf '%s' \"$line\" > {}",
                shell_quote(&receipt.to_string_lossy())
            ),
            interactive: true,
            prompt: Some("Range".into()),
            default: true,
            ..Action::default()
        }],
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;

    h.press("!");
    h.type_text("week");
    h.press("enter");
    h.until(|app| app.session.is_some()).await;
    h.until(|app| {
        app.session
            .as_ref()
            .is_some_and(|s| s.pty.contents().contains("SESSION UP"))
    })
    .await;
    h.draw();
    let text = h.text();
    // The prompt's answer reached it, and the window is its own: the note
    // columns are gone and the footer says where the keys go.
    assert!(text.contains("SESSION UP week"), "{text}");
    assert!(text.contains("Session"), "{text}");
    assert!(text.contains("Keys go to the command"), "{text}");
    assert!(
        !text.contains("Garden Plan"),
        "the note list is not drawn: {text}"
    );

    // Keys reach the command, not Bjorn: "q" would otherwise ask to quit.
    h.type_text("qq");
    h.press("enter");
    h.until(|_| receipt.exists()).await;
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "qq");

    // When it ends, Bjorn comes back.
    h.until(|app| app.session.is_none()).await;
    h.draw();
    assert!(h.text().contains("Garden Plan"), "{}", h.text());
}
