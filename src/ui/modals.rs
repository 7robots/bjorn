//! Overlays: confirm dialog, format picker, text prompt, new-note prompt,
//! and the toast queue. The help overlay's text lives in `ui::help`.

use std::time::{Duration, Instant};

use crate::actions::Action;
use crate::bear::Note;
use crate::templates::Template;
use crate::wiki::WikiLink;

/// What a confirmed dialog goes on to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Quit,
    Trash(Note),
    /// An action whose config says `confirm = true`. The string is what its
    /// `prompt` collected, empty when it has none.
    RunAction(Action, Note, String),
    /// Deleting an action from the config; cancelling goes back to the menu.
    DeleteAction(Action, Note),
    Tick(Vec<crate::ui::triage::TriageRow>),
    /// A wiki link to a note that does not exist yet: create it.
    CreateLinked(String),
}

/// Where a row of the Links list goes.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// An outgoing link, resolved by title when followed.
    Wiki(WikiLink),
    /// A note that links here.
    Note { id: String },
}

/// One row of the Links list.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRow {
    pub label: String,
    /// Muted text after the label: the target behind an alias, the heading,
    /// the location, or that the note does not exist yet.
    pub detail: String,
    pub target: LinkTarget,
    /// An outgoing link no note answers to; following it offers to create one.
    pub missing: bool,
}

impl LinkRow {
    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.label.to_lowercase().contains(&query)
            || self.detail.to_lowercase().contains(&query)
    }
}

/// The rows of the Links list that match `query`: outgoing, then backlinks.
/// The highlight indexes the two as one list, outgoing first.
pub fn filter_links<'a>(
    outgoing: &'a [LinkRow],
    backlinks: Option<&'a [LinkRow]>,
    query: &str,
) -> (Vec<&'a LinkRow>, Vec<&'a LinkRow>) {
    (
        outgoing.iter().filter(|r| r.matches(query)).collect(),
        backlinks
            .unwrap_or_default()
            .iter()
            .filter(|r| r.matches(query))
            .collect(),
    )
}

/// What a submitted text prompt goes on to do.
#[derive(Debug, Clone, PartialEq)]
pub enum TextPurpose {
    ExportPath {
        format_id: &'static str,
        note: Note,
    },
    /// The answer to an action's `prompt`, on its way to `$BJORN_ACTION_INPUT`.
    /// Empty is allowed: a command can treat "no answer" as its default.
    ActionInput {
        action: Action,
        note: Note,
    },
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

/// Rows in the new-action form: name, command, format, confirm, default,
/// output, section.
pub const NEW_ACTION_FIELDS: usize = 7;

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
    /// The reader's headings under a search box. `index` is the highlighted
    /// row among the ones the search leaves.
    Outline {
        field: Field,
        index: usize,
    },
    /// The form for a new action (the menu's last row) or for editing one
    /// (`ctrl+e`); saving writes the config.
    /// `focus` is the row: 0 name, 1 command, 2 format, 3 confirm, 4 default,
    /// 5 output, 6 section.
    NewAction {
        name: Field,
        command: Field,
        /// An index into `export::FORMATS`; `None` while an edited action's
        /// unknown format stands as written, until `←`/`→` picks a real one.
        format: Option<usize>,
        confirm: bool,
        default: bool,
        /// An index into `actions::ActionOutput::ALL`.
        output: usize,
        /// The heading an `append` goes under; blank for the end of the note.
        section: Field,
        focus: usize,
        note: Note,
        /// The action being edited, as it was read; `None` for a new one.
        editing: Option<Action>,
    },
    /// `L`: the note's wiki links and the notes linking to it, under a search
    /// box. `backlinks` is None while the search for them runs.
    Links {
        field: Field,
        index: usize,
        note: Note,
        /// At most `app::OUTGOING_LIMIT` of the note's links.
        outgoing: Vec<LinkRow>,
        /// Links past that limit, not listed.
        more: usize,
        backlinks: Option<Vec<LinkRow>>,
        /// The backlink search hit its cap, so the list may be incomplete.
        capped: bool,
        /// Why the backlinks could not be read, if they could not.
        error: String,
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
    /// The action the new-action form would save, as it stands.
    ///
    /// An edit keeps what the form does not show: the timeout, the prompt and
    /// `interactive`, which `update_in_config`'s read-back then finds as they
    /// were. A bad `output` value is the exception: choosing an output in the
    /// form is what fixes it, so it must not ride along. A bad `format` rides
    /// along until one is chosen, so saving is refused rather than writing
    /// "md" over it unseen.
    pub fn form_action(&self) -> Option<Action> {
        let Overlay::NewAction {
            name,
            command,
            format,
            confirm,
            default,
            output,
            section,
            editing,
            ..
        } = self
        else {
            return None;
        };
        let heading = section.value.trim();
        let edited = editing.clone().unwrap_or_default();
        let (format, format_error) = match format {
            Some(index) => (crate::export::FORMATS[*index].id.to_string(), None),
            None => (edited.format.clone(), edited.format_error.clone()),
        };
        Some(Action {
            name: name.value.trim().to_string(),
            command: command.value.trim().to_string(),
            format,
            format_error,
            confirm: *confirm,
            default: *default,
            output: crate::actions::ActionOutput::ALL[*output],
            section: (!heading.is_empty()).then(|| heading.to_string()),
            output_error: None,
            ..edited
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Overlay::Confirm { .. } => "Confirm",
            Overlay::Help { .. } => "Help",
            Overlay::Format { .. } => "Format",
            Overlay::Text { .. } => "Text",
            Overlay::Actions { .. } => "Actions",
            Overlay::Outline { .. } => "Outline",
            Overlay::NewAction { .. } => "NewAction",
            Overlay::NewNote { .. } => "NewNote",
            Overlay::Links { .. } => "Links",
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
            title: crate::util::strip_bidi(title),
            message: crate::util::strip_bidi(message),
            severity,
            expires: Instant::now() + timeout,
        }
    }
}
