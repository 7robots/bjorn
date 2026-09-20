//! Wiki links: drawn as links in the reader, the `L` list with its backlinks,
//! following by key and by click, creating a missing note, headings, the
//! workspace, and back / forward.

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use bjorn::app::{App, Pane};
use bjorn::harness::Harness;
use bjorn::model::View;
use bjorn::ui::modals::{LinkRow, LinkTarget, Overlay, filter_links};
use bjorn::ui::theme;
use common::Fake;
use crossterm::event::{KeyCode, KeyModifiers};

/// The reader shows note `id`, body and all.
fn reader_on(app: &App, id: &str) -> bool {
    app.reader
        .note
        .as_ref()
        .is_some_and(|n| n.id == id && app.reader.full_text.is_some())
}

fn reader_on_planning(app: &App) -> bool {
    reader_on(app, "NOTE-PLANNING")
}

/// The Links list is up and the backlink search has answered.
fn backlinks_loaded(app: &App) -> bool {
    matches!(
        &app.overlay,
        Some(Overlay::Links {
            backlinks: Some(_),
            ..
        })
    )
}

/// The Links overlay's rows, once the backlink search has answered.
fn link_lists(h: &Harness) -> (Vec<LinkRow>, Vec<LinkRow>) {
    match &h.app.overlay {
        Some(Overlay::Links {
            outgoing,
            backlinks: Some(backlinks),
            ..
        }) => (outgoing.clone(), backlinks.clone()),
        other => panic!("no Links list with backlinks: {other:?}"),
    }
}

fn labels(rows: &[LinkRow]) -> Vec<&str> {
    rows.iter().map(|r| r.label.as_str()).collect()
}

/// The reader cell where `needle` is drawn, searching the rows on screen.
fn reader_cell(h: &Harness, needle: &str) -> Option<(u16, u16)> {
    let body = h.app.rects.reader_body;
    (body.y..body.y + body.height).find_map(|y| {
        h.find_cell(y, needle, body.x, body.x + body.width)
            .map(|x| (x, y))
    })
}

fn fake_editor(dir: &Path, body: &str) -> String {
    let path = dir.join("editor.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn the_reader_draws_links_in_the_link_color_and_leaves_code_alone() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(|app| app.reader.full_text.is_some()).await;
    let text = h.app.reader.plain_text();
    assert!(text.contains("Keep the reading list short"), "{text}");
    assert!(text.contains("draft the Team Offsite agenda"), "{text}");
    assert!(!text.contains("[[Reading Queue"), "{text}");
    assert!(
        text.contains("Bear writes a link as [[Note title]]."),
        "a code span shows its brackets: {text}"
    );
    let (x, y) = reader_cell(&h, "the reading list").expect("the alias is drawn");
    assert_eq!(h.cell_fg(x, y), theme::current().link);
    let (x, y) = reader_cell(&h, "[[Note title]]").expect("code is drawn");
    assert_ne!(h.cell_fg(x + 2, y), theme::current().link);
}

#[tokio::test]
async fn l_lists_outgoing_links_and_verified_backlinks() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Links"));
    h.until(backlinks_loaded).await;
    let (outgoing, backlinks) = link_lists(&h);
    // The code span's `[[Note title]]` is not a link.
    assert_eq!(labels(&outgoing), vec!["the reading list", "Team Offsite"]);
    assert!(!outgoing[0].missing && outgoing[1].missing);
    assert!(outgoing[0].detail.contains("Reading Queue"));
    // Loose Thought mentions it only inside a fence, and Old Draft is in the
    // trash: bearcli's phrase search returns both, and both are dropped.
    assert_eq!(labels(&backlinks), vec!["Garden Plan", "Finished Project"]);
    assert_eq!(backlinks[0].detail, "› Notes");
    assert_eq!(backlinks[1].detail, "in the archive");
    let screen = h.text();
    assert!(screen.contains("Links from this note · 2"), "{screen}");
    assert!(screen.contains("Linked from · 2"), "{screen}");
    h.press("escape");
    assert!(h.app.overlay.is_none());
}

