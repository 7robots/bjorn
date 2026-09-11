//! The triage screen: every open todo in scope, grouped by note.
//!
//! The screen owns display state only; every action is answered by the app,
//! which owns the clients and the main snapshot.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::bear::display_tag;
use crate::reminders::Status;
use crate::todos::{Todo, TodoScan};
use crate::ui::modals::Field;
use crate::ui::theme;

pub fn status_glyph(status: Status) -> &'static str {
    match status {
        Status::New => " ",
        Status::Added => "⏰",
        Status::Done => "✓",
    }
}

/// One todo on the screen, with what became of it in Reminders (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct TriageRow {
    pub todo: Todo,
    pub status: Status,
    pub reminder_id: Option<i64>,
    pub marked: bool,
}

/// One line of the list: a note header (not selectable) or a todo (an index into `rows`).
#[derive(Debug, Clone, PartialEq)]
pub enum TriageLine {
    Blank,
    Header {
        title: String,
        count: usize,
        tags: Vec<String>,
    },
    Todo(usize),
}

#[derive(Debug, Clone)]
pub struct Triage {
    pub scope_label: String,
    pub rows: Vec<TriageRow>,
    pub locked: usize,
    pub notes: usize,
    pub reminders_enabled: bool,
    pub reminders_error: String,
    pub filter_text: String,
    /// The filter box while it is open.
    pub filter: Option<Field>,
    /// Indices into `rows` of the todos currently listed, in order.
    pub items: Vec<usize>,
    pub cursor: usize,
    pub scroll: usize,
    pub status: String,
    pub loaded: bool,
}

impl Triage {
    pub fn new(scope_label: &str, reminders_enabled: bool) -> Triage {
        Triage {
            scope_label: scope_label.to_string(),
            rows: Vec::new(),
            locked: 0,
            notes: 0,
            reminders_enabled,
            reminders_error: String::new(),
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
        format!("TRIAGE · {}", self.scope_label)
    }

    /// Replace the rows from a scan, keeping marks and cursor where the keys survive.
    pub fn show(&mut self, scan: TodoScan, statuses: &HashMap<String, (Status, i64)>, error: &str) {
        let marks: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.marked)
            .map(|r| r.todo.key())
            .collect();
        let current_key = self.current_row().map(|r| r.todo.key());
        self.rows = scan
            .todos
            .into_iter()
            .map(|todo| {
                let key = todo.key();
                let (status, rid) = statuses
                    .get(&key)
                    .map(|(s, id)| (*s, Some(*id)))
                    .unwrap_or((Status::New, None));
                TriageRow {
                    marked: marks.contains(&key),
                    todo,
                    status,
                    reminder_id: rid,
                }
            })
            .collect();
        self.locked = scan.locked;
        self.notes = scan.notes;
        self.reminders_error = error.to_string();
        self.loaded = true;
        self.rebuild(current_key.as_deref());
    }

    fn matches_filter(&self, row: &TriageRow) -> bool {
        let needle = self.filter_text.trim().to_lowercase();
        needle.is_empty()
            || row.todo.text.to_lowercase().contains(&needle)
            || row.todo.note_title.to_lowercase().contains(&needle)
    }

    pub fn rebuild(&mut self, keep_key: Option<&str>) {
        self.items = (0..self.rows.len())
            .filter(|i| self.matches_filter(&self.rows[*i]))
            .collect();
        self.cursor = keep_key
            .and_then(|key| {
                self.items
                    .iter()
                    .position(|i| self.rows[*i].todo.key() == key)
            })
            .unwrap_or(0)
            .min(self.items.len().saturating_sub(1));
        self.update_status();
    }

    pub fn marked(&self) -> Vec<TriageRow> {
        self.rows.iter().filter(|r| r.marked).cloned().collect()
    }

    fn update_status(&mut self) {
        let mut parts = vec![
            format!("{} open", self.rows.len()),
            format!("{} notes", self.notes),
        ];
        let marked = self.rows.iter().filter(|r| r.marked).count();
        if marked > 0 {
            parts.push(format!("{marked} marked"));
        }
        if self.locked > 0 {
            parts.push(format!("{} locked skipped", self.locked));
        }
        if !self.filter_text.is_empty() {
            parts.push(format!("filter “{}”", self.filter_text));
        }
        if self.reminders_enabled {
            let added = self
                .rows
                .iter()
                .filter(|r| r.status == Status::Added)
                .count();
            let done = self
                .rows
                .iter()
                .filter(|r| r.status == Status::Done)
                .count();
            parts.push(format!("reminders: {added} added, {done} completed"));
        }
        if !self.reminders_error.is_empty() {
            parts.push(self.reminders_error.clone());
        }
        self.status = parts.join(" · ");
    }

    pub fn current_row(&self) -> Option<&TriageRow> {
        self.items.get(self.cursor).and_then(|i| self.rows.get(*i))
    }

