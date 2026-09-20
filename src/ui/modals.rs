//! Overlays: confirm dialog, format picker, text prompt, new-note prompt,
//! and the toast queue. The help overlay's text lives in `ui::help`.

use std::time::{Duration, Instant};

use crate::actions::Action;
use crate::bear::Note;
use crate::templates::Template;

/// What a confirmed dialog goes on to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Quit,
    Trash(Note),
    /// An action whose config says `confirm = true`.
    RunAction(Action, Note),
    /// Deleting an action from the config; cancelling goes back to the menu.
    DeleteAction(Action, Note),
    Tick(Vec<crate::ui::triage::TriageRow>),
}

/// What a submitted text prompt goes on to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TextPurpose {
    ExportPath { format_id: &'static str, note: Note },
}

/// A one-line text field with a cursor, shared by the prompts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    pub value: String,
    pub cursor: usize,
}

impl Field {
    pub fn new(value: &str) -> Field {
        Field {
            value: value.to_string(),
            cursor: value.chars().count(),
        }
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, ch: char) {
        let at = self.byte_index(self.cursor);
        self.value.insert(at, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let at = self.byte_index(self.cursor - 1);
            self.value.remove(at);
            self.cursor -= 1;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.value.chars().count() {
            let at = self.byte_index(self.cursor);
            self.value.remove(at);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.chars().count();
    }
}

/// Rows in the new-action form: name, command, format, confirm, default.
pub const NEW_ACTION_FIELDS: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Confirm {
        message: String,
        confirm_label: String,
        action: Pending,
    },
    Help {
        scroll: usize,
    },
    /// Pick an export format: one key per format, or arrows and enter.
    Format {
        index: usize,
        note: Note,
    },
    /// One line of text; escape cancels.
    Text {
        title: String,
        field: Field,
        hint: String,
        purpose: TextPurpose,
    },
    /// The action palette: a search box over the actions from the config.
    Actions {
        field: Field,
        index: usize,
        note: Note,
    },
    /// The form for a new action (the menu's last row) or for editing one
    /// (`ctrl+e`); saving writes the config.
    /// `focus` is the row: 0 name, 1 command, 2 format, 3 confirm, 4 default.
    NewAction {
        name: Field,
        command: Field,
        /// An index into `export::FORMATS`.
        format: usize,
        confirm: bool,
        default: bool,
        focus: usize,
        note: Note,
        /// The action being edited, as it was read; `None` for a new one.
        editing: Option<Action>,
    },
    /// Title and tags for a new note; `field` is 0 for the title, 1 for the tags.
    /// `template` is the one picked with `N`, whose body the note starts from.
    NewNote {
        title: Field,
        tags: Field,
        field: usize,
        template: Option<Template>,
    },
    /// `N`: a search box over the templates directory, read when it opened.
    Templates {
        field: Field,
        index: usize,
        templates: Vec<Template>,
        /// `*.md` files that could not be used (too big, not UTF-8, ...).
        skipped: usize,
    },
}

/// The templates whose name holds `query`, ignoring case, in listed order.
pub fn filter_templates<'a>(templates: &'a [Template], query: &str) -> Vec<&'a Template> {
    let query = query.trim().to_lowercase();
    templates
        .iter()
        .filter(|t| query.is_empty() || t.name.to_lowercase().contains(&query))
        .collect()
}

impl Overlay {
    pub fn name(&self) -> &'static str {
        match self {
            Overlay::Confirm { .. } => "Confirm",
            Overlay::Help { .. } => "Help",
            Overlay::Format { .. } => "Format",
            Overlay::Text { .. } => "Text",
            Overlay::Actions { .. } => "Actions",
            Overlay::NewAction { .. } => "NewAction",
            Overlay::NewNote { .. } => "NewNote",
            Overlay::Templates { .. } => "Templates",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Information,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub title: String,
    pub message: String,
    pub severity: Severity,
    pub expires: Instant,
}

impl Toast {
    pub fn new(title: &str, message: &str, severity: Severity, timeout: Duration) -> Toast {
        Toast {
            title: title.to_string(),
            message: message.to_string(),
            severity,
            expires: Instant::now() + timeout,
        }
    }
}