#[tokio::test]
async fn a_longer_title_that_starts_the_same_is_not_a_backlink() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("j");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    h.press("L");
    h.until(backlinks_loaded).await;
    let (outgoing, backlinks) = link_lists(&h);
    assert_eq!(labels(&outgoing), vec!["Sprint Planning › Notes"]);
    // Reading Queue links to [[Garden Plan 2027]], which the phrase search
    // for `[[Garden Plan` also finds.
    assert_eq!(labels(&backlinks), vec!["Loose Thought"]);
    assert_eq!(backlinks[0].detail, "› Next spring");
}

#[tokio::test]
async fn typing_filters_the_list_and_enter_follows_then_back_and_forward() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    h.until(backlinks_loaded).await;
    h.type_text("reading");
    let (outgoing, backlinks) = link_lists(&h);
    let (out, back) = filter_links(&outgoing, Some(&backlinks), "reading");
    assert_eq!(out.len(), 1);
    assert!(back.is_empty());
    assert!(matches!(&out[0].target, LinkTarget::Wiki(link) if link.title == "Reading Queue"));
    h.press("enter");
    assert!(h.app.overlay.is_none());
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    assert_eq!(h.app.focus, Pane::Reader);
    assert_eq!(h.app.back.len(), 1);

    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    assert_eq!(h.app.forward.len(), 1);
    h.key(KeyCode::Right, KeyModifiers::ALT);
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    h.key(KeyCode::Char('o'), KeyModifiers::CONTROL);
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    h.press("backspace");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m == "Nothing to go back to.")
    );
}

#[tokio::test]
async fn following_a_backlink_opens_that_note_in_its_own_view() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    h.until(backlinks_loaded).await;
    h.type_text("finished");
    h.press("enter");
    h.until(|app| reader_on(app, "NOTE-ARCHIVED")).await;
    assert_eq!(h.app.selection.view, View::Archive);
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    assert_eq!(h.app.selection.view, View::All);
}

#[tokio::test]
async fn clicking_a_link_in_the_reader_follows_it() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    let (x, y) = reader_cell(&h, "the reading list").expect("drawn");
    // Plain text beside it does nothing.
    h.click(x.saturating_sub(3), y);
    assert!(h.app.back.is_empty());
    h.click(x + 4, y);
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    assert_eq!(h.app.back.len(), 1);
    assert_eq!(h.app.back[0].id, "NOTE-PLANNING");
}

#[tokio::test]
async fn a_link_with_a_heading_scrolls_to_it() {
    let fake = Fake::new();
    let mut h = Harness::new(fake.config(), Arc::new(fake.client()), None, (120, 12));
    h.load().await;
    h.press("j");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    h.press("L");
    h.press("enter");
    h.until(|app| reader_on(app, "NOTE-PLANNING") && app.reader.scroll > 0)
        .await;
    let body = h.app.rects.reader_body;
    assert_eq!(
        reader_cell(&h, "Notes").map(|(_, y)| y),
        Some(body.y),
        "{}",
        h.text()
    );
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
}

#[tokio::test]
async fn a_missing_heading_is_reported() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_link(bjorn::wiki::WikiLink {
        title: "reading queue".into(),
        section: "Nowhere".into(),
        alias: String::new(),
        ..Default::default()
    });
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    h.settle().await;
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("No heading “Nowhere”")),
        "{:?}",
        h.app.toast_messages()
    );
}

#[tokio::test]
async fn a_link_to_a_missing_note_offers_to_create_it() {
    let fake = Fake::new();
    let editor = fake_editor(fake.dir.path(), "exit 0");
    let environ: HashMap<String, String> = [("EDITOR".to_string(), editor)].into();
    let mut h = fake.harness_env(environ);
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    h.type_text("offsite");
    h.press("enter");
    match &h.app.overlay {
        Some(Overlay::Confirm { message, .. }) => {
            assert!(message.contains("“Team Offsite”"), "{message}")
        }
        other => panic!("expected the confirm dialog: {other:?}"),
    }
    // Canceling writes nothing.
    h.press("n");
    assert!(h.app.overlay.is_none());
    assert!(h.app.resolve_title("Team Offsite").is_none());

    h.press("L");
    h.type_text("offsite");
    h.press("enter");
    h.press("y");
    h.until(|app| app.resolve_title("team offsite").is_some())
        .await;
    let snap = fake.client().snapshot().await.unwrap();
    assert!(snap.notes.iter().any(|n| n.title == "Team Offsite"));
    assert_eq!(
        h.app.back.last().map(|p| p.id.as_str()),
        Some("NOTE-PLANNING")
    );
}

