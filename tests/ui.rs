//! The read-only TUI against the fake bearcli: layout, navigation, workspace,
//! folds, views, poll, columns, help and quit.

mod common;

use std::time::Duration;

use bjorn::app::Pane;
use bjorn::model::View;
use bjorn::render::OPEN_BOX;
use bjorn::ui::sidebar::Row;
use bjorn::ui::theme;
use common::{Fake, titles};

#[tokio::test]
async fn three_columns_load_and_render_first_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    assert_eq!(
        titles(&h),
        vec![
            "Sprint Planning",
            "Garden Plan",
            "Reading Queue",
            "CAD and Design",
            "Loose Thought"
        ]
    );
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-PLANNING")
    })
    .await;
    assert_eq!(h.app.reader.header, "Sprint Planning");
    let text = h.app.reader.plain_text();
    assert!(text.contains(OPEN_BOX), "{text}");
    assert!(text.contains("Velocity is holding steady."), "{text}");
    assert!(text.contains("#work/sprint"), "{text}");
    let screen = h.text();
    assert!(screen.contains("BJORN"));
    assert!(screen.contains("Notes · 5"));
    assert!(screen.contains("Sprint Planning"));
    assert!(screen.contains("TAGS"));
    assert!(screen.contains("▮▮▮"));
}

#[tokio::test]
async fn moving_the_cursor_changes_the_note() {
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
    h.press("k");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-PLANNING")
    })
    .await;
}

#[tokio::test]
async fn long_note_renders_in_full_and_scrolls() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.notes.select_id("NOTE-READING");
    h.press("j");
    h.press("k");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-READING")
    })
    .await;
    assert!(
        !h.app.reader.header.contains("lines"),
        "nothing is truncated any more"
    );
    assert!(h.app.reader.plain_text().contains("Book 120"));
    h.press("enter");
    assert_eq!(h.app.focus, Pane::Reader);
    h.press("end");
    h.draw();
    assert!(h.text().contains("Book 120"), "{}", h.text());
    h.press("home");
    h.draw();
    assert!(h.text().contains("Book 1"));
}

#[tokio::test]
async fn sidebar_counts_match_the_snapshot() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let c = &h.app.sidebar.view_counts;
    assert_eq!(
        (
            c[&View::All],
            c[&View::Untagged],
            c[&View::Todo],
            c[&View::Today],
            c[&View::Pinned],
            c[&View::Archive],
            c[&View::Trash]
        ),
        (5, 1, 2, 1, 2, 1, 1)
    );
    assert_eq!(h.app.sidebar.tag_roots(), vec!["home", "work"]);
}

#[tokio::test]
async fn help_screen_opens_and_closes() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("question_mark");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Help"));
    assert!(h.text().contains("Three columns"));
    h.press("escape");
    assert!(h.app.overlay.is_none());
}

#[tokio::test]
async fn mouse_click_selects_and_focuses() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let rows = h.app.rects.notes_rows;
    h.click(rows.x + 2, rows.y + 4 * 2 + 1);
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-READING")
    })
    .await;
    assert_eq!(h.app.focus, Pane::Notes);
    h.press("enter");
    assert_eq!(h.app.focus, Pane::Reader);
    assert!(!h.app.sidebar.is_expanded("home"), "tags start folded");
    let side = h.app.rects.sidebar_rows;
    let home_row = h
        .app
        .sidebar
        .rows
        .iter()
        .position(|r| r.tag() == Some("home"))
        .unwrap();
    h.click(side.x + 4, side.y + home_row as u16);
    h.until(|app| app.selection.tag == "home").await;
    assert!(
        !h.app.sidebar.is_expanded("home"),
        "clicking a tag must select it, not toggle it"
    );
    assert_eq!(h.app.focus, Pane::Sidebar);
    let untagged_row = h
        .app
        .sidebar
        .rows
        .iter()
        .position(|r| r.view() == Some(View::Untagged))
        .unwrap();
    h.click(side.x + 4, side.y + untagged_row as u16);
    h.until(|app| app.notes.titles() == vec!["Loose Thought"])
        .await;
    assert_eq!(h.app.focus, Pane::Sidebar);
}

