//! Middle column: the notes matching the current selection.

use chrono::{DateTime, Datelike, Local, Utc};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use crate::bear::Note;
use crate::ui::markdown::{RLine, wrap};
use crate::ui::theme;

/// Rows per note: a title line, two preview rows, and the separator under them.
pub const ROW_HEIGHT: usize = 4;
pub const PREVIEW_ROWS: usize = 2;

/// `14:02` today, `Yesterday`, `Mon`, `Sep 3`, or `2025-11-06` for older.
pub fn relative_date(when: Option<DateTime<Utc>>, now: Option<DateTime<Utc>>) -> String {
    let Some(when) = when else {
        return String::new();
    };
    let now = now.unwrap_or_else(Utc::now).with_timezone(&Local);
    let local = when.with_timezone(&Local);
    let delta = (now.date_naive() - local.date_naive()).num_days();
    if delta <= 0 {
        return local.format("%H:%M").to_string();
    }
    if delta == 1 {
        return "Yesterday".to_string();
    }
    if delta < 7 {
        return local.format("%a").to_string();
    }
    if local.year() == now.year() {
        return format!("{} {}", local.format("%b"), local.day());
    }
    local.format("%Y-%m-%d").to_string()
}

#[derive(Debug, Clone, Default)]
pub struct NoteList {
    pub notes: Vec<Note>,
    pub cursor: Option<usize>,
    pub scroll: usize,
    pub header: String,
    pub pattern: Option<Regex>,
    /// How many times the list has been rebuilt, for tests that assert a
    /// focus change rebuilt nothing.
    pub rebuilds: usize,
}

impl NoteList {
    /// Replace the list; keep the cursor on `keep_id` when it is still present,
    /// else on the same index (clamped). Returns the note now under the cursor.
    pub fn show_notes(&mut self, notes: Vec<Note>, keep_id: Option<&str>) -> Option<Note> {
        self.rebuilds += 1;
        let previous = self.cursor.unwrap_or(0);
        self.notes = notes;
        if self.notes.is_empty() {
            self.cursor = None;
            self.scroll = 0;
            return None;
        }
        let target = keep_id
            .and_then(|id| self.notes.iter().position(|n| n.id == id))
            .unwrap_or_else(|| previous.min(self.notes.len() - 1));
        self.cursor = Some(target);
        Some(self.notes[target].clone())
    }