#[tokio::test]
async fn following_out_of_the_workspace_clears_it_and_back_restores_it() {
    let fake = Fake::new();
    let mut h = fake.harness_with(fake.config(), Some("home"));
    h.load().await;
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    h.press("L");
    h.press("enter");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    assert_eq!(h.app.selection.workspace, "");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("is outside #home"))
    );
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    assert_eq!(h.app.selection.workspace, "home");
}

#[tokio::test]
async fn a_title_shared_by_several_notes_resolves_to_the_active_one() {
    let fake = Fake::new();
    let client = fake.client();
    let id = client
        .create("Finished Project", &[], "Second life.\n")
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    assert_eq!(
        h.app
            .resolve_title("finished project")
            .map(|n| n.id.clone()),
        Some(id)
    );
    assert_eq!(
        h.app.resolve_title("Old Draft").map(|n| n.id.as_str()),
        Some("NOTE-TRASHED"),
        "a note only in the trash still resolves"
    );
}

/// Remove a note from the fake's state file, as if it were deleted in Bear.
fn delete_from_fake(fake: &Fake, id: &str) {
    let path = fake.state();
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    state["notes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["id"] != id);
    std::fs::write(&path, state.to_string()).unwrap();
}

#[tokio::test]
async fn backspace_edits_the_search_and_the_list_filter_instead_of_going_back() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_note("NOTE-GARDEN");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    assert_eq!(h.app.back.len(), 1);

    // In the Links filter, backspace deletes a character.
    h.press("L");
    h.type_text("xy");
    h.press("backspace");
    match &h.app.overlay {
        Some(Overlay::Links { field, .. }) => assert_eq!(field.value, "x"),
        other => panic!("{other:?}"),
    }
    h.press("escape");

    // In the search box, and still after enter has moved focus to the list.
    h.press("slash");
    h.type_text("plan");
    h.press("backspace");
    assert_eq!(h.app.notes.search.value, "pla");
    h.press("enter");
    h.until(|app| app.search_query == "pla").await;
    assert_eq!(h.app.focus, Pane::Notes);
    h.press("backspace");
    h.settle().await;
    assert_eq!(h.app.back.len(), 1, "history untouched");
    assert!(h.app.forward.is_empty());
    assert_eq!(h.app.search_query, "pla");
    // Once the search is cleared, backspace goes back again.
    h.press("escape");
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
}

#[tokio::test]
async fn the_history_keeps_at_most_history_limit_places() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    for i in 0..bjorn::app::HISTORY_LIMIT + 5 {
        let id = if i % 2 == 0 {
            "NOTE-READING"
        } else {
            "NOTE-PLANNING"
        };
        h.app.follow_note(id);
        h.until(|app| reader_on(app, id)).await;
    }
    assert_eq!(h.app.back.len(), bjorn::app::HISTORY_LIMIT);
}

#[tokio::test]
async fn alt_arrows_and_alt_b_f_walk_the_history() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_note("NOTE-READING");
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    h.key(KeyCode::Left, KeyModifiers::ALT);
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    h.key(KeyCode::Char('f'), KeyModifiers::ALT);
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    h.key(KeyCode::Char('b'), KeyModifiers::ALT);
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    assert!(h.app.back.is_empty());
    assert_eq!(h.app.forward.len(), 1);
}

