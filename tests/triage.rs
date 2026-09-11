//! The triage screen against the fake bearcli, and Reminders mode against the fake remctl.

mod common;

use std::sync::Arc;

use bjorn::app::Pane;
use bjorn::config::{Config, RemindersConfig};
use bjorn::harness::Harness;
use bjorn::reminders::{RemctlClient, Status};
use bjorn::todos::Todo;
use common::Fake;

fn remctl(fake: &Fake) -> RemctlClient {
    RemctlClient::with_env(
        vec![env!("CARGO_BIN_EXE_fake-remctl").to_string()],
        vec![(
            "BJORN_FAKE_REMCTL_STATE".to_string(),
            fake.dir
                .path()
                .join("reminders.json")
                .to_string_lossy()
                .into_owned(),
        )],
    )
}

fn reminders_harness(fake: &Fake) -> Harness {
    let config = Config {
        reminders: RemindersConfig {
            enabled: true,
            list: "Work".into(),
            due: "today".into(),
            remctl: String::new(),
        },
        ..fake.config()
    };
    let mut h = Harness::new(config, Arc::new(fake.client()), None, (120, 40));
    h.app.remctl = Some(Arc::new(remctl(fake)));
    h
}

async fn open_triage(h: &mut Harness) {
    h.press("t");
    h.until(|app| {
        app.triage
            .as_ref()
            .is_some_and(|t| t.loaded && !t.rows.is_empty())
    })
    .await;
}

fn texts(h: &Harness) -> Vec<String> {
    h.app
        .triage
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.todo.text.clone())
        .collect()
}

fn current(h: &Harness) -> String {
    h.app
        .triage
        .as_ref()
        .unwrap()
        .current_row()
        .unwrap()
        .todo
        .text
        .clone()
}

#[tokio::test]
async fn t_lists_open_todos_grouped_and_scoped() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_triage(&mut h).await;
    assert_eq!(
        texts(&h),
        vec![
            "write the release notes",
            "ask Priya about the API deprecation",
            "confirm the sunset date",
            "order bulbs for the front bed",
            "move the hydrangea"
        ]
    );
    let triage = h.app.triage.as_ref().unwrap();
    assert_eq!(triage.notes, 2);
    assert!(
        triage.status.starts_with("5 open · 2 notes"),
        "{}",
        triage.status
    );
    assert_eq!(triage.header(), "TRIAGE · all notes");
    assert_eq!(current(&h), "write the release notes");
    let screen = h.text();
    assert!(
        screen.contains("TRIAGE · all notes")
            && screen.contains("Sprint Planning")
            && screen.contains("☐ write the release notes"),
        "{screen}"
    );
    assert!(screen.contains("Tick in Bear") && !screen.contains(" n New"));
    // main-screen keys are disabled while triage is up: e must not open an editor
    h.press("e");
    assert!(h.app.triage.is_some() && h.app.overlay.is_none());
    h.press("escape");
    assert!(h.app.triage.is_none());
}

#[tokio::test]
async fn workspace_scopes_the_triage() {
    let fake = Fake::new();
    let mut h = fake.harness_with(fake.config(), Some("home"));
    h.load().await;
    open_triage(&mut h).await;
    assert_eq!(
        texts(&h),
        vec!["order bulbs for the front bed", "move the hydrangea"]
    );
    assert!(h.app.triage.as_ref().unwrap().header().contains("#home"));
}

#[tokio::test]
async fn cursor_mark_and_filter() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_triage(&mut h).await;
    h.press("j");
    assert_eq!(current(&h), "ask Priya about the API deprecation");
    h.press("space");
    h.press("j");
    h.press("space");
    let marked: Vec<String> = h
        .app
        .triage
        .as_ref()
        .unwrap()
        .marked()
        .iter()
        .map(|r| r.todo.text.clone())
        .collect();
    assert_eq!(
        marked,
        vec![
            "ask Priya about the API deprecation",
            "confirm the sunset date"
        ]
    );
    assert!(h.app.triage.as_ref().unwrap().status.contains("2 marked"));
    assert!(h.text().contains("● "));
    h.press("slash");
    h.type_text("hydra");
    h.press("enter");
    assert_eq!(
        h.app.triage.as_ref().unwrap().visible_texts(),
        vec!["move the hydrangea"]
    );
    h.press("escape"); // clears the filter first
    assert_eq!(h.app.triage.as_ref().unwrap().visible_texts().len(), 5);
    assert!(h.app.triage.is_some());
    assert_eq!(
        h.app.triage.as_ref().unwrap().marked().len(),
        2,
        "marks survive a rebuild"
    );
}