    pub fn visible_texts(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|i| self.rows[*i].todo.text.clone())
            .collect()
    }

    pub fn move_cursor(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let next = (self.cursor as i64 + delta as i64).clamp(0, self.items.len() as i64 - 1);
        self.cursor = next as usize;
    }

    pub fn select_key(&mut self, key: &str) -> bool {
        match self
            .items
            .iter()
            .position(|i| self.rows[*i].todo.key() == key)
        {
            Some(i) => {
                self.cursor = i;
                true
            }
            None => false,
        }
    }

    pub fn toggle_mark(&mut self) {
        if let Some(i) = self.items.get(self.cursor).copied() {
            self.rows[i].marked = !self.rows[i].marked;
            self.update_status();
        }
    }

    /// The marked rows, else the highlighted one.
    pub fn targets(&self) -> Vec<TriageRow> {
        let marked = self.marked();
        if !marked.is_empty() {
            return marked;
        }
        self.current_row().cloned().into_iter().collect()
    }

    pub fn unmark(&mut self, keys: &[String]) {
        for row in &mut self.rows {
            if keys.contains(&row.todo.key()) {
                row.marked = false;
            }
        }
        self.update_status();
    }

    /// Drop rows after a successful tick and rebuild the list with them gone.
    /// The cursor lands on the next surviving todo at or below it, else the
    /// nearest one above.
    pub fn note_removed(&mut self, keys: &[String]) {
        let listed: Vec<String> = self
            .items
            .iter()
            .map(|i| self.rows[*i].todo.key())
            .collect();
        let below = listed.iter().skip(self.cursor).find(|k| !keys.contains(k));
        let above = listed
            .iter()
            .take(self.cursor)
            .rev()
            .find(|k| !keys.contains(k));
        let keep = below.or(above).cloned();
        self.rows.retain(|r| !keys.contains(&r.todo.key()));
        self.rebuild(keep.as_deref());
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
            let keep = self.current_row().map(|r| r.todo.key());
            self.rebuild(keep.as_deref());
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        let keep = self.current_row().map(|r| r.todo.key());
        self.rebuild(keep.as_deref());
    }

    /// The list as lines: a blank line and a note header before each note's
    /// first todo, then the todos.
    pub fn lines(&self) -> Vec<TriageLine> {
        let mut per_note: HashMap<&str, usize> = HashMap::new();
        for i in &self.items {
            *per_note
                .entry(self.rows[*i].todo.note_id.as_str())
                .or_default() += 1;
        }
        let mut out = Vec::new();
        let mut last_note: Option<&str> = None;
        for i in &self.items {
            let todo = &self.rows[*i].todo;
            if last_note != Some(todo.note_id.as_str()) {
                let tags: Vec<String> = todo
                    .note_tags
                    .iter()
                    .filter(|t| {
                        !todo
                            .note_tags
                            .iter()
                            .any(|o| o != *t && o.starts_with(&format!("{t}/")))
                    })
                    .take(2)
                    .map(|t| display_tag(t))
                    .collect();
                out.push(TriageLine::Blank);
                out.push(TriageLine::Header {
                    title: todo.note_title.clone(),
                    count: per_note[todo.note_id.as_str()],
                    tags,
                });
                last_note = Some(&todo.note_id);
            }
            out.push(TriageLine::Todo(*i));
        }
        out
    }

    /// Render one todo row.
    pub fn render_row(&self, row: &TriageRow, cursor: Option<Style>) -> Line<'static> {
        let base = cursor.unwrap_or_default();
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(
            if row.marked { "● " } else { "  " },
            base.fg(if row.marked {
                Color::Yellow
            } else {
                Color::Reset
            }),
        ));
        if self.reminders_enabled {
            let color = match row.status {
                Status::Done => Color::Green,
                Status::Added => Color::Cyan,
                Status::New => Color::Reset,
            };
            spans.push(Span::styled(
                format!("{} ", status_glyph(row.status)),
                base.fg(color),
            ));
        }
        spans.push(Span::styled(
            "☐ ",
            if cursor.is_some() { base } else { theme::dim() },
        ));
        let text_style = if row.status == Status::Done {
            base.add_modifier(Modifier::CROSSED_OUT | Modifier::DIM)
        } else {
            base
        };
        spans.push(Span::styled(row.todo.text.clone(), text_style));
        // The note's own H1 is the section for items above any subheading;
        // repeating the title under its header says nothing.
        let header = row.todo.header();
        if !header.is_empty() && !row.todo.section.starts_with("# ") {
            spans.push(Span::styled(
                format!("  {header}"),
                if cursor.is_some() { base } else { theme::dim() },
            ));
        }
        Line::from(spans)
    }

    pub fn render_header(title: &str, count: usize, tags: &[String]) -> Line<'static> {
        let mut spans = vec![
            Span::styled(title.to_string(), theme::bold()),
            Span::styled(format!("  {count}"), theme::dim()),
        ];
        if !tags.is_empty() {
            spans.push(Span::styled(
                format!("  {}", tags.join(" ")),
                theme::dim().add_modifier(Modifier::ITALIC),
            ));
        }
        Line::from(spans)
    }
}
