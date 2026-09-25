//! Actions whose output goes back into Bear: `output = "append"`,
//! `"new-note"` and `"replace"`, and the cases where nothing may be written.

mod common;

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers};

use bjorn::actions::{Action, ActionOutput};
use bjorn::config::Config;
use bjorn::harness::Harness;
use bjorn::ui::modals::Overlay;
use common::Fake;

const PLANNING: &str = "NOTE-PLANNING";

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A default action that runs `command` and sends its output to `output`.
fn writer(name: &str, command: &str, output: ActionOutput) -> Action {
    Action {
        name: name.into(),
        command: command.into(),
        output,
        default: true,
        ..Action::default()
    }
}

async fn start(fake: &Fake, action: Action, workspace: Option<&str>) -> Harness {
    let config = Config {
        actions: vec![action],
        ..fake.config()
    };
    let mut h = fake.harness_with(config, workspace);
    h.load().await;
    h
}

async fn body(fake: &Fake, id: &str) -> String {
    fake.client().cat(id).await.unwrap().content
}

async fn note_count(fake: &Fake) -> usize {
    fake.client().snapshot().await.unwrap().notes.len()
}

async fn wait_for_toast(h: &mut Harness, needle: &str) -> String {
    h.until(|app| app.toast_messages().iter().any(|m| m.contains(needle)))
        .await;
    h.app
        .toast_messages()
        .into_iter()
        .find(|m| m.contains(needle))
        .unwrap()
}

/// The path at the end of "… the output is at <path>", which the test removes.
fn kept_file(message: &str) -> PathBuf {
    PathBuf::from(message.rsplit("the output is at ").next().unwrap().trim())
}

fn remove_kept(path: &Path) {
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(path.parent().unwrap()).unwrap();
}

/// A command that changes the note in Bear behind Bjorn's back.
fn change_in_bear(fake: &Fake) -> String {
    format!(
        "BJORN_FAKE_BEAR_STATE={} {} overwrite {PLANNING} --content '# Sprint Planning\\n\\nchanged in Bear\\n'",
        shell_quote(&fake.state().to_string_lossy()),
        shell_quote(env!("CARGO_BIN_EXE_fake-bearcli"))
    )
}

#[tokio::test]
async fn append_adds_the_output_to_the_end_of_the_note() {
    let fake = Fake::new();
    let before = body(&fake, PLANNING).await;
    let mut h = start(
        &fake,
        writer(
            "Summarize",
            "printf '\\n\\nA short summary.\\n\\n'",
            ActionOutput::Append,
        ),
        None,
    )
    .await;
    h.press("!");
    wait_for_toast(&mut h, "Added to the end of “Sprint Planning”").await;
    assert_eq!(
        body(&fake, PLANNING).await,
        format!("{}\nA short summary.\n", before.trim_end())
    );
    h.until(|app| app.reader.plain_text().contains("A short summary."))
        .await;
}

#[tokio::test]
async fn append_can_go_under_a_section() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        Action {
            section: Some("## Tasks".into()),
            ..writer(
                "Add a task",
                "echo '- [ ] from the action'",
                ActionOutput::Append,
            )
        },
        None,
    )
    .await;
    h.press("!");
    wait_for_toast(&mut h, "Added under “## Tasks” in “Sprint Planning”").await;
    let after = body(&fake, PLANNING).await;
    assert!(
        after.contains("  - [ ] confirm the sunset date\n- [ ] from the action\n\n## Notes\n"),
        "{after}"
    );
}

#[tokio::test]
async fn append_to_a_missing_section_writes_nothing_and_keeps_the_output() {
    let fake = Fake::new();
    let before = body(&fake, PLANNING).await;
    let mut h = start(
        &fake,
        Action {
            section: Some("## Summary".into()),
            ..writer("Summarize", "echo 'worth keeping'", ActionOutput::Append)
        },
        None,
    )
    .await;
    h.press("!");
    let message = wait_for_toast(&mut h, "the output is at").await;
    assert!(message.contains("Section not found"), "{message}");
    assert!(h.text().contains("Summarize: write failed"), "{}", h.text());
    let kept = kept_file(&message);
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "worth keeping\n");
    remove_kept(&kept);
    assert_eq!(body(&fake, PLANNING).await, before);
}