#[tokio::test]
async fn note_rows_show_a_preview_and_no_tags() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let first = h.app.notes.notes[0].clone();
    assert!(!first.preview.is_empty());
    let rendered = h.app.notes.preview_text(&first, 33);
    assert!(rendered.contains(first.preview.split_whitespace().next().unwrap()));
    assert!(!rendered.contains('#'));
    assert!(rendered.matches('\n').count() <= 1);
    let rows = h.app.rects.notes_rows;
    let title_row = h.row(rows.y);
    assert!(title_row.contains("Sprint Planning"), "{title_row}");
    assert!(
        h.row(rows.y + 3).contains('─'),
        "a separator under every note"
    );
}

#[tokio::test]
async fn tag_counts_sit_flush_right_at_every_depth() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("F");
    h.draw();
    let side = h.app.rects.sidebar_rows;
    let mut views = 0;
    let mut tags = 0;
    for (i, row) in h.app.sidebar.rows.clone().iter().enumerate() {
        let text: String = h
            .row(side.y + i as u16)
            .chars()
            .skip(side.x as usize)
            .take(side.width as usize)
            .collect();
        match row {
            Row::Gap => assert!(text.trim().is_empty(), "{text:?}"),
            Row::Heading => assert_eq!(text.trim(), "TAGS"),
            Row::View(_) => {
                views += 1;
                assert!(!text.ends_with(' '), "{text:?}");
                let words: Vec<&str> = text.split_whitespace().collect();
                assert!(
                    words[words.len() - 1].chars().all(|c| c.is_ascii_digit())
                        && words[words.len() - 2].chars().all(|c| c.is_ascii_digit()),
                    "{text:?}"
                );
            }
            Row::Tag { .. } => {
                tags += 1;
                assert!(!text.ends_with(' '), "{text:?}");
                assert!(
                    text.split_whitespace()
                        .last()
                        .unwrap()
                        .chars()
                        .all(|c| c.is_ascii_digit()),
                    "{text:?}"
                );
            }
        }
    }
    assert_eq!(views, 7);
    assert!(tags >= 3);
    assert!(
        h.app
            .sidebar
            .rows
            .iter()
            .any(|r| matches!(r, Row::Tag { depth: 1, .. }))
    );
}

#[tokio::test]
async fn focused_column_header_is_filled_with_the_accent() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let lit = |h: &bjorn::harness::Harness, r: ratatui::layout::Rect| {
        h.cell_bg(r.x + 1, r.y) == theme::accent_color()
    };
    h.app.focus = Pane::Notes;
    h.draw();
    assert!(
        lit(&h, h.app.rects.notes_header)
            && !lit(&h, h.app.rects.sidebar_header)
            && !lit(&h, h.app.rects.note_bar)
    );
    h.app.set_focus(Pane::Sidebar);
    h.draw();
    assert!(
        lit(&h, h.app.rects.sidebar_header)
            && !lit(&h, h.app.rects.notes_header)
            && !lit(&h, h.app.rects.note_bar)
    );
    h.app.set_focus(Pane::Reader);
    h.draw();
    assert!(
        lit(&h, h.app.rects.note_bar)
            && !lit(&h, h.app.rects.sidebar_header)
            && !lit(&h, h.app.rects.notes_header)
    );
}

#[tokio::test]
async fn c_cycles_three_two_one_and_back() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    assert_eq!(
        h.app.visible_panes(),
        vec![Pane::Sidebar, Pane::Notes, Pane::Reader]
    );
    assert!(h.text().contains("▮▮▮"));
    h.press("c");
    assert_eq!(h.app.columns, 2);
    assert_eq!(h.app.visible_panes(), vec![Pane::Notes, Pane::Reader]);
    assert!(h.text().contains("▯▮▮") && !h.text().contains("BJORN"));
    h.press("c");
    assert_eq!(h.app.columns, 1);
    assert!(h.text().contains("▯▯▮") && !h.text().contains("Notes · 5"));
    h.press("c");
    assert_eq!(h.app.columns, 3);
    assert!(h.text().contains("▮▮▮") && h.text().contains("BJORN"));
}

