//! Writes: editor round trip with the hash guard, create, trash, restore, pin,
//! open in Bear, and export in every format.

mod common;

use std::collections::HashMap;
use std::path::Path;

use bjorn::app::Pane;
use bjorn::model::View;
use common::{Fake, titles};

/// A shell script standing in for `$EDITOR`; it gets the temp file as `$1`.
fn fake_editor(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn edit_round_trips_through_editor() {
    let fake = Fake::new();
    let editor = fake_editor(
        fake.dir.path(),
        "editor.sh",
        "printf '\\nAdded from the editor\\n' >> \"$1\"",
    );
    let mut h = fake.harness_env(env(&[("EDITOR", &editor)]));
    h.load().await;
    h.press("e");
    h.until(|app| app.reader.plain_text().contains("Added from the editor"))
        .await;
    let content = fake.client().cat("NOTE-PLANNING").await.unwrap();
    assert!(content.content.ends_with("Added from the editor\n"));
    assert!(h.app.toast_messages().iter().any(|m| m == "Saved to Bear."));
}

#[tokio::test]
async fn visual_beats_editor_and_config_beats_both() {
    let fake = Fake::new();
    let marker = fake.dir.path().join("which");
    let visual = fake_editor(
        fake.dir.path(),
        "visual.sh",
        &format!("printf visual > '{}'", marker.display()),
    );
    let mut h = fake.harness_env(env(&[
        ("EDITOR", "definitely-not-an-editor"),
        ("VISUAL", &visual),
    ]));
    h.load().await;
    h.press("e");
    h.until(|_| marker.exists()).await;
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "visual");
    let configured = fake_editor(
        fake.dir.path(),
        "configured.sh",
        &format!("printf config > '{}'", marker.display()),
    );
    let config = bjorn::config::Config {
        editor: configured,
        ..fake.config()
    };
    let mut h = bjorn::harness::Harness::with_env(
        config,
        std::sync::Arc::new(fake.client()),
        None,
        (120, 40),
        env(&[("VISUAL", &visual)]),
    );
    h.load().await;
    h.press("e");
    h.until(|_| std::fs::read_to_string(&marker).unwrap() == "config")
        .await;
}

#[tokio::test]
async fn unchanged_edit_writes_nothing() {
    let fake = Fake::new();
    let editor = fake_editor(fake.dir.path(), "editor.sh", "true");
    let before = fake.client().cat("NOTE-PLANNING").await.unwrap();
    let mut h = fake.harness_env(env(&[("EDITOR", &editor)]));
    h.load().await;
    h.press("e");
    h.until(|app| app.toast_messages().iter().any(|m| m == "No changes."))
        .await;
    assert_eq!(fake.client().cat("NOTE-PLANNING").await.unwrap(), before);
}

#[tokio::test]
async fn conflict_keeps_the_temp_file() {
    let fake = Fake::new();
    // The "editor" changes the note in Bear behind our back, then edits the file.
    let body = format!(
        "BJORN_FAKE_BEAR_STATE='{}' '{}' overwrite NOTE-PLANNING --content '# Sprint Planning\\n#work/sprint\\n\\nchanged in Bear\\n'\nprintf 'my edit\\n' >> \"$1\"",
        fake.state().display(),
        env!("CARGO_BIN_EXE_fake-bearcli")
    );
    let editor = fake_editor(fake.dir.path(), "editor.sh", &body);
    let mut h = fake.harness_env(env(&[("EDITOR", &editor)]));
    h.load().await;
    h.press("e");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("your version is at"))
    })
    .await;
    let message = h
        .app
        .toast_messages()
        .into_iter()
        .find(|m| m.contains("your version is at"))
        .unwrap();
    let kept = Path::new(message.rsplit("your version is at ").next().unwrap().trim());
    assert!(
        kept.exists(),
        "the edited file must survive a conflict: {message}"
    );
    assert!(
        std::fs::read_to_string(kept)
            .unwrap()
            .ends_with("my edit\n")
    );
    std::fs::remove_file(kept).unwrap();
    std::fs::remove_dir(kept.parent().unwrap()).unwrap();
    assert!(
        fake.client()
            .cat("NOTE-PLANNING")
            .await
            .unwrap()
            .content
            .ends_with("changed in Bear\n")
    );
}