#[tokio::test]
async fn new_note_makes_a_note_of_the_output_and_selects_it() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        Action {
            prompt: Some("Issue number".into()),
            ..writer(
                "Issue to note",
                "printf '# Issue %s\\n\\nFrom the tracker.\\n' \"$BJORN_ACTION_INPUT\"",
                ActionOutput::NewNote,
            )
        },
        Some("work"),
    )
    .await;
    let count = note_count(&fake).await;
    h.press("!");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Text"));
    h.type_text("42");
    h.press("enter");
    wait_for_toast(&mut h, "Created “Issue 42”.").await;
    h.until(|app| app.notes.current().is_some_and(|n| n.title == "Issue 42"))
        .await;
    assert_eq!(note_count(&fake).await, count + 1);
    let id = h.app.notes.current().unwrap().id.clone();
    let created = body(&fake, &id).await;
    // Tagged with the workspace, as `n` would.
    assert_eq!(created, "# Issue 42\n#work\n\nFrom the tracker.\n");
}

#[tokio::test]
async fn new_note_keeps_front_matter_and_takes_the_title_after_it() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        writer(
            "PR to note",
            "printf -- '---\\nsource: gh\\n---\\n# PR 7\\n\\nMerged.\\n'",
            ActionOutput::NewNote,
        ),
        None,
    )
    .await;
    h.press("!");
    wait_for_toast(&mut h, "Created “PR 7”.").await;
    h.until(|app| app.notes.current().is_some_and(|n| n.title == "PR 7"))
        .await;
    let id = h.app.notes.current().unwrap().id.clone();
    assert!(
        body(&fake, &id)
            .await
            .starts_with("---\nsource: gh\n---\n# PR 7\n"),
        "{}",
        body(&fake, &id).await
    );
}

#[tokio::test]
async fn replace_always_asks_first_and_can_be_called_off() {
    let fake = Fake::new();
    let before = body(&fake, PLANNING).await;
    let marker = fake.dir.path().join("ran");
    let mut h = start(
        &fake,
        writer(
            "Shout",
            &format!(
                "touch {}; tr a-z A-Z",
                shell_quote(&marker.to_string_lossy())
            ),
            ActionOutput::Replace,
        ),
        None,
    )
    .await;
    h.press("!");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    match &h.app.overlay {
        Some(Overlay::Confirm {
            message,
            confirm_label,
            ..
        }) => {
            assert_eq!(
                message,
                "Run “Shout” and replace “Sprint Planning” with what it prints?"
            );
            assert_eq!(confirm_label, "Replace");
        }
        other => panic!("{other:?}"),
    }
    h.press("escape");
    h.settle().await;
    assert!(!marker.exists(), "canceled before the command ran");
    assert_eq!(body(&fake, PLANNING).await, before);

    h.press("!");
    h.press("y");
    let message = wait_for_toast(&mut h, "Replaced “Sprint Planning”.").await;
    assert_eq!(body(&fake, PLANNING).await, before.to_uppercase());
    // Bear cannot undo it, so the text it had is kept, privately.
    let backup = PathBuf::from(
        message
            .rsplit("The text it had is at ")
            .next()
            .unwrap()
            .trim(),
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    remove_kept(&backup);
    h.until(|app| app.notes.titles().contains(&"SPRINT PLANNING".to_string()))
        .await;
}

#[tokio::test]
async fn replace_refuses_a_note_that_changed_while_the_command_ran() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        writer(
            "Rewrite",
            &format!(
                "{}; printf '# Sprint Planning\\n\\nmy rewrite\\n'",
                change_in_bear(&fake)
            ),
            ActionOutput::Replace,
        ),
        None,
    )
    .await;
    h.press("!");
    h.press("y");
    let message = wait_for_toast(&mut h, "the output is at").await;
    assert!(
        message
            .contains("“Sprint Planning” changed in Bear while “Rewrite” ran. Nothing was written"),
        "{message}"
    );
    assert!(h.text().contains("Edit conflict"), "{}", h.text());
    let kept = kept_file(&message);
    assert_eq!(
        std::fs::read_to_string(&kept).unwrap(),
        "# Sprint Planning\n\nmy rewrite\n"
    );
    remove_kept(&kept);
    assert!(body(&fake, PLANNING).await.contains("changed in Bear"));
}

