//! Right column: the rendered note.

use chrono::Local;
use ratatui::text::Line;
use regex::Regex;

use crate::bear::Note;
use crate::ui::highlight::highlight_line;
use crate::ui::markdown::{RLine, render, wrap};

/// Glyph per column count: a hollow block for each hidden column.
pub fn column_glyph(count: u8) -> &'static str {
    match count {
        1 => "▯▯▮",
        2 => "▯▮▮",
        _ => "▮▮▮",
    }
}

#[derive(Debug, Clone, Default)]
pub struct Reader {
    pub note: Option<Note>,
    pub full_text: Option<String>,
    lines: Vec<RLine>,
    wrapped: Vec<(Line<'static>, usize)>,
    wrapped_width: usize,
    pub scroll: usize,
    pub header: String,
    pub meta: String,
    pub pattern: Option<Regex>,
    /// Blocks holding a match, in document order, and the index last jumped to (-1: none yet).
    pub matches: Vec<usize>,
    pub match_index: i64,
    message: Option<String>,
    /// How many times a note was (re)rendered, for tests.
    pub renders: usize,
}

impl Reader {
    /// Is this version of `note` (same id, same modification) already on the
    /// page? An error page counts as not shown.
    pub fn shows(&self, note: &Note) -> bool {
        self.note.as_ref().is_some_and(|shown| {
            self.full_text.is_some() && shown.id == note.id && shown.modified == note.modified
        })
    }

    pub fn clear(&mut self, message: &str) {
        self.note = None;
        self.full_text = None;
        self.lines.clear();
        self.wrapped.clear();
        self.matches.clear();
        self.match_index = -1;
        self.header.clear();
        self.meta.clear();
        self.scroll = 0;
        self.message = if message.is_empty() {
            None
        } else {
            Some(message.to_string())
        };
    }

    pub fn show(&mut self, note: &Note, content: &str) {
        self.renders += 1;
        self.note = Some(note.clone());
        self.full_text = Some(content.to_string());
        self.lines = render(content);
        self.wrapped.clear();
        self.wrapped_width = 0;
        self.message = None;
        self.scroll = 0;
        self.recompute_matches();
        self.set_header();
    }