#[tokio::test]
async fn hiding_the_focused_pane_moves_focus_and_keeps_keys_working() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.press("c");
    assert_eq!(h.app.focus, Pane::Notes);
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    h.press("c");
    assert_eq!(h.app.focus, Pane::Reader);
    for _ in 0..4 {
        h.press("tab");
        assert_eq!(
            h.app.focus,
            Pane::Reader,
            "tab never lands on a hidden pane"
        );
    }
    h.press("c");
    assert_eq!(h.app.focus, Pane::Reader);
}

#[tokio::test]
async fn clicking_the_glyph_cycles_too() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let glyph = h.app.rects.glyph;
    h.click(glyph.x + 1, glyph.y);
    assert_eq!(h.app.columns, 2);
    let glyph = h.app.rects.glyph;
    h.click(glyph.x + 1, glyph.y);
    assert_eq!(h.app.columns, 1);
    let glyph = h.app.rects.glyph;
    h.click(glyph.x + 1, glyph.y);
    assert_eq!(h.app.columns, 3);
}

#[tokio::test]
async fn w_scopes_and_big_w_clears() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.move_to_tag("home");
    h.press("enter");
    h.until(|app| app.selection.tag == "home").await;
    h.press("w");
    h.until(|app| app.selection.workspace == "home").await;
    assert_eq!(titles(&h), vec!["Garden Plan", "Reading Queue"]);
    assert_eq!(h.app.sidebar.header(), "WORKSPACE #home");
    assert!(h.text().contains("WORKSPACE #home"));
    assert_eq!(h.app.sidebar.tag_roots(), vec!["home"]);
    let c = &h.app.sidebar.view_counts;
    assert_eq!(
        (c[&View::All], c[&View::Untagged], c[&View::Todo]),
        (2, 0, 1)
    );
    h.press("3");
    h.until(|app| app.notes.titles() == vec!["Garden Plan"])
        .await;
    h.press("W");
    h.until(|app| app.selection.workspace.is_empty()).await;
    assert_eq!(titles(&h).len(), 5);
    assert_eq!(h.app.sidebar.header(), "BJORN");
}

#[tokio::test]
async fn workspace_from_cli_flag_and_config() {
    let fake = Fake::new();
    let mut h = fake.harness_with(fake.config(), Some("#work"));
    h.load().await;
    assert_eq!(h.app.selection.workspace, "work");
    assert_eq!(titles(&h), vec!["Sprint Planning", "CAD and Design"]);
    let config = bjorn::config::Config {
        workspace: "home".into(),
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    assert_eq!(h.app.selection.workspace, "home");
    assert_eq!(titles(&h), vec!["Garden Plan", "Reading Queue"]);
}

#[tokio::test]
async fn unknown_theme_in_the_config_keeps_the_default_and_says_so() {
    // The config file is shared with the Python Bjorn, which accepts Textual's
    // own theme names, so `theme = "nord"` must not stop this build.
    let fake = Fake::new();
    let config = bjorn::config::Config {
        theme: "nord".into(),
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    assert_eq!(theme::current().name, theme::DEFAULT_THEME);
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("nord") && m.contains(theme::DEFAULT_THEME)),
        "{:?}",
        h.app.toast_messages()
    );
    assert!(!titles(&h).is_empty(), "the app still loads");
}