    pub fn current(&self) -> Option<&Note> {
        self.cursor.and_then(|i| self.notes.get(i))
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn titles(&self) -> Vec<String> {
        self.notes.iter().map(|n| n.title.clone()).collect()
    }

    pub fn select_id(&mut self, note_id: &str) -> bool {
        match self.notes.iter().position(|n| n.id == note_id) {
            Some(i) => {
                self.cursor = Some(i);
                true
            }
            None => false,
        }
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if index < self.notes.len() {
            self.cursor = Some(index);
            true
        } else {
            false
        }
    }

    /// Move the cursor by `delta`; false at the edges.
    pub fn move_cursor(&mut self, delta: i32) -> bool {
        let Some(cursor) = self.cursor else {
            return false;
        };
        let next = cursor as i64 + delta as i64;
        if next < 0 || next >= self.notes.len() as i64 {
            return false;
        }
        self.cursor = Some(next as usize);
        true
    }

    /// Keep the cursor's row inside a viewport of `height` cells.
    pub fn ensure_visible(&mut self, height: usize) {
        let rows = (height / ROW_HEIGHT).max(1);
        let cursor = self.cursor.unwrap_or(0);
        if cursor < self.scroll {
            self.scroll = cursor;
        } else if cursor >= self.scroll + rows {
            self.scroll = cursor + 1 - rows;
        }
        self.scroll = self.scroll.min(self.notes.len().saturating_sub(rows));
    }

    /// The three content lines of one note row, wrapped to `width`.
    pub fn render_item(
        &self,
        note: &Note,
        width: usize,
        cursor_style: Option<Style>,
    ) -> Vec<Line<'static>> {
        let base = cursor_style.unwrap_or_default();
        let dim = if cursor_style.is_some() {
            base
        } else {
            theme::dim()
        };
        let mut title_spans: Vec<Span<'static>> = Vec::new();
        if note.pinned() {
            title_spans.push(Span::styled("📌 ", base.fg(theme::WARNING)));
        }
        title_spans.push(Span::styled(
            note.title.clone(),
            base.add_modifier(ratatui::style::Modifier::BOLD),
        ));
        let title = truncate_line(Line::from(title_spans), width);

        let mut lead = relative_date(note.modified, None);
        if note.todos > 0 {
            lead.push_str(&format!("  ☐ {}", note.todos));
        }
        if note.locked {
            lead.push_str("  🔒");
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        if !lead.is_empty() {
            spans.push(Span::styled(lead, dim));
        }
        if !note.preview.is_empty() {
            if !spans.is_empty() {
                spans.push(Span::styled("  ", dim));
            }
            spans.push(Span::styled(note.preview.clone(), dim));
        }
        let preview = RLine {
            spans,
            block: 0,
            cont: Vec::new(),
        };
        let mut rows = wrap(&preview, width);
        let clipped = rows.len() > PREVIEW_ROWS;
        rows.truncate(PREVIEW_ROWS);
        if clipped && let Some(last) = rows.last_mut() {
            let mut shortened = truncate_line(std::mem::take(last), width.saturating_sub(1));
            if !shortened
                .spans
                .last()
                .is_some_and(|s| s.content.ends_with('…'))
            {
                shortened.spans.push(Span::styled("…", dim));
            }
            *last = shortened;
        }
        while rows.len() < PREVIEW_ROWS {
            rows.push(Line::default());
        }
        let mut out = vec![title];
        out.extend(rows);
        if let Some(pattern) = &self.pattern {
            out = out
                .into_iter()
                .map(|line| crate::ui::highlight::highlight_line(line, pattern))
                .collect();
        }
        out
    }

    /// The preview rows as plain text, for tests.
    pub fn preview_text(&self, note: &Note, width: usize) -> String {
        let lines = self.render_item(note, width, None);
        lines[1..]
            .iter()
            .map(|l| l.to_string().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Cut a line to `width` cells, ending with an ellipsis when something was cut.
pub fn truncate_line(line: Line<'static>, width: usize) -> Line<'static> {
    let total: usize = line
        .spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total <= width {
        return line;
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let budget = width.saturating_sub(1);
    for span in line.spans {
        let mut piece = String::new();
        for ch in span.content.chars() {
            let w = UnicodeWidthStr::width(ch.to_string().as_str());
            if used + w > budget {
                break;
            }
            piece.push(ch);
            used += w;
        }
        let done = used >= budget
            || UnicodeWidthStr::width(piece.as_str())
                < UnicodeWidthStr::width(span.content.as_ref());
        if !piece.is_empty() {
            out.push(Span::styled(piece, span.style));
        }
        if done {
            break;
        }
    }
    out.push(Span::styled(
        "…",
        out.last().map(|s| s.style).unwrap_or_default(),
    ));
    Line::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn relative_dates() {
        let now = Local
            .with_ymd_and_hms(2026, 9, 11, 15, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let at = |y, m, d, h| {
            Some(
                Local
                    .with_ymd_and_hms(y, m, d, h, 2, 0)
                    .unwrap()
                    .with_timezone(&Utc),
            )
        };
        assert_eq!(relative_date(at(2026, 9, 11, 14), Some(now)), "14:02");
        assert_eq!(relative_date(at(2026, 9, 10, 9), Some(now)), "Yesterday");
        assert_eq!(relative_date(at(2026, 9, 8, 9), Some(now)), "Tue");
        assert_eq!(relative_date(at(2026, 9, 3, 9), Some(now)), "Sep 3");
        assert_eq!(relative_date(at(2025, 11, 6, 9), Some(now)), "2025-11-06");
        assert_eq!(relative_date(None, Some(now)), "");
    }

    #[test]
    fn item_has_title_and_two_preview_rows() {
        let list = NoteList::default();
        let note = Note {
            id: "N".into(),
            title: "A very long title that will need cutting at some point".into(),
            pins: vec!["global".into()],
            todos: 2,
            preview: "word ".repeat(40),
            ..Note::default()
        };
        let lines = list.render_item(&note, 30, None);
        assert_eq!(lines.len(), 3);
        let title = lines[0].to_string();
        assert!(title.starts_with("📌 A very long"));
        assert!(title.ends_with('…'));
        assert!(UnicodeWidthStr::width(title.as_str()) <= 30);
        let preview = list.preview_text(&note, 30);
        assert!(preview.contains("☐ 2"));
        assert!(preview.ends_with('…'));
        assert!(!preview.ends_with("……"), "{preview}");
        assert_eq!(preview.lines().count(), 2);
        let wide = Note {
            preview: "x".repeat(200),
            ..Note::default()
        };
        let text = list.preview_text(&wide, 30);
        assert!(text.ends_with('…') && !text.ends_with("……"), "{text}");
    }

    #[test]
    fn show_notes_keeps_cursor_by_id_or_index() {
        let mut list = NoteList::default();
        let mk = |id: &str| Note {
            id: id.into(),
            title: id.into(),
            ..Note::default()
        };
        list.show_notes(vec![mk("a"), mk("b"), mk("c")], None);
        assert_eq!(list.current().unwrap().id, "a");
        list.move_cursor(2);
        list.show_notes(vec![mk("a"), mk("c")], Some("c"));
        assert_eq!(list.current().unwrap().id, "c");
        list.show_notes(vec![mk("a")], Some("zzz"));
        assert_eq!(list.current().unwrap().id, "a");
        assert!(list.show_notes(vec![], None).is_none());
        assert_eq!(list.rebuilds, 4);
    }
}
