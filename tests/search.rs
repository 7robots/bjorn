//! Search through bearcli, the box's completion, and match highlighting.

mod common;

use bjorn::app::Pane;
use common::Fake;

async fn search(h: &mut bjorn::harness::Harness, query: &str) {
    h.press("slash");
    h.type_text(query);
    h.press("enter");
}

#[tokio::test]
async fn slash_search_and_escape() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("slash");
    assert_eq!(h.app.focus, Pane::Search);
    assert!(
        h.text().contains("\"phrase\"  -term  #tag"),
        "the hint row shows under the box"
    );
    h.type_text("bulbs");
    h.press("enter");
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    assert_eq!(h.app.notes.header, "“bulbs” · 1");
    assert_eq!(h.app.focus, Pane::Notes);
    assert!(h.app.notes.search.open, "the box stays open with the query");
    h.press("escape");
    h.until(|app| app.notes.titles().len() == 5).await;
    assert!(!h.app.notes.search.open);
    assert!(!h.text().contains("\"phrase\"  -term"));
}

#[tokio::test]
async fn search_respects_workspace_and_view() {
    let fake = Fake::new();
    let mut h = fake.harness_with(fake.config(), Some("work"));
    h.load().await;
    search(&mut h, "@todo").await;
    h.until(|app| app.notes.titles() == vec!["Sprint Planning"])
        .await;
    h.press("7");
    h.until(|app| app.notes.titles() == vec!["Old Draft"]).await;
    assert_eq!(h.app.search_query, "");
}

#[tokio::test]
async fn typing_in_search_does_not_trigger_bindings() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("slash");
    h.type_text("dnq");
    assert!(h.app.running);
    assert_eq!(h.app.notes.search.value, "dnq");
    assert!(h.app.overlay.is_none());
    h.press("escape");
}

#[tokio::test]
async fn at_completes_and_right_accepts_and_enter_runs() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("slash");
    h.type_text("@to");
    assert_eq!(h.app.notes.search.suggestion.as_deref(), Some("@todo"));
    assert!(h.text().contains("@todo"), "ghost text is drawn");
    h.press("right");
    assert_eq!(h.app.notes.search.value, "@todo");
    h.press("enter");
    h.until(|app| app.search_query == "@todo").await;
    h.until(|app| {
        let mut t = app.notes.titles();
        t.sort();
        t == vec!["Garden Plan", "Sprint Planning"]
    })
    .await;
}

#[tokio::test]
async fn hash_completes_from_the_snapshot_and_workspace_tags_come_first() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("slash");
    h.type_text("bulbs #ho");
    assert_eq!(
        h.app.notes.search.suggestion.as_deref(),
        Some("bulbs #home")
    );
    h.type_text("me/");
    assert_eq!(
        h.app.notes.search.suggestion.as_deref(),
        Some("bulbs #home/garden")
    );
    h.press("escape");
    let mut h = fake.harness_with(fake.config(), Some("home"));
    h.load().await;
    assert_eq!(h.app.query_tags()[0], "home");
    assert!(h.app.query_tags().last().unwrap().starts_with("work"));
}

#[tokio::test]
async fn tab_accepts_the_completion_and_otherwise_moves_focus() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("slash");
    h.type_text("@to");
    h.press("tab");
    assert_eq!(h.app.notes.search.value, "@todo");
    assert_eq!(h.app.focus, Pane::Search);
    h.press("tab");
    assert_ne!(h.app.focus, Pane::Search);
}

#[tokio::test]
async fn bare_subtag_search_is_rewritten_and_finds_the_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "#garden").await;
    h.until(|app| app.search_query == "#*/garden").await;
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    assert_eq!(h.app.notes.search.value, "#*/garden");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("Sub-tag search"))
    );
}

// -- highlighting ---------------------------------------------------------------

#[tokio::test]
async fn body_match_is_highlighted_in_the_reader() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "bulbs").await;
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
            && app.reader.matches.len() == 1
    })
    .await;
    assert!(
        h.app.reader.header.contains("1 match"),
        "{}",
        h.app.reader.header
    );
    let body = h.app.rects.reader_body;
    let hit = (body.y..body.y + body.height).find_map(|y| {
        h.find_cell(y, "bulbs", body.x, body.x + body.width)
            .map(|x| (x, y))
    });
    let (x, y) = hit.expect("the term is on the page");
    assert!(
        h.cell_is_match(x, y),
        "the term is drawn in the match style\n{}",
        h.text()
    );
    assert!(
        !h.cell_is_match(x.saturating_sub(1), y),
        "only the term is styled"
    );
}