#[tokio::test]
async fn w_again_leaves_the_workspace() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.move_to_tag("home");
    h.press("enter");
    h.until(|app| app.selection.tag == "home").await;
    h.press("w");
    h.until(|app| app.selection.workspace == "home").await;
    assert!(["home", ""].contains(&h.app.sidebar.highlighted_tag().as_str()));
    h.press("w");
    h.until(|app| app.selection.workspace.is_empty()).await;
    assert_eq!(titles(&h).len(), 5);
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.move_to_tag("work");
    h.press("enter");
    h.until(|app| app.selection.tag == "work").await;
    h.press("w");
    h.until(|app| app.selection.workspace == "work").await;
    h.app.set_focus(Pane::Notes);
    h.press("w");
    h.until(|app| app.selection.workspace.is_empty()).await;
}

#[tokio::test]
async fn f_folds_and_unfolds_a_tag() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    assert!(!h.app.sidebar.is_expanded("home"), "tags start folded");
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.move_to_tag("home");
    h.press("f");
    assert!(h.app.sidebar.is_expanded("home"));
    h.press("f");
    assert!(!h.app.sidebar.is_expanded("home"));
    h.press("f");
    assert!(h.app.sidebar.is_expanded("home"));
    h.press("down");
    assert_eq!(h.app.sidebar.highlighted_tag(), "home/garden");
    h.press("f");
    assert!(!h.app.sidebar.is_expanded("home"));
    assert_eq!(h.app.sidebar.highlighted_tag(), "home");
}

#[tokio::test]
async fn folds_survive_entering_and_leaving_a_workspace() {
    let fake = Fake::new();
    let client = fake.client();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    for root in h.app.sidebar.tag_roots() {
        h.app.sidebar.set_expanded(&root, false);
    }
    h.app.sidebar.move_to_tag("home");
    h.press("enter");
    h.until(|app| app.selection.tag == "home").await;
    h.press("w");
    h.until(|app| app.selection.workspace == "home").await;
    h.press("w");
    h.until(|app| app.selection.workspace.is_empty()).await;
    let roots = h.app.sidebar.tag_roots();
    assert_eq!(
        roots
            .iter()
            .map(|r| h.app.sidebar.is_expanded(r))
            .collect::<Vec<_>>(),
        vec![false, false]
    );
    h.app.sidebar.set_expanded("work", true);
    client
        .create("Another", &["home/new".to_string()], "")
        .await
        .unwrap();
    h.press("r");
    h.until(|app| app.snapshot.notes.iter().any(|n| n.title == "Another"))
        .await;
    let roots = h.app.sidebar.tag_roots();
    assert_eq!(
        roots
            .iter()
            .map(|r| h.app.sidebar.is_expanded(r))
            .collect::<Vec<_>>(),
        vec![false, true]
    );
}

#[tokio::test]
async fn big_f_toggles_every_fold_and_respects_the_workspace() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    let all_expanded = |h: &bjorn::harness::Harness| {
        h.app
            .sidebar
            .branches()
            .iter()
            .all(|b| h.app.sidebar.is_expanded(b))
    };
    let none_expanded = |h: &bjorn::harness::Harness| {
        !h.app
            .sidebar
            .branches()
            .iter()
            .any(|b| h.app.sidebar.is_expanded(b))
    };
    assert!(none_expanded(&h), "tags start folded");
    h.press("F");
    assert!(all_expanded(&h));
    h.press("F");
    assert!(none_expanded(&h));
    h.press("F");
    assert!(all_expanded(&h));
    h.app.sidebar.move_to_tag("work");
    h.press("enter");
    h.until(|app| app.selection.tag == "work").await;
    h.press("w");
    h.until(|app| app.selection.workspace == "work").await;
    h.press("F");
    assert_eq!(h.app.sidebar.tag_roots(), vec!["work"]);
    assert!(!h.app.sidebar.is_expanded("work"));
    h.press("w");
    h.until(|app| app.selection.workspace.is_empty()).await;
    assert!(h.app.sidebar.is_expanded("home"));
    assert!(!h.app.sidebar.is_expanded("work"));
}

