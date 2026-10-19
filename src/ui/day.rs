//! The day screen: every dated section written on one day, across all notes.
//!
//! Bear's tag view lists notes, not sections, so a day tag answers "which notes
//! did I touch" but not "what did I write". This screen reads the notes the day
//! tag found and lists the sections inside them, grouped by note. Like triage,
//! it owns display state only; the app owns the clients.

use std::collections::HashMap;

use chrono::NaiveDate;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::sections::{DayRow, DayScan};
use crate::ui::modals::Field;
use crate::ui::theme;

/// One line of the list: a note header (not selectable) or a section (an index
/// into `rows`).
#[derive(Debug, Clone, PartialEq)]
pub enum DayLine {
    Blank,
    Header {
        title: String,
        count: usize,
        archived: bool,
    },
    Section(usize),
}

#[derive(Debug, Clone)]
pub struct DayView {
    pub date: NaiveDate,
    /// The day tag as Bear writes it, for the header and the empty message.
    pub tag: String,
    /// How the date reads in the header, from the configured heading format.
    pub label: String,
    pub rows: Vec<DayRow>,
    pub locked: usize,
    pub notes: usize,
    pub error: String,
    pub filter_text: String,
    /// The filter box while it is open.
    pub filter: Option<Field>,
    /// Indices into `rows` of the sections currently listed, in order.
    pub items: Vec<usize>,
    pub cursor: usize,
    pub scroll: usize,
    pub status: String,
    pub loaded: bool,
}

impl DayView {
    pub fn new(date: NaiveDate, tag: &str, label: &str) -> DayView {
        DayView {
            date,
            tag: tag.to_string(),
            label: label.to_string(),
            rows: Vec::new(),
            locked: 0,
            notes: 0,
            error: String::new(),
            filter_text: String::new(),
            filter: None,
            items: Vec::new(),
            cursor: 0,
            scroll: 0,
            status: "loading…".to_string(),
            loaded: false,
        }
    }

    pub fn header(&self) -> String {
        format!("DAY · {} · {}", self.label, self.tag)
    }

    /// What the list says when the day is empty.
    pub fn empty_message(&self) -> String {
        if self.filter_text.is_empty() {
            format!(
                "Nothing written on {} — no section carries {}",
                self.label, self.tag
            )
        } else {
            format!(
                "No section on {} matches “{}”",
                self.label, self.filter_text
            )
        }
    }

    /// Replace the rows from a scan, keeping the cursor where the key survives.
    pub fn show(&mut self, scan: DayScan, error: &str) {
        let current = self.current_row().map(key);
        self.rows = scan.rows;
        self.locked = scan.locked;
        self.notes = scan.notes;
        self.error = error.to_string();
        self.loaded = true;
        self.rebuild(current.as_deref());
    }

    fn matches_filter(&self, row: &DayRow) -> bool {
        let needle = self.filter_text.trim().to_lowercase();
        needle.is_empty()
            || row.note_title.to_lowercase().contains(&needle)
            || row.heading.to_lowercase().contains(&needle)
            || row.snippet.to_lowercase().contains(&needle)
    }

    pub fn rebuild(&mut self, keep_key: Option<&str>) {
        self.items = (0..self.rows.len())
            .filter(|i| self.matches_filter(&self.rows[*i]))
            .collect();
        self.cursor = keep_key
            .and_then(|k| self.items.iter().position(|i| key(&self.rows[*i]) == k))
            .unwrap_or(0)
            .min(self.items.len().saturating_sub(1));
        self.update_status();
    }

    fn update_status(&mut self) {
        let mut parts = vec![
            format!(
                "{} section{}",
                self.rows.len(),
                if self.rows.len() == 1 { "" } else { "s" }
            ),
            format!(
                "{} note{}",
                self.notes,
                if self.notes == 1 { "" } else { "s" }
            ),
        ];
        if self.items.len() != self.rows.len() {
            parts.push(format!("{} shown", self.items.len()));
        }
        if self.locked > 0 {
            parts.push(format!("{} locked skipped", self.locked));
        }
        if !self.filter_text.is_empty() {
            parts.push(format!("filter “{}”", self.filter_text));
        }
        if !self.error.is_empty() {
            parts.push(self.error.clone());
        }
        self.status = parts.join(" · ");
    }

    pub fn current_row(&self) -> Option<&DayRow> {
        self.items.get(self.cursor).and_then(|i| self.rows.get(*i))
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let next = (self.cursor as i64 + delta as i64).clamp(0, self.items.len() as i64 - 1);
        self.cursor = next as usize;
    }

    pub fn open_filter(&mut self) {
        self.filter = Some(Field::new(&self.filter_text));
    }

    pub fn close_filter(&mut self) {
        self.filter = None;
    }

    pub fn apply_filter(&mut self) {
        if let Some(field) = self.filter.take() {
            self.filter_text = field.value.trim().to_string();
            let keep = self.current_row().map(key);
            self.rebuild(keep.as_deref());
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        let keep = self.current_row().map(key);
        self.rebuild(keep.as_deref());
    }

    /// The list as lines: a blank line and a note header before each note's
    /// first section, then the sections.
    pub fn lines(&self) -> Vec<DayLine> {
        let mut per_note: HashMap<&str, usize> = HashMap::new();
        for i in &self.items {
            *per_note.entry(self.rows[*i].note_id.as_str()).or_default() += 1;
        }
        let mut out = Vec::new();
        let mut last: Option<&str> = None;
        for i in &self.items {
            let row = &self.rows[*i];
            if last != Some(row.note_id.as_str()) {
                out.push(DayLine::Blank);
                out.push(DayLine::Header {
                    title: row.note_title.clone(),
                    count: per_note[row.note_id.as_str()],
                    archived: row.archived,
                });
                last = Some(&row.note_id);
            }
            out.push(DayLine::Section(*i));
        }
        out
    }

    /// Render one section row: its heading, then the snippet.
    ///
    /// Most sections are headed by the date, which the screen's own header
    /// already says, so that heading is left off and the snippet gets the
    /// width. A section headed some other way keeps its heading.
    pub fn render_row(&self, row: &DayRow, cursor: Option<Style>) -> Line<'static> {
        let base = cursor.unwrap_or_default();
        let dim = if cursor.is_some() { base } else { theme::dim() };
        let mut spans: Vec<Span<'static>> = vec![Span::styled("§ ", dim)];
        let header = row.header();
        if header != self.label || row.snippet.is_empty() {
            spans.push(Span::styled(header, base));
            if !row.snippet.is_empty() {
                spans.push(Span::styled(format!("  {}", row.snippet), dim));
            }
        } else {
            spans.push(Span::styled(row.snippet.clone(), base));
        }
        Line::from(spans)
    }

    pub fn render_header(title: &str, count: usize, archived: bool) -> Line<'static> {
        let mut spans = vec![
            Span::styled(title.to_string(), theme::bold()),
            Span::styled(format!("  {count}"), theme::dim()),
        ];
        if archived {
            spans.push(Span::styled("  (archived)", theme::dim()));
        }
        Line::from(spans)
    }
}

/// What keeps the cursor on the same section across a reload.
fn key(row: &DayRow) -> String {
    format!("{}\n{}", row.note_id, row.heading)
}