#[tokio::test]
async fn brackets_jump_between_matching_blocks() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "the").await;
    h.until(|app| app.notes.titles().contains(&"Garden Plan".to_string()))
        .await;
    h.app.notes.select_id("NOTE-GARDEN");
    let note = h.app.notes.current().unwrap().clone();
    h.app.schedule_preview(note, true, false);
    // Three blocks hold "the": the bulbs item, the roses item and the hydrangea item.
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
            && app.reader.matches.len() == 3
    })
    .await;
    assert!(h.app.reader.header.contains("3 matches"));
    h.press("]");
    assert_eq!(h.app.reader.match_index, 0);
    assert_eq!(h.app.focus, Pane::Reader);
    assert!(h.app.reader.header.contains("match 1/3"));
    h.press("]");
    assert_eq!(h.app.reader.match_index, 1);
    assert!(h.app.reader.header.contains("match 2/3"));
    h.press("[");
    assert_eq!(h.app.reader.match_index, 0);
    h.press("[");
    assert_eq!(h.app.reader.match_index, 2, "wraps");
}

#[tokio::test]
async fn enter_from_the_list_lands_on_the_first_match() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "roses").await;
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    h.until(|app| app.reader.matches.len() == 1).await;
    assert_eq!(h.app.focus, Pane::Notes);
    h.press("enter");
    assert_eq!(h.app.reader.match_index, 0);
    assert_eq!(h.app.focus, Pane::Reader);
    assert!(h.app.reader.header.contains("match 1/1"));
}

#[tokio::test]
async fn escape_clears_the_highlights() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "bulbs").await;
    h.until(|app| app.reader.matches.len() == 1).await;
    h.press("escape");
    h.until(|app| app.reader.pattern.is_none()).await;
    assert!(h.app.reader.matches.is_empty());
    assert!(!h.app.reader.header.contains("match"));
    assert!(h.app.notes.pattern.is_none());
}

#[tokio::test]
async fn jump_scrolls_a_long_note_to_the_match() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "119").await;
    h.until(|app| app.notes.titles() == vec!["Reading Queue"])
        .await;
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-READING")
            && app.reader.matches.len() == 1
    })
    .await;
    h.press("]");
    assert_eq!(h.app.reader.match_index, 0);
    assert!(h.app.reader.scroll > 0);
    assert!(h.text().contains("Book 119"));
}

#[tokio::test]
async fn rows_highlight_terms_in_title_and_preview_only() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    search(&mut h, "bulbs").await;
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    h.draw();
    let rows = h.app.rects.notes_rows;
    let x = h
        .find_cell(rows.y + 1, "bulbs", rows.x, rows.x + rows.width)
        .expect("preview shows the term");
    assert!(h.cell_is_match(x, rows.y + 1));
    h.press("escape");
    h.until(|app| app.notes.titles().len() == 5).await;
    search(&mut h, "garden").await;
    h.until(|app| {
        app.notes.header.starts_with("“garden”")
            && app.notes.titles().contains(&"Garden Plan".to_string())
    })
    .await;
    let rows = h.app.rects.notes_rows;
    let x = h
        .find_cell(rows.y, "Garden", rows.x, rows.x + rows.width)
        .unwrap_or_else(|| panic!("{}", h.text()));
    assert!(
        h.cell_is_match(x, rows.y),
        "title matches case-insensitively"
    );
    h.press("escape");
    h.until(|app| app.notes.titles().len() == 5).await;
    search(&mut h, "119").await;
    h.until(|app| app.notes.titles() == vec!["Reading Queue"])
        .await;
    let rows = h.app.rects.notes_rows;
    for y in rows.y..rows.y + 3 {
        for x in rows.x..rows.x + rows.width {
            assert!(
                !h.cell_is_match(x, y),
                "a body-only match shows no highlight in the row"
            );
        }
    }
    h.press("escape");
    h.until(|app| app.notes.titles().len() == 5).await;
    assert!(h.app.notes.pattern.is_none());
}