#[tokio::test]
async fn w_without_a_tag_explains_itself() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("w");
    assert_eq!(h.app.selection.workspace, "");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("Highlight a tag first"))
    );
    assert!(h.text().contains("Highlight a tag first"));
}

#[tokio::test]
async fn number_keys_switch_views() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let expectations: Vec<(&str, Vec<&str>)> = vec![
        ("2", vec!["Loose Thought"]),
        ("3", vec!["Sprint Planning", "Garden Plan"]),
        ("4", vec!["Sprint Planning"]),
        ("5", vec!["Sprint Planning", "Garden Plan"]),
        ("6", vec!["Finished Project"]),
        ("7", vec!["Old Draft"]),
        (
            "1",
            vec![
                "Sprint Planning",
                "Garden Plan",
                "Reading Queue",
                "CAD and Design",
                "Loose Thought",
            ],
        ),
    ];
    for (key, expected) in expectations {
        h.press(key);
        assert_eq!(titles(&h), expected, "after {key}");
        let index = key.parse::<usize>().unwrap() - 1;
        assert_eq!(h.app.selection.view, View::ALL[index]);
    }
    assert_eq!(h.app.notes.header, "Notes · 5");
    assert_eq!(h.app.sidebar.highlighted_view(), Some(View::All));
}

#[tokio::test]
async fn sidebar_cursor_selects_views_and_tags() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.press("j");
    assert_eq!(titles(&h), vec!["Loose Thought"]);
    assert_eq!(h.app.notes.header, "Untagged · 1");
    for _ in 0..6 {
        h.press("down");
    }
    assert_eq!(h.app.selection.tag, "home");
    assert_eq!(h.app.sidebar.highlighted_tag(), "home");
    assert_eq!(titles(&h), vec!["Garden Plan", "Reading Queue"]);
    h.press("f");
    h.press("down");
    assert_eq!(h.app.selection.tag, "home/garden");
    assert_eq!(titles(&h), vec!["Garden Plan"]);
    assert_eq!(h.app.notes.header, "#home/garden · 1");
}

#[tokio::test]
async fn multi_word_tag_selects_its_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    assert!(h.app.sidebar.move_to_tag("work/CAD and Design"));
    h.press("enter");
    assert_eq!(h.app.selection.tag, "work/CAD and Design");
    assert_eq!(titles(&h), vec!["CAD and Design"]);
}

#[tokio::test]
async fn refresh_picks_up_external_changes() {
    let fake = Fake::new();
    let client = fake.client();
    let mut h = fake.harness();
    h.load().await;
    client
        .create("Made Elsewhere", &["home".to_string()], "")
        .await
        .unwrap();
    h.press("r");
    h.until(|app| app.notes.titles().contains(&"Made Elsewhere".to_string()))
        .await;
}

#[tokio::test]
async fn poll_reloads_when_the_probe_changes() {
    let fake = Fake::new();
    let client = fake.client();
    let config = bjorn::config::Config {
        poll_seconds: 1,
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    client
        .create("Polled In", &["home".to_string()], "")
        .await
        .unwrap();
    h.wait_until(
        |app| app.notes.titles().contains(&"Polled In".to_string()),
        Duration::from_secs(6),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn reload_leaves_an_unchanged_note_on_the_page() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    // The first note is stamped this very second, which the caches rightly distrust; step onto an older one.
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.title == "Garden Plan")
    })
    .await;
    let renders = h.app.reader.renders;
    let generation_before = h.app.notes.rebuilds;
    h.app.start_reload(None, None, false);
    h.until(|app| app.notes.rebuilds > generation_before).await;
    h.settle().await;
    assert_eq!(h.app.reader.note.as_ref().unwrap().title, "Garden Plan");
    assert_eq!(
        h.app.reader.renders, renders,
        "a poll that changes nothing must not redraw the reader"
    );
}