#[tokio::test]
async fn x_ticks_in_bear_and_reloads_counts() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    assert_eq!(h.app.snapshot.by_id("NOTE-PLANNING").unwrap().todos, 3);
    open_triage(&mut h).await;
    h.press("x"); // highlighted row only, no confirm
    h.until(|app| {
        app.triage.as_ref().is_some_and(|t| {
            !t.rows
                .iter()
                .any(|r| r.todo.text == "write the release notes")
        })
    })
    .await;
    assert!(
        fake.client()
            .cat("NOTE-PLANNING")
            .await
            .unwrap()
            .content
            .contains("- [x] write the release notes")
    );
    h.until(|app| {
        app.snapshot
            .by_id("NOTE-PLANNING")
            .is_some_and(|n| n.todos == 2)
    })
    .await;
    // two marked rows ask first; escape declines
    h.until(|app| app.triage.as_ref().is_some_and(|t| t.rows.len() == 4))
        .await;
    h.press("space");
    h.press("j");
    h.press("space");
    h.press("x");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    assert!(h.text().contains("Tick 2 todos in Bear?"));
    h.press("escape");
    assert_eq!(texts(&h).len(), 4);
    h.press("x");
    h.press("y");
    h.until(|app| app.triage.as_ref().is_some_and(|t| t.rows.len() == 2))
        .await;
    let content = fake.client().cat("NOTE-PLANNING").await.unwrap().content;
    assert!(
        content.contains("- [x] ask Priya") && content.contains("  - [x] confirm the sunset date")
    );
}

#[tokio::test]
async fn stale_line_is_reported_not_fatal() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_triage(&mut h).await;
    fake.client()
        .edit(
            "NOTE-PLANNING",
            "- [ ] write the release notes",
            "- [ ] write the release notes tomorrow",
            "",
        )
        .await
        .unwrap();
    h.press("x");
    h.until(|app| {
        app.triage.as_ref().is_some_and(|t| {
            t.rows
                .iter()
                .any(|r| r.todo.text == "write the release notes tomorrow")
        })
    })
    .await;
    assert!(h.app.running);
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("no longer in the note"))
    );
    assert!(
        fake.client()
            .cat("NOTE-PLANNING")
            .await
            .unwrap()
            .content
            .contains("- [ ] write the release notes tomorrow")
    );
}

#[tokio::test]
async fn enter_goes_to_the_note_and_b_opens_bear_at_the_section() {
    let fake = Fake::new();
    let mut h = fake.harness_with(fake.config(), Some("home"));
    h.load().await;
    open_triage(&mut h).await;
    h.press("j");
    assert_eq!(current(&h), "move the hydrangea");
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
    assert_eq!(
        last,
        serde_json::json!({"id": "NOTE-GARDEN", "header": "Next spring"})
    );
    h.press("enter");
    assert!(h.app.triage.is_none());
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    assert_eq!(h.app.focus, Pane::Notes);
}

#[tokio::test]
async fn goto_a_note_outside_the_current_list() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("2");
    h.until(|app| app.notes.titles() == vec!["Loose Thought"])
        .await;
    open_triage(&mut h).await;
    h.press("enter");
    assert!(h.app.triage.is_none());
    h.until(|app| app.notes.current().is_some_and(|n| n.id == "NOTE-PLANNING"))
        .await;
    assert_eq!(h.app.selection.view, bjorn::model::View::All);
}

#[tokio::test]
async fn a_ticked_row_leaves_the_list_before_the_next_key() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_triage(&mut h).await;
    h.press("x");
    h.until(|app| {
        app.triage.as_ref().is_some_and(|t| {
            !t.rows
                .iter()
                .any(|r| r.todo.text == "write the release notes")
        })
    })
    .await;
    let triage = h.app.triage.as_ref().unwrap();
    assert_eq!(triage.visible_texts(), texts(&h));
    assert_eq!(
        current(&h),
        "ask Priya about the API deprecation",
        "the cursor moves down to the next surviving todo"
    );
    h.press("space");
    let marked: Vec<String> = h
        .app
        .triage
        .as_ref()
        .unwrap()
        .marked()
        .iter()
        .map(|r| r.todo.text.clone())
        .collect();
    assert_eq!(marked, vec!["ask Priya about the API deprecation"]);
}

// -- Reminders mode ----------------------------------------------------------------

