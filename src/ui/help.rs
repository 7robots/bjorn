//! The `?` overlay: keys grouped by what they act on, then the themes.
//!
//! Drawn straight to styled lines rather than through the markdown renderer:
//! a two-column key table wraps badly once its cells run long, and the theme
//! list is live (it names the palette in use), which a constant cannot be.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::ui::theme;

/// Width of the key column; descriptions hang from the column after it.
const KEY_WIDTH: usize = 14;
const INDENT: usize = 2;

const INTRO: &str = "Three columns: smart views and tags · notes · the rendered note. \
j k or the arrows scroll this help; esc, q or ? close it.";

/// (heading, rows). A row is (key, what it does); an empty key is a note
/// under the row above it.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("tab / ⇧tab", "cycle the panes"),
            ("j k ↑ ↓", "move within a pane; in the reader they scroll"),
            ("g / G", "first / last row"),
            (
                "enter",
                "open the highlighted note in the reader, at the first match while searching",
            ),
            (
                "1 – 7",
                "Notes, Untagged, Todo, Today, Pinned, Archive, Trash",
            ),
            (
                "c",
                "cycle the columns: hide tags, then notes too, then show all three (or click ▮▮▮ in the note header)",
            ),
            ("r", "refresh from Bear now"),
        ],
    ),
    (
        "Search",
        &[
            (
                "/",
                "search with Bear syntax: @todo, #tag, \"phrase\", -term. @ and # complete as you type; tab or → accepts. A bare sub-tag is searched as #*/name",
            ),
            ("esc", "clear the search and its highlights"),
            ("] / [", "next / previous match in the reader"),
        ],
    ),
    (
        "Notes",
        &[
            ("n", "new note (title, tags), then edit"),
            (
                "e",
                "edit in $VISUAL / $EDITOR. Hash-guarded: if the note changed in Bear meanwhile, nothing is written and your version is kept in a temp file",
            ),
            ("d", "move the note to the trash (asks first)"),
            ("u", "restore from Trash or Archive"),
            ("p", "toggle the global pin"),
            ("b", "open the note in Bear.app"),
            (
                "x",
                "export as Markdown, HTML, plain text, RTF or TextBundle; ← → pick, export_format in the config sets the default",
            ),
        ],
    ),
    (
        "Actions",
        &[
            (
                "!",
                "run the default action on the note, or open the menu if none is set",
            ),
            (
                "a",
                "the action menu: your [[actions]] under a search box, ★ on the default, the highlighted command shown in full",
            ),
            (
                "",
                "in the menu: enter runs · + New action adds one · ctrl+e edits · ctrl+d deletes (asks first) · esc closes",
            ),
        ],
    ),
    (
        "Links",
        &[
            (
                "L",
                "the note's [[wiki links]] and the notes linking to it, under a search box; enter follows. A missing note is offered for creation. Backlinks read at most 200 candidates, and say so when the list may be incomplete",
            ),
            (
                "click",
                "a link in the reader (or a table) follows it; [[Title/Heading]] lands on the heading",
            ),
            (
                "backspace",
                "back to the note you followed a link from, list and scroll included (also ctrl+o, alt+←, alt+b); edits the query while the search box is open",
            ),
            ("alt+→", "forward again (also alt+f)"),
        ],
    ),
    (
        "Tags and workspace",
        &[
            (
                "w",
                "make the highlighted tag the workspace; again on it to leave",
            ),
            ("W", "clear the workspace"),
            ("f", "fold / unfold the highlighted tag's subtree"),
            (
                "F",
                "fold every tag, or unfold them all when all are folded",
            ),
        ],
    ),
    (
        "Triage",
        &[
            ("t", "every open todo in the workspace, grouped by note"),
            ("space", "mark the row"),
            ("x", "tick the marked (or highlighted) todos in Bear"),
            (
                "enter / b",
                "go to the note / open it in Bear at that section",
            ),
            ("/ · r", "filter · reload"),
            (
                "a",
                "add the marked todos to Apple Reminders (needs [reminders] enabled = true); rows show ⏰ once added and ✓ when completed there",
            ),
            ("esc / q", "close triage"),
        ],
    ),
];