/// A bearcli that fails after Bear saved (a timeout, a killed process) proves
/// nothing about the note, so the copy of its old text stays and is named.
#[tokio::test]
async fn a_replace_that_fails_without_proof_keeps_the_old_text() {
    let fake = Fake::new();
    let before = body(&fake, PLANNING).await;
    let knob = PathBuf::from(format!("{}.overwrite-fails-after", fake.state().display()));
    std::fs::write(&knob, "").unwrap();
    let mut h = start(
        &fake,
        writer("Shout", "tr a-z A-Z", ActionOutput::Replace),
        None,
    )
    .await;
    h.press("!");
    h.press("y");
    let message = wait_for_toast(&mut h, "the text it had is at").await;
    std::fs::remove_file(&knob).unwrap();
    assert!(
        message.contains("Lost touch with Bear before it answered — the output is at "),
        "{message}"
    );
    assert!(
        message.contains("Bear may have replaced “Sprint Planning” anyway"),
        "{message}"
    );
    // Bear did write, and the copy is all that is left of what it replaced.
    assert_eq!(body(&fake, PLANNING).await, before.to_uppercase());
    let backup = PathBuf::from(
        message
            .rsplit("the text it had is at ")
            .next()
            .unwrap()
            .trim(),
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);
    remove_kept(&backup);
    let kept = PathBuf::from(
        message
            .split("the output is at ")
            .nth(1)
            .unwrap()
            .split(". Bear may have")
            .next()
            .unwrap(),
    );
    assert_eq!(
        std::fs::read_to_string(&kept).unwrap(),
        before.to_uppercase()
    );
    remove_kept(&kept);
}

#[tokio::test]
async fn empty_output_never_makes_a_note_or_wipes_one() {
    for output in [
        ActionOutput::Append,
        ActionOutput::NewNote,
        ActionOutput::Replace,
    ] {
        let fake = Fake::new();
        let before = body(&fake, PLANNING).await;
        let mut h = start(
            &fake,
            writer("Quiet", "printf '\\n  \\n\\t\\n'", output),
            None,
        )
        .await;
        let count = note_count(&fake).await;
        h.press("!");
        if output == ActionOutput::Replace {
            h.press("y");
        }
        let message = wait_for_toast(&mut h, "nothing was written to Bear").await;
        assert!(message.contains("printed nothing"), "{output:?}: {message}");
        assert_eq!(body(&fake, PLANNING).await, before, "{output:?}");
        assert_eq!(note_count(&fake).await, count, "{output:?}");
    }
}

#[tokio::test]
async fn a_failing_command_writes_nothing() {
    for output in [
        ActionOutput::Append,
        ActionOutput::NewNote,
        ActionOutput::Replace,
    ] {
        let fake = Fake::new();
        let before = body(&fake, PLANNING).await;
        let mut h = start(
            &fake,
            writer(
                "Broken",
                "printf '# Half done\\n'; echo 'rate limited' >&2; exit 2",
                output,
            ),
            None,
        )
        .await;
        let count = note_count(&fake).await;
        h.press("!");
        if output == ActionOutput::Replace {
            h.press("y");
        }
        let message = wait_for_toast(&mut h, "exit 2: rate limited").await;
        assert!(
            message.contains("Nothing was written to Bear."),
            "{message}"
        );
        assert!(h.text().contains("Broken failed"), "{}", h.text());
        assert_eq!(body(&fake, PLANNING).await, before, "{output:?}");
        assert_eq!(note_count(&fake).await, count, "{output:?}");
    }
}

#[tokio::test]
async fn output_past_the_size_cap_is_refused() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        writer(
            "Firehose",
            "printf '# Big\\n'; head -c 1100000 /dev/zero | tr '\\0' 'a'",
            ActionOutput::NewNote,
        ),
        None,
    )
    .await;
    let count = note_count(&fake).await;
    h.press("!");
    let message = wait_for_toast(&mut h, "nothing was written to Bear").await;
    assert!(message.contains("more than 1 MB"), "{message}");
    assert_eq!(note_count(&fake).await, count);
    // The first megabyte is kept, as the limit promises.
    let kept = kept_file(&message);
    assert_eq!(std::fs::metadata(&kept).unwrap().len(), 1024 * 1024);
    remove_kept(&kept);
}