#[tokio::test]
async fn a_stale_backlink_answer_is_dropped_and_errors_are_shown() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    let current = h.app.backlinks_generation();
    // Opening the list again while its search runs does not start another.
    h.press("escape");
    h.press("L");
    assert_eq!(h.app.backlinks_generation(), current);

    let stale = bjorn::wiki::Backlinks {
        notes: vec![bjorn::wiki::Backlink {
            id: "NOTE-READING".into(),
            title: "From an old search".into(),
            location: bjorn::bear::Location::Notes,
            sections: Vec::new(),
        }],
        capped: false,
    };
    h.app.handle_msg(bjorn::app::Msg::Backlinks {
        generation: current - 1,
        note_id: "NOTE-PLANNING".into(),
        result: Ok(stale),
    });
    assert!(!backlinks_loaded(&h.app), "an older answer is ignored");

    h.app.handle_msg(bjorn::app::Msg::Backlinks {
        generation: current,
        note_id: "NOTE-PLANNING".into(),
        result: Err(bjorn::bear::BearError::new("bearcli timed out")),
    });
    h.draw();
    assert!(h.text().contains("bearcli timed out"), "{}", h.text());
    assert!(h.text().contains("Linked from · 0"), "{}", h.text());
}

#[tokio::test]
async fn a_capped_search_says_the_list_may_be_incomplete() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    h.app.handle_msg(bjorn::app::Msg::Backlinks {
        generation: h.app.backlinks_generation(),
        note_id: "NOTE-PLANNING".into(),
        result: Ok(bjorn::wiki::Backlinks {
            notes: Vec::new(),
            capped: true,
        }),
    });
    h.draw();
    assert!(
        h.text().contains("200+ candidates, list may be incomplete"),
        "{}",
        h.text()
    );
}

#[tokio::test]
async fn a_link_to_a_heading_on_the_page_scrolls_without_reloading() {
    let fake = Fake::new();
    let mut h = Harness::new(fake.config(), Arc::new(fake.client()), None, (120, 12));
    h.load().await;
    h.until(reader_on_planning).await;
    let renders = h.app.reader.renders;
    h.app.follow_link(bjorn::wiki::WikiLink {
        title: String::new(),
        section: "Notes".into(),
        alias: String::new(),
        ..Default::default()
    });
    h.draw();
    assert!(h.app.reader.scroll > 0);
    assert_eq!(
        h.app.reader.renders, renders,
        "the note was not loaded again"
    );
    assert_eq!(h.app.back.len(), 1);
    assert_eq!(h.app.back[0].scroll, 0);
    h.press("backspace");
    h.until(|app| app.reader.scroll == 0).await;
}

#[tokio::test]
async fn back_restores_the_search_and_the_scroll() {
    let fake = Fake::new();
    let mut h = Harness::new(fake.config(), Arc::new(fake.client()), None, (120, 12));
    h.load().await;
    h.press("slash");
    h.type_text("velocity");
    h.press("enter");
    h.until(|app| app.notes.titles() == vec!["Sprint Planning"])
        .await;
    h.until(reader_on_planning).await;
    h.app.focus = Pane::Reader;
    h.press("j");
    h.press("j");
    let scroll = h.app.reader.scroll;
    assert!(scroll > 0);
    h.app.follow_note("NOTE-READING");
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    assert_eq!(h.app.search_query, "", "the search was dropped to show it");
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    h.draw();
    assert_eq!(h.app.search_query, "velocity");
    assert!(h.app.notes.search.open);
    assert_eq!(h.app.notes.titles(), vec!["Sprint Planning"]);
    assert_eq!(h.app.reader.scroll, scroll);
}

#[tokio::test]
async fn back_skips_notes_deleted_since() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_note("NOTE-GARDEN");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    h.app.follow_note("NOTE-READING");
    h.until(|app| reader_on(app, "NOTE-READING")).await;
    delete_from_fake(&fake, "NOTE-GARDEN");
    h.press("r");
    h.until(|app| app.snapshot.by_id("NOTE-GARDEN").is_none())
        .await;
    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING")).await;
    assert!(h.app.back.is_empty());
    assert_eq!(h.app.forward.len(), 1);

    // With nothing alive behind, the stacks stay as they were.
    delete_from_fake(&fake, "NOTE-READING");
    h.press("r");
    h.until(|app| app.snapshot.by_id("NOTE-READING").is_none())
        .await;
    h.key(KeyCode::Right, KeyModifiers::ALT);
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m == "Nothing to go forward to.")
    );
    assert!(h.app.back.is_empty() && h.app.forward.is_empty());
}

