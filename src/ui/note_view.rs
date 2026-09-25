//! Right column: the rendered note.

use chrono::Local;
use ratatui::text::Line;
use regex::Regex;

use crate::bear::Note;
use crate::ui::highlight::highlight_line;
use crate::ui::markdown::{Heading, Placed, RLine, render_with_headings, wrap_mapped};
use crate::wiki::WikiLink;

/// Glyph per column count: a hollow block for each hidden column.
pub fn column_glyph(count: u8) -> &'static str {
    match count {
        1 => "▯▯▮",
        2 => "▯▮▮",
        _ => "▮▮▮",
    }
}

/// One screen row of the wrapped note: what it draws, the block and line it
/// came from, and where that line's spans landed on it.
#[derive(Debug, Clone)]
struct Row {
    line: Line<'static>,
    block: usize,
    source: usize,
    placed: Vec<Placed>,
}

/// The headings the outline lists for `query`: their indices into
/// `headings`, in document order. Case-insensitive substring match; an empty
/// query lists them all.
pub fn outline_filter(headings: &[Heading], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    headings
        .iter()
        .enumerate()
        .filter(|(_, h)| query.is_empty() || h.text.to_lowercase().contains(&query))
        .map(|(i, _)| i)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct Reader {
    pub note: Option<Note>,
    pub full_text: Option<String>,
    lines: Vec<RLine>,
    wrapped: Vec<Row>,
    wrapped_width: usize,
    pub scroll: usize,
    pub header: String,
    pub meta: String,
    pub pattern: Option<Regex>,
    /// Blocks holding a match, in document order, and the index last jumped to (-1: none yet).
    pub matches: Vec<usize>,
    pub match_index: i64,
    /// The note's headings, in document order, from the same parse that drew it.
    pub headings: Vec<Heading>,
    /// The heading last jumped to and the scroll that jump set. It counts as
    /// the current section until the reader scrolls (a heading near the end
    /// cannot reach the top row).
    heading_index: Option<(usize, usize)>,
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
        self.headings.clear();
        self.heading_index = None;
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
        (self.lines, self.headings) = render_with_headings(content);
        self.heading_index = None;
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
        self.headings.clear();
        self.heading_index = None;
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
        if let Some(row) = self.wrapped.iter().position(|r| r.block == block) {
            self.scroll = row;
        }
        self.set_header();
        true
    }

    /// The first wrapped row of heading `index`, at `width`.
    pub fn heading_row(&mut self, index: usize, width: usize) -> Option<usize> {
        let block = self.headings.get(index)?.block;
        self.ensure_wrapped(width);
        // Block ids only grow down the note, so the rows are sorted by block.
        let row = self.wrapped.partition_point(|r| r.block < block);
        (self.wrapped.get(row).map(|r| r.block) == Some(block)).then_some(row)
    }

    /// The section the viewport is in: the heading last jumped to while it
    /// is still on screen, else the last heading at or above the top row.
    /// None above the first heading.
    pub fn current_heading(&mut self, width: usize, height: usize) -> Option<usize> {
        if self.message.is_some() || self.headings.is_empty() {
            return None;
        }
        let top = self.scroll.min(self.max_scroll(width, height));
        if let Some((i, scroll)) = self.heading_index
            && scroll == self.scroll
            && let Some(row) = self.heading_row(i, width)
            && (top..top + height.max(1)).contains(&row)
        {
            return Some(i);
        }
        let mut current = None;
        for i in 0..self.headings.len() {
            match self.heading_row(i, width) {
                Some(row) if row <= top => current = Some(i),
                Some(_) => break,
                None => {}
            }
        }
        current
    }

    /// The name of the section the viewport is in, for the reader's footer.
    /// None above the first heading and in the note's own title: an H1 on
    /// the first line, which is where Bear takes the title from.
    pub fn section_name(&mut self, width: usize, height: usize) -> Option<String> {
        let index = self.current_heading(width, height)?;
        let heading = &self.headings[index];
        let is_title = index == 0
            && heading.level == 1
            && self
                .full_text
                .as_deref()
                .is_some_and(|text| text.trim_start().starts_with("# "));
        (!is_title).then(|| heading.text.clone())
    }

    /// Scroll so heading `index` is the top row, or as near as the end of
    /// the note allows. False when there is no such heading.
    pub fn scroll_to_heading(&mut self, index: usize, width: usize, height: usize) -> bool {
        let Some(row) = self.heading_row(index, width) else {
            return false;
        };
        self.scroll = row.min(self.max_scroll(width, height));
        self.heading_index = Some((index, self.scroll));
        true
    }

    /// `}` (`delta` 1) or `{` (-1): the next heading below the current
    /// section's, or the start of the current section, then the one before
    /// it. No wrapping. False when there is nowhere to go.
    pub fn jump_heading(&mut self, delta: i64, width: usize, height: usize) -> bool {
        let current = self.current_heading(width, height);
        let target = if delta > 0 {
            current.map_or(0, |i| i + 1)
        } else {
            let Some(i) = current else { return false };
            let top = self.scroll.min(self.max_scroll(width, height));
            let above = self.heading_row(i, width).is_some_and(|row| row < top);
            if above {
                i
            } else if i > 0 {
                i - 1
            } else {
                return false;
            }
        };
        if target >= self.headings.len() {
            return false;
        }
        self.scroll_to_heading(target, width, height)
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
            .enumerate()
            .flat_map(|(source, l)| {
                wrap_mapped(l, width)
                    .into_iter()
                    .map(move |(line, placed)| Row {
                        line,
                        block: l.block,
                        source,
                        placed,
                    })
            })
            .collect();
    }

    /// The wiki link drawn at `col`, `row` of the viewport, if any.
    pub fn link_at(&mut self, col: usize, row: usize, width: usize) -> Option<WikiLink> {
        if self.message.is_some() {
            return None;
        }
        self.ensure_wrapped(width);
        let drawn = self.wrapped.get(self.scroll + row)?;
        let span = drawn
            .placed
            .iter()
            .find(|p| p.start <= col && col < p.end)?
            .span;
        self.lines[drawn.source]
            .links
            .iter()
            .find(|(index, _)| *index == span)
            .map(|(_, link)| link.clone())
    }

    /// Scroll to the first heading called `section`, ignoring case, as
    /// `scroll_to_heading` does. It looks the name up in `headings`, so a
    /// heading inside a quote matches on its text, without the quote bars.
    /// False when the note has no such heading.
    pub fn scroll_to_section(&mut self, section: &str, width: usize, height: usize) -> bool {
        let wanted = section.trim().to_lowercase();
        match self
            .headings
            .iter()
            .position(|h| h.text.to_lowercase() == wanted)
        {
            Some(index) => self.scroll_to_heading(index, width, height),
            None => false,
        }
    }

    /// The note's wiki links in order, as drawn (none from inside code).
    pub fn wiki_links(&self) -> Vec<WikiLink> {
        self.full_text
            .as_deref()
            .map(crate::ui::markdown::wiki_links)
            .unwrap_or_default()
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
            .map(|row| match &pattern {
                Some(p) => highlight_line(row.line.clone(), p),
                None => row.line.clone(),
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