#[tokio::test]
async fn missing_editor_is_reported_not_fatal() {
    let fake = Fake::new();
    let mut h = fake.harness_env(env(&[("EDITOR", "no-such-editor-xyz")]));
    h.load().await;
    h.press("e");
    assert!(h.app.running);
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("not found"))
    );
}

#[tokio::test]
async fn new_note_prompt_creates_and_opens_editor() {
    let fake = Fake::new();
    let editor = fake_editor(
        fake.dir.path(),
        "editor.sh",
        "printf 'first line\\n' >> \"$1\"",
    );
    let mut h = fake.harness_env(env(&[("EDITOR", &editor)]));
    h.load().await;
    h.press("n");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("NewNote"));
    h.type_text("Fresh");
    h.press("enter");
    h.type_text(",garden");
    h.press("enter");
    h.until(|app| app.notes.titles().contains(&"Fresh".to_string()))
        .await;
    h.until(|app| {
        app.reader.note.as_ref().is_some_and(|n| n.title == "Fresh")
            && app.reader.plain_text().contains("first line")
    })
    .await;
    let snap = fake.client().snapshot().await.unwrap();
    let note = snap.notes.iter().find(|n| n.title == "Fresh").unwrap();
    assert_eq!(note.tags, vec!["garden"]);
    assert!(
        fake.client()
            .cat(&note.id)
            .await
            .unwrap()
            .content
            .ends_with("first line\n")
    );
}

#[tokio::test]
async fn new_note_defaults_tags_to_selected_tag_or_workspace() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.move_to_tag("home");
    h.press("enter");
    h.until(|app| app.selection.tag == "home").await;
    h.press("n");
    match h.app.overlay.clone() {
        Some(bjorn::ui::modals::Overlay::NewNote { tags, .. }) => assert_eq!(tags.value, "home"),
        other => panic!("{other:?}"),
    }
    assert!(h.text().contains("New note"));
    h.press("escape");
    assert!(h.app.overlay.is_none());
    let mut h = fake.harness_with(fake.config(), Some("#work"));
    h.load().await;
    h.press("n");
    match h.app.overlay.clone() {
        Some(bjorn::ui::modals::Overlay::NewNote { tags, .. }) => assert_eq!(tags.value, "work"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn trash_needs_confirmation_then_restore() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("d");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    h.press("escape");
    assert_eq!(titles(&h)[0], "Sprint Planning");
    h.press("d");
    h.press("y");
    h.until(|app| !app.notes.titles().contains(&"Sprint Planning".to_string()))
        .await;
    h.press("7");
    h.until(|app| app.notes.titles() == vec!["Sprint Planning", "Old Draft"])
        .await;
    h.press("d");
    assert!(h.app.overlay.is_none(), "no trashing from the trash");
    h.press("u");
    h.until(|app| app.notes.titles() == vec!["Old Draft"]).await;
    h.press("1");
    h.until(|app| app.notes.titles().contains(&"Sprint Planning".to_string()))
        .await;
    let snap = fake.client().snapshot().await.unwrap();
    assert_eq!(
        snap.by_id("NOTE-PLANNING").unwrap().location,
        bjorn::bear::Location::Notes
    );
}

#[tokio::test]
async fn restore_from_archive_and_u_elsewhere_explains() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("u");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("Trash and Archive"))
    );
    h.press("6");
    h.until(|app| app.notes.titles() == vec!["Finished Project"])
        .await;
    h.press("u");
    h.until(|app| app.notes.titles().is_empty()).await;
    assert_eq!(h.app.selection.view, View::Archive);
    assert!(h.text().contains("No notes"));
    let snap = fake.client().snapshot().await.unwrap();
    assert_eq!(
        snap.by_id("NOTE-ARCHIVED").unwrap().location,
        bjorn::bear::Location::Notes
    );
}