#[tokio::test]
async fn a_click_on_the_second_row_of_a_wrapped_link_follows_it() {
    let mut reader = bjorn::ui::note_view::Reader::default();
    let note = bjorn::bear::Note {
        id: "X".into(),
        title: "X".into(),
        ..Default::default()
    };
    reader.show(&note, "aaa [[Target|link text here]] bbb");
    let width = 12;
    assert!(reader.row_count(width) >= 2);
    let second = reader.link_at(1, 1, width).expect("row two holds the link");
    assert_eq!(second.title, "Target");
    assert_eq!(reader.link_at(1, 0, width), None, "aaa is plain text");
}

#[tokio::test]
async fn links_in_tables_and_quoted_headings_work() {
    let mut reader = bjorn::ui::note_view::Reader::default();
    let note = bjorn::bear::Note::default();
    reader.show(
        &note,
        "| a | b |\n|---|---|\n| [[One|uno]] | two |\n\n> ## Quoted heading\n",
    );
    // Row 0 is the header, 1 the rule, 2 the first body row.
    let link = reader.link_at(0, 2, 40).expect("the cell's link");
    assert_eq!(link.title, "One");
    assert!(reader.scroll_to_heading("quoted heading", 40));
}

#[tokio::test]
async fn a_title_that_looks_like_an_option_stays_a_title() {
    let fake = Fake::new();
    let editor = fake_editor(fake.dir.path(), "exit 0");
    let environ: HashMap<String, String> = [("EDITOR".to_string(), editor)].into();
    let mut h = fake.harness_env(environ);
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_link(bjorn::wiki::WikiLink {
        title: "--content=x".into(),
        ..Default::default()
    });
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    h.press("y");
    h.until(|app| app.resolve_title("--content=x").is_some())
        .await;
    let snap = fake.client().snapshot().await.unwrap();
    assert!(snap.notes.iter().any(|n| n.title == "--content=x"));

    // The fake, like bearcli, refuses a hyphen-led title without `--`.
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_fake-bearcli"))
        .args(["create", "-x"])
        .env("BJORN_FAKE_BEAR_STATE", fake.state())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
        .status;
    assert!(!status.success());
}

#[tokio::test]
async fn the_list_holds_at_most_outgoing_limit_links_and_keeps_alias_targets_visible() {
    let fake = Fake::new();
    let alias = "a very long alias that would fill the whole row of the list by itself ".repeat(3);
    let mut body = format!("# Many\n\n[[Reading Queue|{alias}]]\n\n");
    for i in 0..bjorn::app::OUTGOING_LIMIT + 4 {
        body.push_str(&format!("[[Missing {i}]] "));
    }
    let id = fake.client().create("Many", &[], &body).await.unwrap();
    let mut h = fake.harness();
    h.load().await;
    h.app.follow_note(&id);
    h.until(|app| reader_on(app, &id)).await;
    h.press("L");
    match &h.app.overlay {
        Some(Overlay::Links { outgoing, more, .. }) => {
            assert_eq!(outgoing.len(), bjorn::app::OUTGOING_LIMIT);
            assert_eq!(*more, 5);
        }
        other => panic!("{other:?}"),
    }
    assert!(h.text().contains("→ Reading Queue"), "{}", h.text());
}

#[tokio::test]
async fn escaped_and_doubly_escaped_titles_resolve_and_find_their_backlinks() {
    let fake = Fake::new();
    let client = fake.client();
    let session = client
        .create("Session #1", &[], "# Session #1\n\nNotes.\n")
        .await
        .unwrap();
    let cloud = client
        .create(
            "Cloud Arch / EA",
            &[],
            "# Cloud Arch / EA\n\n## Apr 19\n\nMet.\n",
        )
        .await
        .unwrap();
    // As Bear writes them: `\#`, and the doubled `\\/` seen in real libraries.
    let linker = client
        .create(
            "Linker",
            &[],
            "# Linker\n\nSee [[Session \\#1]] and [[Cloud Arch \\\\/ EA/Apr 19]].\n",
        )
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    h.app.follow_note(&linker);
    h.until(|app| reader_on(app, &linker)).await;
    h.press("L");
    h.until(backlinks_loaded).await;
    let (outgoing, _) = link_lists(&h);
    assert_eq!(
        labels(&outgoing),
        vec!["Session #1", "Cloud Arch / EA › Apr 19"]
    );
    assert!(outgoing.iter().all(|r| !r.missing), "{outgoing:?}");
    h.press("down");
    h.press("enter");
    assert!(h.app.overlay.is_none(), "no offer to create a note");
    h.until(|app| reader_on(app, &cloud)).await;

    // The note it lands on finds the link back, written with `\\/`.
    h.press("L");
    h.until(backlinks_loaded).await;
    let (_, backlinks) = link_lists(&h);
    assert_eq!(labels(&backlinks), vec!["Linker"]);
    assert_eq!(backlinks[0].detail, "› Apr 19");
    h.press("escape");
    h.app.follow_note(&session);
    h.until(|app| reader_on(app, &session)).await;
    h.press("L");
    h.until(backlinks_loaded).await;
    let (_, backlinks) = link_lists(&h);
    assert_eq!(labels(&backlinks), vec!["Linker"]);
}

