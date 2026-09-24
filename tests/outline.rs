//! The section outline (`o`) and heading jumps (`}` / `{`) in the reader.

mod common;

use bjorn::app::Pane;
use bjorn::harness::Harness;
use bjorn::ui::modals::Overlay;
use common::Fake;

/// A note whose first section is one long paragraph, so the rows above the
/// later headings are wrapped rows, not source lines.
fn body() -> String {
    let long = "This opening paragraph runs on well past the width of the reader so that it wraps onto several rows. "
        .repeat(8);
    let filler: String = (1..=40).map(|i| format!("- filler {i}\n")).collect();
    format!(
        "# Outline Test\n#work\n\n## Intro\n{long}\n\n```sh\n# not a heading\n## nor this\n```\n\n\
         ## Details\nfirst details\n\n### Deep dive\nunder the surface\n\n## Details\nsecond details\n\n\
         {filler}\n## Last\nthe end\n"
    )
}

/// The harness with `body()` in the reader, drawn once so the viewport is known.
async fn reader_with(fake: &Fake, content: &str) -> Harness {
    let mut h = fake.harness();
    h.load().await;
    h.until(|app| app.reader.note.is_some()).await;
    let note = h.app.reader.note.clone().unwrap();
    h.app.reader.show(&note, content);
    h.draw();
    h
}

fn texts(h: &Harness) -> Vec<String> {
    h.app
        .reader
        .headings
        .iter()
        .map(|x| x.text.clone())
        .collect()
}

/// The text on the reader's top row.
fn top_row(h: &Harness) -> String {
    h.row(h.app.rects.reader_body.y)
}

fn current(h: &mut Harness) -> Option<usize> {
    let (w, ht) = h.app.reader_viewport();
    h.app.reader.current_heading(w, ht)
}

/// The outline dialog's rows, from its top border to its bottom one.
fn dialog(h: &Harness) -> String {
    let lines = h.lines();
    let start = lines
        .iter()
        .position(|l| l.contains("┌ Outline"))
        .expect("the outline is drawn");
    let end = start + lines[start..].iter().position(|l| l.contains('└')).unwrap();
    lines[start..=end].join("\n")
}

fn outline_index(h: &Harness) -> usize {
    match &h.app.overlay {
        Some(Overlay::Outline { index, .. }) => *index,
        other => panic!("the outline is not open: {other:?}"),
    }
}

#[tokio::test]
async fn the_outline_lists_every_heading_but_none_from_fenced_code() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    assert_eq!(
        texts(&h),
        vec![
            "Outline Test",
            "Intro",
            "Details",
            "Deep dive",
            "Details",
            "Last"
        ]
    );
    let levels: Vec<u8> = h.app.reader.headings.iter().map(|x| x.level).collect();
    assert_eq!(levels, vec![1, 2, 2, 3, 2, 2]);

    h.press("o");
    assert_eq!(outline_index(&h), 0, "the top of the note is in the title");
    let screen = dialog(&h);
    assert!(screen.contains("Outline · “Sprint Planning”"), "{screen}");
    assert!(screen.contains("▸ Outline Test"), "{screen}");
    // Indented by level: two cells per level below the first.
    assert!(screen.contains("      Deep dive"), "{screen}");
    assert!(screen.contains("    Intro"), "{screen}");

    h.press("escape");
    assert!(h.app.overlay.is_none());
    assert_eq!(h.app.reader.scroll, 0, "escape does not move the reader");
}

#[tokio::test]
async fn enter_puts_the_heading_on_the_top_row_past_wrapped_lines() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.app.set_focus(Pane::Notes);
    h.press("o");
    h.press("down");
    h.press("down");
    assert_eq!(outline_index(&h), 2);
    h.press("enter");
    assert!(h.app.overlay.is_none());
    assert_eq!(h.app.focus, Pane::Reader);
    assert!(top_row(&h).contains("Details"), "{}", h.text());
    // The paragraph above wraps, so the row is past its source line number.
    let logical = h
        .app
        .reader
        .plain_text()
        .lines()
        .position(|l| l == "Details")
        .unwrap();
    assert!(
        h.app.reader.scroll > logical,
        "scroll {} counts wrapped rows, not the {} lines above",
        h.app.reader.scroll,
        logical
    );
    assert_eq!(current(&mut h), Some(2));
}

#[tokio::test]
async fn typing_filters_the_outline_and_duplicates_stay_apart() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.press("o");
    h.type_text("DET");
    assert_eq!(
        outline_index(&h),
        0,
        "typing sends the highlight to the top"
    );
    let screen = dialog(&h);
    assert_eq!(screen.matches("Details").count(), 2, "{screen}");
    assert!(!screen.contains("Intro"), "{screen}");
    assert!(!screen.contains("Deep dive"), "{screen}");
    h.press("down");
    h.press("enter");
    assert!(top_row(&h).contains("Details"), "{}", h.text());
    assert_eq!(
        current(&mut h),
        Some(4),
        "the second Details, not the first"
    );
    assert!(
        h.text().contains("second details"),
        "the second section is on screen"
    );

    h.press("o");
    h.type_text("zzz");
    assert!(dialog(&h).contains("no heading matches"));
    h.press("enter");
    assert!(
        matches!(h.app.overlay, Some(Overlay::Outline { .. })),
        "enter with nothing listed keeps the outline open"
    );
    h.press("backspace");
    h.press("backspace");
    h.press("backspace");
    assert!(dialog(&h).contains("Deep dive"));
}