/// Every line of the help, wrapped to `width` cells.
pub fn lines(width: usize) -> Vec<Line<'static>> {
    let width = width.max(KEY_WIDTH + INDENT + 8);
    let mut out: Vec<Line<'static>> = Vec::new();

    for row in wrap(INTRO, width) {
        out.push(Line::from(Span::styled(row, theme::muted())));
    }

    for (heading, rows) in SECTIONS {
        out.push(Line::from(""));
        out.push(heading_line(heading));
        for (key, text) in rows.iter() {
            push_row(&mut out, key, text, width);
        }
    }

    out.push(Line::from(""));
    out.push(heading_line("Themes"));
    let current = theme::current().name;
    let how = format!(
        "Now drawing {current}. Set theme = \"name\" in ~/.config/bjorn/config.toml, \
         or start with bjorn --theme name; bjorn --list-themes prints the names."
    );
    for row in wrap(&how, width - INDENT) {
        out.push(Line::from(vec![
            Span::raw(" ".repeat(INDENT)),
            Span::raw(row),
        ]));
    }
    out.push(Line::from(""));
    out.extend(theme_rows(current, width));
    out.push(Line::from(""));
    out
}

fn heading_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        theme::accent().add_modifier(Modifier::BOLD),
    ))
}

/// One key row: the key in the accent colour, the description wrapped so
/// every continuation line hangs under the first word of the description.
fn push_row(out: &mut Vec<Line<'static>>, key: &str, text: &str, width: usize) {
    let hang = INDENT + KEY_WIDTH + 2;
    let body = wrap(text, width.saturating_sub(hang));
    let mut first = true;
    for row in body {
        let lead = if first {
            let pad = KEY_WIDTH.saturating_sub(key.width()) + 2;
            vec![
                Span::raw(" ".repeat(INDENT)),
                Span::styled(
                    key.to_string(),
                    theme::accent().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(pad)),
            ]
        } else {
            vec![Span::raw(" ".repeat(hang))]
        };
        let style = if key.is_empty() {
            theme::muted()
        } else {
            Style::default()
        };
        let mut spans = lead;
        spans.push(Span::styled(row, style));
        out.push(Line::from(spans));
        first = false;
    }
}

/// The theme names as a wrapped list, the active one marked.
fn theme_rows(current: &str, width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ".repeat(INDENT))];
    let mut used = INDENT;
    for name in theme::names() {
        let cell = name.width() + 2;
        if used + cell > width && used > INDENT {
            rows.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::raw(" ".repeat(INDENT)));
            used = INDENT;
        }
        let style = if name == current {
            theme::accent().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(name.to_string(), style));
        spans.push(Span::raw("  "));
        used += cell;
    }
    if spans.len() > 1 {
        rows.push(Line::from(spans));
    }
    rows
}

/// Greedy word wrap on display width; a word longer than the width gets a
/// line of its own rather than being split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.width() + 1 + word.width() > width {
            rows.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn every_line_fits_the_width() {
        for width in [60usize, 76, 100] {
            for line in lines(width) {
                let t = text(&line);
                assert!(t.width() <= width, "{width}: {t:?} is {} wide", t.width());
            }
        }
    }

    #[test]
    fn continuation_lines_hang_under_the_description() {
        let all = lines(60);
        let rows: Vec<String> = all.iter().map(text).collect();
        // `e` wraps at 60 cells; its second line must start at the hang column.
        let i = rows
            .iter()
            .position(|r| r.trim_start().starts_with("e  "))
            .unwrap();
        assert!(
            rows[i + 1].starts_with(&" ".repeat(INDENT + KEY_WIDTH + 2)),
            "{:?}",
            rows[i + 1]
        );
        assert!(!rows[i + 1].trim().is_empty());
    }

    #[test]
    fn names_every_theme_and_the_active_one() {
        let all: Vec<String> = lines(80).iter().map(text).collect();
        let joined = all.join("\n");
        for name in theme::names() {
            assert!(joined.contains(name), "{name} missing");
        }
        assert!(joined.contains(&format!("Now drawing {}", theme::current().name)));
    }
}