#[tokio::test]
async fn p_toggles_global_pin_and_resorts() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    assert_eq!(titles(&h)[0], "Sprint Planning");
    h.press("p");
    h.until(|app| app.notes.titles()[0] == "Garden Plan").await;
    assert!(
        fake.client()
            .snapshot()
            .await
            .unwrap()
            .by_id("NOTE-PLANNING")
            .unwrap()
            .pins
            .is_empty()
    );
    assert_eq!(h.app.notes.current().unwrap().id, "NOTE-PLANNING");
    h.press("p");
    h.until(|app| app.notes.titles()[0] == "Sprint Planning")
        .await;
    assert!(
        fake.client()
            .snapshot()
            .await
            .unwrap()
            .by_id("NOTE-PLANNING")
            .unwrap()
            .pinned_globally()
    );
}

#[tokio::test]
async fn b_opens_the_note_in_bear() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("b");
    let log = fake.dir.path().join("bear.json.opened");
    h.until(|_| log.exists()).await;
    let last: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(last["id"], "NOTE-PLANNING");
}

fn prefill(h: &bjorn::harness::Harness) -> String {
    match h.app.overlay.clone() {
        Some(bjorn::ui::modals::Overlay::Text { field, .. }) => field.value,
        other => panic!("expected the path prompt, got {other:?}"),
    }
}

async fn export_via_picker(h: &mut bjorn::harness::Harness, key: &str) -> std::path::PathBuf {
    h.press("x");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Format"));
    h.press(key);
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Text"));
    let target = std::path::PathBuf::from(prefill(h));
    h.press("enter");
    h.until(|_| target.exists()).await;
    target
}

#[tokio::test]
async fn x_exports_to_the_prefilled_path_and_can_be_cancelled() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("x");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| o.name()),
        Some("Format"),
        "the picker comes first, Markdown preselected"
    );
    assert!(h.text().contains("Export as"));
    h.press("escape");
    assert!(!fake.config().export_dir.exists());
    h.press("x");
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Text"));
    let path = prefill(&h);
    assert_eq!(
        path,
        fake.config()
            .export_dir
            .join("Sprint Planning.md")
            .to_string_lossy()
    );
    h.press("enter");
    h.until(|_| Path::new(&path).exists()).await;
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Sprint Planning\n#work/sprint\n"));
    assert!(text.contains("- [ ] write the release notes"));
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.starts_with("Exported to"))
    })
    .await;
}

#[tokio::test]
async fn h_exports_self_contained_html_with_the_attachment_embedded() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    let written = export_via_picker(&mut h, "h").await;
    assert_eq!(written, fake.config().export_dir.join("Garden Plan.html"));
    let doc = std::fs::read_to_string(&written).unwrap();
    assert!(doc.starts_with("<!DOCTYPE html>") && doc.contains("<title>Garden Plan</title>"));
    assert!(
        doc.contains("<li class=\"task\"><input type=\"checkbox\" disabled> order bulbs"),
        "{doc}"
    );
    assert!(
        doc.contains("src=\"data:image/png;base64,iVBORw0KGgo"),
        "the attachment travels inside the file"
    );
    assert!(!doc.contains("Front%20bed.png"));
}

#[tokio::test]
async fn t_exports_plain_text_and_config_preselects() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let written = export_via_picker(&mut h, "t").await;
    assert_eq!(
        written,
        fake.config().export_dir.join("Sprint Planning.txt")
    );
    let text = std::fs::read_to_string(&written).unwrap();
    assert!(text.starts_with("Sprint Planning\n#work/sprint\n\nTasks\n- ☑ book the retro room\n- ☐ write the release notes\n"), "{text}");
    assert!(text.contains("Velocity is holding steady.") && !text.contains("=="));

    let config = bjorn::config::Config {
        export_format: "txt".into(),
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("x");
    let index = |h: &bjorn::harness::Harness| match h.app.overlay.clone() {
        Some(bjorn::ui::modals::Overlay::Format { index, .. }) => index,
        other => panic!("{other:?}"),
    };
    assert_eq!(bjorn::export::FORMATS[index(&h)].id, "txt");
    h.press("left");
    assert_eq!(bjorn::export::FORMATS[index(&h)].id, "html");
    h.press("enter");
    assert!(prefill(&h).ends_with("Sprint Planning.html"));
    h.press("escape");
    assert!(h.app.overlay.is_none());
}