#[tokio::test]
async fn a_jump_for_a_note_left_behind_is_dropped() {
    let fake = Fake::new();
    let mut h = Harness::new(fake.config(), Arc::new(fake.client()), None, (120, 12));
    h.load().await;
    h.press("j");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    // Nothing cached, so the target loads in the background.
    h.app.forget_content(None);
    h.app.follow_link(bjorn::wiki::WikiLink {
        title: "Sprint Planning".into(),
        section: "Notes".into(),
        ..Default::default()
    });
    // Before it arrives, the reader moves on to another note...
    let garden = h.app.snapshot.by_id("NOTE-GARDEN").cloned().unwrap();
    h.app.notes.select_id("NOTE-GARDEN");
    h.app.schedule_preview(garden, true, false);
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    // ...so opening Sprint Planning later starts at its top.
    let planning = h.app.snapshot.by_id("NOTE-PLANNING").cloned().unwrap();
    h.app.notes.select_id("NOTE-PLANNING");
    h.app.schedule_preview(planning, true, false);
    h.until(reader_on_planning).await;
    h.settle().await;
    assert_eq!(h.app.reader.scroll, 0);
}

#[tokio::test]
async fn a_list_opened_for_another_note_waits_for_the_running_search() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.press("L");
    let first = h.app.backlinks_generation();
    h.press("escape");
    h.press("j");
    assert!(
        reader_on(&h.app, "NOTE-GARDEN"),
        "a cached body shows at once"
    );
    h.press("L");
    assert_eq!(
        h.app.backlinks_generation(),
        first,
        "queued behind the running search"
    );
    h.until(backlinks_loaded).await;
    assert_eq!(h.app.backlinks_generation(), first + 1);
    let (_, backlinks) = link_lists(&h);
    assert_eq!(labels(&backlinks), vec!["Loose Thought"]);
}

#[tokio::test]
async fn backspace_goes_on_back_past_a_restored_search() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.until(reader_on_planning).await;
    h.app.follow_note("NOTE-GARDEN");
    h.until(|app| reader_on(app, "NOTE-GARDEN")).await;
    h.press("slash");
    h.type_text("velocity");
    h.press("enter");
    h.until(|app| app.notes.titles() == vec!["Sprint Planning"] && reader_on(app, "NOTE-PLANNING"))
        .await;
    h.app.follow_note("NOTE-READING");
    h.until(|app| reader_on(app, "NOTE-READING")).await;

    h.press("backspace");
    h.until(|app| reader_on(app, "NOTE-PLANNING") && app.search_query == "velocity")
        .await;
    assert!(h.app.notes.search.open);
    // The box only shows what going back put there: backspace goes on.
    // Back to where Garden Plan was followed from: all notes, no search.
    h.press("backspace");
    h.until(|app| app.search_query.is_empty() && app.back.is_empty())
        .await;
    assert!(reader_on(&h.app, "NOTE-PLANNING"));
    assert!(!h.app.notes.search.open);
    // Once the user types in a restored box, backspace edits it again.
    h.key(KeyCode::Right, KeyModifiers::ALT);
    h.until(|app| app.search_query == "velocity").await;
    h.press("slash");
    h.press("backspace");
    assert_eq!(h.app.notes.search.value, "velocit");
    assert_eq!(h.app.forward.len(), 1);
}