    pub fn show_error(&mut self, note: &Note, message: &str) {
        self.note = Some(note.clone());
        self.full_text = None;
        self.lines.clear();
        self.wrapped.clear();
        self.matches.clear();
        self.match_index = -1;
        self.scroll = 0;
        self.header = note.title.clone();
        self.meta.clear();
        self.message = Some(message.to_string());
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Highlight `pattern` in the rendered note now and in every note shown
    /// until it changes; None clears.
    pub fn set_pattern(&mut self, pattern: Option<Regex>) {
        let same = match (&pattern, &self.pattern) {
            (Some(a), Some(b)) => a.as_str() == b.as_str(),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.pattern = pattern;
        self.recompute_matches();
        self.set_header();
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        self.match_index = -1;
        let Some(pattern) = &self.pattern else { return };
        for line in &self.lines {
            if pattern.is_match(&line.plain()) && self.matches.last() != Some(&line.block) {
                self.matches.push(line.block);
            }
        }
    }

    /// The next `jump(1)` lands on the first match.
    pub fn reset_match_cursor(&mut self) {
        self.match_index = -1;
    }

    /// Scroll to the next (`delta` 1) or previous (-1) matching block,
    /// wrapping. False when there is nothing to jump to. Needs the width the
    /// note is wrapped at, so the block's first row is known.
    pub fn jump(&mut self, delta: i64, width: usize) -> bool {
        if self.matches.is_empty() {
            return false;
        }
        let n = self.matches.len() as i64;
        self.match_index = (self.match_index + delta).rem_euclid(n);
        let block = self.matches[self.match_index as usize];
        self.ensure_wrapped(width);
        if let Some(row) = self.wrapped.iter().position(|(_, b)| *b == block) {
            self.scroll = row;
        }
        self.set_header();
        true
    }

    /// Scroll to the block holding a heading, given the heading line as it is
    /// written in the note (`## September 19, 2026 (Saturday)`). The rendered
    /// line carries the text without its `#` markers, which is what is matched
    /// — and only against lines the renderer marked as headings, so a body
    /// line repeating the date cannot win. False when there is no such heading.
    pub fn scroll_to_heading(&mut self, heading: &str, width: usize) -> bool {
        let needle = heading.trim().trim_start_matches('#').trim();
        if needle.is_empty() {
            return false;
        }
        let Some(block) = self
            .lines
            .iter()
            .find(|line| line.heading && line.plain().trim() == needle)
            .map(|line| line.block)
        else {
            return false;
        };
        self.ensure_wrapped(width);
        match self.wrapped.iter().position(|(_, b)| *b == block) {
            Some(row) => {
                self.scroll = row;
                true
            }
            None => false,
        }
    }

    fn match_label(&self) -> String {
        if self.pattern.is_none() || self.matches.is_empty() {
            return String::new();
        }
        if self.match_index < 0 {
            let n = self.matches.len();
            return if n == 1 {
                "1 match".to_string()
            } else {
                format!("{n} matches")
            };
        }
        format!("match {}/{}", self.match_index + 1, self.matches.len())
    }

    fn set_header(&mut self) {
        let Some(note) = &self.note else { return };
        let mut header = note.title.clone();
        let label = self.match_label();
        if !label.is_empty() {
            header.push_str(&format!("  · {label}"));
        }
        self.header = header;
        let mut meta: Vec<String> = Vec::new();
        if let Some(m) = note.modified {
            meta.push(format!(
                "modified {}",
                m.with_timezone(&Local).format("%Y-%m-%d %H:%M")
            ));
        }
        if let Some(c) = note.created {
            meta.push(format!(
                "created {}",
                c.with_timezone(&Local).format("%Y-%m-%d")
            ));
        }
        let words = self
            .full_text
            .as_deref()
            .map(|t| t.split_whitespace().count())
            .unwrap_or(0);
        meta.push(format!("{words} words"));
        if note.todos > 0 || note.done > 0 {
            meta.push(format!("tasks {}/{}", note.done, note.todos + note.done));
        }
        if !note.pins.is_empty() {
            meta.push(format!("pinned {}", note.pins.join(", ")));
        }
        self.meta = meta.join(" · ");
    }

    fn ensure_wrapped(&mut self, width: usize) {
        let width = width.max(1);
        if self.wrapped_width == width && (!self.wrapped.is_empty() || self.lines.is_empty()) {
            return;
        }
        self.wrapped_width = width;
        self.wrapped = self
            .lines
            .iter()
            .flat_map(|l| wrap(l, width).into_iter().map(move |row| (row, l.block)))
            .collect();
    }

    pub fn row_count(&mut self, width: usize) -> usize {
        self.ensure_wrapped(width);
        self.wrapped.len()
    }

    pub fn max_scroll(&mut self, width: usize, height: usize) -> usize {
        self.row_count(width).saturating_sub(height.max(1))
    }

    pub fn scroll_by(&mut self, delta: i64, width: usize, height: usize) {
        let max = self.max_scroll(width, height) as i64;
        self.scroll = (self.scroll as i64 + delta).clamp(0, max.max(0)) as usize;
    }

    pub fn scroll_to_end(&mut self, width: usize, height: usize) {
        self.scroll = self.max_scroll(width, height);
    }

    /// The rows on screen for a viewport, search matches highlighted.
    pub fn visible(&mut self, width: usize, height: usize) -> Vec<Line<'static>> {
        if let Some(message) = &self.message {
            return vec![Line::from(ratatui::text::Span::styled(
                message.clone(),
                ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::ITALIC),
            ))];
        }
        self.ensure_wrapped(width);
        let max = self.wrapped.len().saturating_sub(height.max(1));
        self.scroll = self.scroll.min(max);
        let pattern = self.pattern.clone();
        self.wrapped
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|(line, _)| match &pattern {
                Some(p) => highlight_line(line.clone(), p),
                None => line.clone(),
            })
            .collect()
    }

    /// Plain text of the whole rendered note, for tests.
    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(RLine::plain)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "# Topic\n#topic\n\nPreamble.\n\n## September 19, 2026 (Saturday)\n\
                        #log/2026/09/19\n* People:\n\n---\n\nSeptember 12, 2026 (Saturday)\n\n\
                        ## September 12, 2026 (Saturday)\n\
                        #log/2026/09/12\n* People:\n\n---\n";

    #[test]
    fn scrolling_to_a_heading_lands_on_its_block() {
        let mut reader = Reader::default();
        reader.show(&Note::default(), BODY);
        assert_eq!(reader.scroll, 0);
        // A body line above it says exactly the same thing; the heading wins.
        assert!(reader.scroll_to_heading("## September 12, 2026 (Saturday)", 60));
        let at = reader.scroll;
        assert!(at > 0, "the second section is below the first");
        let body_line = reader
            .plain_text()
            .lines()
            .position(|l| l.trim() == "September 12, 2026 (Saturday)")
            .unwrap();
        assert!(
            at > body_line,
            "the paragraph that repeats the date is above the heading, not the landing spot"
        );
        // The `#` markers are optional: the rendered line carries the text only.
        assert!(reader.scroll_to_heading("September 12, 2026 (Saturday)", 60));
        assert_eq!(reader.scroll, at);
        // A heading the note does not have leaves the scroll alone.
        assert!(!reader.scroll_to_heading("## Not in this note", 60));
        assert_eq!(reader.scroll, at);
        assert!(!reader.scroll_to_heading("  ", 60));
    }
}