#[tokio::test]
async fn r_exports_rtf_and_rtfd_when_the_note_has_images() {
    if bjorn::util::which("textutil").is_none() {
        return;
    }
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let written = export_via_picker(&mut h, "r").await;
    assert_eq!(
        written,
        fake.config().export_dir.join("Sprint Planning.rtf")
    );
    let rtf = String::from_utf8_lossy(&std::fs::read(&written).unwrap()).into_owned();
    assert!(
        rtf.starts_with("{\\rtf1")
            && rtf.contains("release notes")
            && rtf.contains("Sprint Planning")
    );
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    let package = export_via_picker(&mut h, "r").await;
    assert_eq!(package, fake.config().export_dir.join("Garden Plan.rtfd"));
    assert!(package.is_dir());
    let names: Vec<String> = std::fs::read_dir(&package)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "TXT.rtf") && names.iter().any(|n| n.ends_with(".png")),
        "{names:?}"
    );
}

#[tokio::test]
async fn b_exports_a_textbundle_with_assets_and_rewritten_links() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    let bundle = export_via_picker(&mut h, "b").await;
    assert_eq!(
        bundle,
        fake.config().export_dir.join("Garden Plan.textbundle")
    );
    assert!(bundle.is_dir());
    let info: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(bundle.join("info.json")).unwrap()).unwrap();
    assert_eq!(info["version"], 2);
    assert_eq!(info["type"], "net.daringfireball.markdown");
    assert_eq!(info["transient"], false);
    let text = std::fs::read_to_string(bundle.join("text.md")).unwrap();
    assert!(text.starts_with("# Garden Plan\n#home/garden\n"));
    assert!(text.contains("![](assets/Front%20bed.png)") && !text.contains("](Front%20bed.png)"));
    assert!(
        std::fs::read(bundle.join("assets/Front bed.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
}

#[tokio::test]
async fn editor_runs_inside_the_reader_pane() {
    let fake = Fake::new();
    // Draws a banner, waits for a line of input, appends it to the note.
    let editor = fake_editor(
        fake.dir.path(),
        "editor.sh",
        "printf 'EDITOR SCREEN %s' \"$COLUMNS$LINES\"; stty -echo 2>/dev/null; read -r line; printf '\\n%s\\n' \"$line\" >> \"$1\"",
    );
    let mut h = fake.harness_env(env(&[("EDITOR", &editor)]));
    h.load().await;
    h.press("e");
    h.until(|app| app.editing.is_some()).await;
    h.until(|app| {
        app.editing
            .as_ref()
            .is_some_and(|e| e.pty.contents().contains("EDITOR SCREEN"))
    })
    .await;
    h.draw();
    let text = h.text();
    // The editor's screen shows in the reader pane; the other columns stay up.
    assert!(text.contains("EDITOR SCREEN"), "{text}");
    assert!(text.contains("Editing · Sprint Planning"), "{text}");
    assert!(text.contains("Sprint Planning"), "{text}");
    assert!(text.contains("Keys go to the editor"), "{text}");
    // The pty was sized to the pane, not the whole terminal.
    let (cols, rows) = {
        let e = h.app.editing.as_ref().unwrap();
        let (rows, cols) = e.pty.size();
        (cols, rows)
    };
    assert!((20..120).contains(&cols), "cols {cols}");
    assert!((1..40).contains(&rows), "rows {rows}");
    // Keys go to the editor, not to bjorn: `q` does not open the quit prompt.
    h.press("q");
    assert!(h.app.overlay.is_none());
    for key in ["t", "y", "p", "e", "d", "enter"] {
        h.press(key);
    }
    h.until(|app| app.editing.is_none()).await;
    h.until(|app| app.reader.plain_text().contains("qtyped"))
        .await;
    let content = fake.client().cat("NOTE-PLANNING").await.unwrap();
    assert!(
        content.content.ends_with("qtyped\n"),
        "{:?}",
        content.content
    );
    assert!(!h.text().contains("Keys go to the editor"));
}