#[tokio::test]
async fn sidebar_is_one_column_arrows_cross_the_gap_and_tab_reaches_the_notes() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.set_focus(Pane::Sidebar);
    h.app.sidebar.select_view(View::Trash);
    h.press("down");
    assert_eq!(h.app.sidebar.highlighted_tag(), "home");
    assert_eq!(h.app.selection.tag, "home");
    h.press("up");
    assert_eq!(h.app.sidebar.highlighted_view(), Some(View::Trash));
    assert_eq!(h.app.selection.view, View::Trash);
    assert!(h.app.selection.tag.is_empty());
    let trash = h
        .app
        .sidebar
        .rows
        .iter()
        .position(|r| r.view() == Some(View::Trash))
        .unwrap();
    let home = h
        .app
        .sidebar
        .rows
        .iter()
        .position(|r| r.tag() == Some("home"))
        .unwrap();
    assert_eq!(home - trash - 1, 3, "two blank rows and the TAGS heading");
    h.press("3");
    assert_eq!(h.app.selection.view, View::Todo);
    h.press("tab");
    assert_eq!(h.app.focus, Pane::Notes);
    assert_eq!(h.app.selection.view, View::Todo);
    assert_eq!(titles(&h), vec!["Sprint Planning", "Garden Plan"]);
}

#[tokio::test]
async fn tab_into_the_sidebar_keeps_the_cursor() {
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
    let rebuilds = h.app.notes.rebuilds;
    h.press("shift+tab");
    assert_eq!(h.app.focus, Pane::Sidebar);
    assert_eq!(
        h.app.notes.rebuilds, rebuilds,
        "focus alone must not rebuild the list"
    );
    assert_eq!(h.app.notes.current().unwrap().id, "NOTE-GARDEN");
}

#[tokio::test]
async fn q_asks_and_n_keeps_running_y_quits() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("q");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Confirm"));
    assert!(h.text().contains("Quit Bjorn?"));
    h.press("n");
    assert!(h.app.running);
    assert!(h.app.overlay.is_none());
    h.press("q");
    h.press("y");
    assert!(!h.app.running);
}

#[tokio::test]
async fn locked_and_missing_notes_explain_themselves() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let mut locked = h.app.notes.notes[1].clone();
    locked.locked = true;
    h.app.schedule_preview(locked.clone(), true, true);
    h.draw();
    assert!(h.text().contains("This note is locked"));
    let mut gone = locked.clone();
    gone.locked = false;
    gone.id = "NOPE".into();
    h.app.notes.notes[1] = gone.clone();
    h.app.notes.select_id("NOPE");
    h.app.schedule_preview(gone, true, true);
    h.until(|app| {
        app.reader
            .message()
            .is_some_and(|m| m.contains("Could not read note"))
    })
    .await;
}

#[tokio::test]
async fn the_reader_does_not_wait_for_a_note_it_already_has() {
    // The cold listing reads every body to build the previews, so they are
    // already in hand: moving the cursor draws the note in the same frame
    // rather than after the debounce and a `bearcli cat`.
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let first = h.app.reader.note.as_ref().unwrap().id.clone();

    h.app.set_focus(Pane::Notes);
    h.press("j");
    let next = h.app.notes.current().unwrap().id.clone();
    assert_ne!(next, first, "the cursor moved");
    assert_eq!(
        h.app.reader.note.as_ref().map(|n| n.id.clone()),
        Some(next.clone()),
        "the reader followed inside the keypress"
    );
    assert!(h.app.reader.full_text.is_some());

    // A body the cache has not seen still waits, so a big library does not
    // spawn a bearcli per keypress while the cursor is moving.
    let mut unseen = h.app.notes.notes[0].clone();
    unseen.id = "NOT-CACHED".into();
    h.app.notes.notes[0] = unseen.clone();
    h.app.notes.select_id("NOT-CACHED");
    h.app.schedule_preview(unseen, false, true);
    assert_eq!(
        h.app.reader.note.as_ref().map(|n| n.id.clone()),
        Some(next),
        "an uncached note leaves the last one up until it arrives"
    );
}