#[tokio::test]
async fn a_adds_marked_rows_and_status_glyphs_follow() {
    let fake = Fake::new();
    let remctl = remctl(&fake);
    let mut h = reminders_harness(&fake);
    h.load().await;
    open_triage(&mut h).await;
    assert!(h.app.triage.as_ref().unwrap().reminders_enabled);
    h.press("space");
    h.press("j");
    h.press("space");
    h.press("a");
    h.until(|app| {
        app.triage
            .as_ref()
            .is_some_and(|t| t.rows.iter().filter(|r| r.status == Status::Added).count() == 2)
    })
    .await;
    let linked = remctl.linked_reminders().await.unwrap();
    let mut titles: Vec<&str> = linked.iter().map(|r| r.title.as_str()).collect();
    titles.sort();
    assert_eq!(
        titles,
        vec![
            "ask Priya about the API deprecation",
            "write the release notes"
        ]
    );
    assert!(linked.iter().all(|r| r.list_name == "Work"));
    assert!(h.app.triage.as_ref().unwrap().marked().is_empty());
    assert!(
        h.app
            .triage
            .as_ref()
            .unwrap()
            .status
            .contains("reminders: 2 added, 0 completed")
    );
    assert!(h.text().contains("⏰"));
    // complete one in "Reminders", reload, see it done, tick it in Bear
    remctl
        .run(&["done", &linked[0].id.to_string()])
        .await
        .unwrap();
    h.press("r");
    h.until(|app| {
        app.triage
            .as_ref()
            .is_some_and(|t| t.rows.iter().any(|r| r.status == Status::Done))
    })
    .await;
    let done_row = h
        .app
        .triage
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .find(|r| r.status == Status::Done)
        .unwrap()
        .clone();
    assert!(
        h.app
            .triage
            .as_mut()
            .unwrap()
            .select_key(&done_row.todo.key())
    );
    h.draw();
    assert!(h.text().contains("✓"));
    h.press("x");
    h.until(|app| {
        app.triage
            .as_ref()
            .is_some_and(|t| !t.rows.iter().any(|r| r.todo.key() == done_row.todo.key()))
    })
    .await;
    let content = fake.client().cat("NOTE-PLANNING").await.unwrap().content;
    assert!(content.contains(&format!("- [x] {}", done_row.todo.text)));
}

#[tokio::test]
async fn a_only_adds_rows_without_a_reminder() {
    let fake = Fake::new();
    let remctl = remctl(&fake);
    let mut h = reminders_harness(&fake);
    h.load().await;
    open_triage(&mut h).await;
    h.press("a"); // highlighted row, unmarked
    h.until(|app| {
        app.triage
            .as_ref()
            .is_some_and(|t| t.rows[0].status == Status::Added)
    })
    .await;
    h.press("a"); // same row again: nothing new
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("Mark rows that are not in Reminders yet"))
    })
    .await;
    assert_eq!(remctl.linked_reminders().await.unwrap().len(), 1);
}

#[tokio::test]
async fn reminders_off_by_default_and_missing_remctl_degrades() {
    let fake = Fake::new();
    let mut h = fake.harness();
    assert!(!h.app.reminders_enabled());
    h.load().await;
    open_triage(&mut h).await;
    assert!(!h.app.triage.as_ref().unwrap().reminders_enabled);
    h.press("a");
    assert!(h.app.triage.is_some());
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("Reminders mode is off"))
    );
    let config = Config {
        reminders: RemindersConfig {
            enabled: true,
            remctl: "/nonexistent/remctl".into(),
            ..RemindersConfig::default()
        },
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    assert!(h.app.remctl.is_none() && !h.app.reminders_enabled());
    h.load().await;
    h.press("t");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("remctl was not found"))
    );
}

#[tokio::test]
async fn remctl_client_add_and_search_against_fake() {
    let fake = Fake::new();
    let remctl = remctl(&fake);
    let t1 = Todo::new(
        "N1",
        "Sprint",
        &["work"],
        "write the notes",
        "- [ ] write the notes",
        "## Tasks",
    );
    let t2 = Todo::new(
        "N1",
        "Sprint",
        &["work"],
        "ship it",
        "- [ ] ship it",
        "## Tasks",
    );
    assert!(remctl.linked_reminders().await.unwrap().is_empty());
    assert_eq!(remctl.add(&t1, "Work", "today").await.unwrap(), Some(1));
    let linked = remctl.linked_reminders().await.unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(
        (
            linked[0].id,
            linked[0].title.as_str(),
            linked[0].key.as_str(),
            linked[0].list_name.as_str(),
            linked[0].completed
        ),
        (1, "write the notes", t1.key().as_str(), "Work", false)
    );
    remctl.run(&["done", "1"]).await.unwrap();
    assert!(remctl.linked_reminders().await.unwrap()[0].completed);
    assert!(remctl.add(&t2, "Nope", "").await.is_err());
    assert!(remctl.add(&t2, "", "whenever").await.is_err());
}