#[tokio::test]
async fn an_interactive_action_with_an_output_does_not_run() {
    let fake = Fake::new();
    let marker = fake.dir.path().join("ran");
    let mut h = start(
        &fake,
        Action {
            interactive: true,
            ..writer(
                "Chat",
                &format!("touch {}", shell_quote(&marker.to_string_lossy())),
                ActionOutput::Append,
            )
        },
        None,
    )
    .await;
    h.press("!");
    let message = wait_for_toast(&mut h, "an interactive action").await;
    assert!(
        message.contains("remove interactive or output"),
        "{message}"
    );
    assert!(h.app.session.is_none());
    h.settle().await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn the_menu_says_where_the_output_goes() {
    let fake = Fake::new();
    let mut h = start(
        &fake,
        Action {
            section: Some("## Summary".into()),
            ..writer("Summarize", "true", ActionOutput::Append)
        },
        None,
    )
    .await;
    h.press("a");
    assert!(
        h.text().contains("appends under ## Summary"),
        "{}",
        h.text()
    );
}

/// A config read from a temp file, so the form has somewhere to save that is
/// not the user's own config.
fn file_config(fake: &Fake, body: &str) -> (Config, PathBuf) {
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
async fn the_form_sets_the_output_and_the_section() {
    let fake = Fake::new();
    let (config, path) = file_config(&fake, "# mine\n");
    let mut h = fake.harness_with(config, None);
    h.load().await;

    h.press("a");
    h.type_text("Summarize");
    h.press("enter");
    h.type_text("echo summary");
    // Command (1) → output (5), then the section (6).
    for _ in 0..4 {
        h.press("tab");
    }
    h.press("right");
    assert!(h.text().contains("‹ Append ›"), "{}", h.text());
    h.press("tab");
    h.type_text("## Summary");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("output = \"append\"\nsection = \"## Summary\"\n"),
        "{text}"
    );
    let saved = Config::load(Some(&path)).unwrap().actions;
    assert_eq!(saved[0].output, ActionOutput::Append);
    assert_eq!(saved[0].section.as_deref(), Some("## Summary"));

    // An edit that moves it to a new note has to drop the section, which
    // only an append uses; the cleared key reads back as none.
    h.key(KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert!(h.text().contains("‹ Append ›"), "{}", h.text());
    for _ in 0..5 {
        h.press("tab");
    }
    h.press("right");
    assert!(h.text().contains("‹ New note ›"), "{}", h.text());
    h.press("enter");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("NewAction"),
        "a section on a new note is refused"
    );
    h.press("tab");
    for _ in 0.."## Summary".len() {
        h.press("backspace");
    }
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Actions"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("output = \"new-note\"\nsection = \"\"\n"),
        "{text}"
    );
    let saved = &Config::load(Some(&path)).unwrap().actions[0];
    assert_eq!(saved.output, ActionOutput::NewNote);
    assert_eq!(saved.section, None);
}

#[tokio::test]
async fn output_that_is_not_utf8_is_kept_not_written() {
    let fake = Fake::new();
    let before = body(&fake, PLANNING).await;
    let mut h = start(
        &fake,
        writer("Binary", r"printf '# T\n\377\376\n'", ActionOutput::Append),
        None,
    )
    .await;
    h.press("!");
    let message = wait_for_toast(&mut h, "the output is at").await;
    assert!(message.contains("not UTF-8"), "{message}");
    let kept = kept_file(&message);
    assert_eq!(std::fs::read(&kept).unwrap(), b"# T\n\xff\xfe\n");
    remove_kept(&kept);
    assert_eq!(body(&fake, PLANNING).await, before);
}

#[tokio::test]
async fn a_note_trashed_while_the_command_ran_is_not_written() {
    let fake = Fake::new();
    let trash = format!(
        "BJORN_FAKE_BEAR_STATE={} {} trash {PLANNING}",
        shell_quote(&fake.state().to_string_lossy()),
        shell_quote(env!("CARGO_BIN_EXE_fake-bearcli"))
    );
    let before = body(&fake, PLANNING).await;
    let mut h = start(
        &fake,
        writer(
            "Late",
            &format!("{trash}; echo 'too late'"),
            ActionOutput::Append,
        ),
        None,
    )
    .await;
    h.press("!");
    let message = wait_for_toast(&mut h, "went to the trash while it ran").await;
    let kept = kept_file(&message);
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "too late\n");
    remove_kept(&kept);
    assert_eq!(body(&fake, PLANNING).await, before);
}

#[tokio::test]
async fn the_cursor_stays_where_you_moved_it_while_the_command_ran() {
    for output in [ActionOutput::Append, ActionOutput::Replace] {
        let fake = Fake::new();
        let mut h = start(&fake, writer("Slow", "sleep 0.4; cat", output), None).await;
        h.press("!");
        if output == ActionOutput::Replace {
            h.press("y");
        }
        h.press("j");
        let moved_to = h.app.notes.current().unwrap().title.clone();
        assert_ne!(moved_to, "Sprint Planning");
        wait_for_toast(&mut h, "“Sprint Planning”").await;
        let _ = h
            .wait_until(|_| false, std::time::Duration::from_millis(400))
            .await;
        assert_eq!(h.app.notes.current().unwrap().title, moved_to, "{output:?}");
        if output == ActionOutput::Replace {
            let message = wait_for_toast(&mut h, "The text it had is at").await;
            remove_kept(Path::new(
                message
                    .rsplit("The text it had is at ")
                    .next()
                    .unwrap()
                    .trim(),
            ));
        }
    }
}