#[tokio::test]
async fn the_outline_opens_on_the_current_section() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.press("}");
    h.press("}");
    h.press("}");
    assert_eq!(current(&mut h), Some(3));
    // Scroll a little into the section: still in it.
    h.press("j");
    h.press("o");
    assert_eq!(outline_index(&h), 3);
    assert!(dialog(&h).contains("▸     Deep dive"), "{}", dialog(&h));
}

#[tokio::test]
async fn braces_step_through_headings() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.app.set_focus(Pane::Notes);
    h.press("}");
    assert_eq!(h.app.focus, Pane::Reader);
    assert_eq!(current(&mut h), Some(1));
    assert!(top_row(&h).contains("Intro"));
    h.press("}");
    assert_eq!(current(&mut h), Some(2));
    h.press("{");
    assert_eq!(current(&mut h), Some(1));

    // Partway into Intro, `{` goes back to Intro's own heading first.
    h.press("j");
    h.press("j");
    assert_eq!(current(&mut h), Some(1));
    h.press("{");
    assert_eq!(current(&mut h), Some(1));
    assert!(top_row(&h).contains("Intro"));
    h.press("{");
    assert_eq!(current(&mut h), Some(0));
    assert_eq!(h.app.reader.scroll, 0);
    h.press("{");
    assert_eq!(h.app.reader.scroll, 0, "nothing above the first heading");

    // `]` / `[` stay with search matches; with no search they do nothing.
    h.press("}");
    let scroll = h.app.reader.scroll;
    h.press("]");
    h.press("[");
    assert_eq!(h.app.reader.scroll, scroll);
}

#[tokio::test]
async fn the_last_heading_is_reachable_though_it_cannot_reach_the_top() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    for _ in 0..10 {
        h.press("}");
    }
    assert_eq!(current(&mut h), Some(5));
    assert!(h.text().contains("the end"));
    // The heading before it may share the last screen; `{` still steps back.
    h.press("{");
    assert_eq!(current(&mut h), Some(4));
}

#[tokio::test]
async fn the_reader_footer_names_the_section() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    let meta_row = |h: &Harness| {
        let body = h.app.rects.reader_body;
        h.row(body.y + body.height)
    };
    assert!(
        !meta_row(&h).contains('§'),
        "the title is not a section: {}",
        meta_row(&h)
    );
    h.press("}");
    assert!(meta_row(&h).contains("§ Intro ·"), "{}", meta_row(&h));
}

#[tokio::test]
async fn a_note_without_headings_says_so() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, "just text\n\n```\n# code\n```\n").await;
    assert!(h.app.reader.headings.is_empty());
    h.press("o");
    assert!(h.app.overlay.is_none());
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m == "This note has no headings."),
        "{:?}",
        h.app.toast_messages()
    );
    h.press("}");
    assert_eq!(h.app.reader.scroll, 0);
}

#[tokio::test]
async fn scrolling_away_from_a_jumped_heading_drops_it_as_the_section() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.app.focus = Pane::Reader;
    h.press("}");
    assert_eq!(current(&mut h), Some(1));
    h.press("g");
    h.draw();
    assert_eq!(current(&mut h), Some(0), "back at the title after g");
}

#[tokio::test]
async fn a_refresh_that_drops_headings_keeps_the_outline_usable() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.press("o");
    for _ in 0..5 {
        h.press("down");
    }
    let note = h.app.reader.note.clone().unwrap();
    h.app.reader.show(&note, "# Short\n\n## Only\ntext\n");
    h.draw();
    h.press("enter");
    assert!(
        h.app.overlay.is_none(),
        "enter lands on the last heading left"
    );
}

#[tokio::test]
async fn empty_headings_and_long_setext_underlines() {
    let fake = Fake::new();
    let h = reader_with(&fake, "# A\n\ntext\n\n#\n\nSetext\n=====\n\n## B\nend\n").await;
    assert_eq!(texts(&h), vec!["A", "Setext", "B"]);
}

#[tokio::test]
async fn cursor_keys_in_the_filter_keep_the_highlight() {
    let fake = Fake::new();
    let mut h = reader_with(&fake, &body()).await;
    h.press("o");
    h.press("down");
    h.press("down");
    let before = outline_index(&h);
    for key in ["left", "right", "home", "end"] {
        h.press(key);
        assert_eq!(outline_index(&h), before, "{key} moved the highlight");
    }
}