#[tokio::test]
async fn an_unknown_output_stops_the_action_before_it_runs() {
    let fake = Fake::new();
    let marker = fake.dir.path().join("ran");
    let (config, _) = file_config(
        &fake,
        &format!(
            "[[actions]]\nname = \"Sum\"\ncommand = \"touch {}\"\noutput = \"apend\"\ndefault = true\n",
            marker.display()
        ),
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    let message = wait_for_toast(&mut h, "is not one of").await;
    assert!(
        message.contains("output = \"apend\" is not one of toast, append, new-note, replace"),
        "{message}"
    );
    h.settle().await;
    assert!(
        !marker.exists(),
        "no paid call for output that goes nowhere"
    );
}

#[tokio::test]
async fn an_unknown_format_stops_the_action_before_it_runs() {
    let fake = Fake::new();
    let marker = fake.dir.path().join("ran");
    let (config, _) = file_config(
        &fake,
        &format!(
            "[[actions]]\nname = \"Print\"\ncommand = \"touch {}\"\nformat = \"pfd\"\ndefault = true\n",
            marker.display()
        ),
    );
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("!");
    let message = wait_for_toast(&mut h, "is not one of").await;
    assert!(
        message.contains("format = \"pfd\" is not one of md, html, txt, rtf, textbundle"),
        "{message}"
    );
    h.settle().await;
    assert!(
        !marker.exists(),
        "not run on a Markdown file it never asked for"
    );
}

/// Editing an action whose format is a typo shows the typo and will not save
/// until a real format is picked, so the form never writes "md" over it unseen.
#[tokio::test]
async fn the_form_shows_an_unknown_format_and_saves_only_a_chosen_one() {
    let fake = Fake::new();
    let body =
        "[[actions]]\nname = \"Print\"\ncommand = \"weasyprint - out.pdf\"\nformat = \"pfd\"\n";
    let (config, path) = file_config(&fake, body);
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    h.key(KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    let screen = h.text();
    assert!(screen.contains("‹ \"pfd\" ›"), "{screen}");
    assert!(screen.contains("is not one of md, html"), "{screen}");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);

    h.press("tab");
    h.press("tab"); // format
    h.press("right"); // from the unknown one, the first: Markdown
    assert!(h.text().contains("‹ Markdown ›"), "{}", h.text());
    h.press("right"); // HTML
    h.press("enter");
    h.until(|app| app.overlay.as_ref().map(|o| o.name()) != Some("NewAction"))
        .await;
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("format = \"html\""), "{saved}");
    assert!(!saved.contains("pfd"), "{saved}");
}

#[tokio::test]
async fn replace_needs_markdown() {
    let fake = Fake::new();
    let marker = fake.dir.path().join("ran");
    let mut h = start(
        &fake,
        Action {
            format: "html".into(),
            ..writer(
                "Tidy",
                &format!("touch {}", shell_quote(&marker.to_string_lossy())),
                ActionOutput::Replace,
            )
        },
        None,
    )
    .await;
    h.press("!");
    let message = wait_for_toast(&mut h, "needs format = \"md\"").await;
    assert!(message.contains("HTML"), "{message}");
    assert!(h.app.overlay.is_none(), "not even the confirm dialog");
    h.settle().await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn the_form_will_not_save_an_action_that_could_not_run() {
    let fake = Fake::new();
    let (config, path) = file_config(&fake, "# mine\n");
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("a");
    h.type_text("Tidy");
    h.press("enter");
    h.type_text("llm");
    h.press("tab"); // format
    h.press("right"); // HTML
    for _ in 0..3 {
        h.press("tab"); // output
    }
    for _ in 0..3 {
        h.press("right"); // Replace
    }
    let screen = h.text();
    assert!(screen.contains("‹ Replace ›"), "{screen}");
    assert!(
        screen.contains("always, for an action that replaces"),
        "{screen}"
    );
    assert!(screen.contains("needs format = \"md\""), "{screen}");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("needs format")),
        "{:?}",
        h.app.toast_messages()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");

    // A section on anything but append is flagged the same way.
    h.press("left"); // New note
    h.press("tab");
    h.type_text("## S");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewAction"));
    assert!(
        h.text().contains("section is only used with"),
        "{}",
        h.text()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
}
