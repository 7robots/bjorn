//! Overlays: confirm dialog, format picker, text prompt, new-note prompt,
//! help, and the toast queue.

use std::time::{Duration, Instant};

use crate::bear::Note;

pub const HELP_TEXT: &str = "# Bjorn

Three columns: smart views and tags · notes · the rendered note.

| Key | Action |
|---|---|
| `tab` / `shift+tab` | cycle panes |
| `j` `k` / `↑` `↓` | move within a pane |
| `enter` | open the highlighted note in the reader, at the first match while searching |
| `1`–`7` | Notes, Untagged, Todo, Today, Pinned, Archive, Trash |
| `/` | search with Bear syntax (`@todo`, `#tag`, `\"phrase\"`, `-term`); `@` and `#` complete as you type, `tab` or `→` accepts; a bare sub-tag is searched as `#*/name` |
| `esc` | clear the search and its highlights |
| `]` / `[` | next / previous match in the reader while searching |
| `n` | new note (title, tags) then edit |
| `e` | edit in `$VISUAL` / `$EDITOR` |
| `d` | move the note to the trash |
| `u` | restore from Trash or Archive |
| `p` | toggle the global pin |
| `x` | export the note: Markdown, HTML, plain text, RTF or TextBundle (`←` `→` pick, `export_format` sets the default) |
| `b` | open the note in Bear.app |
| `w` | make the highlighted tag the workspace; again to leave it (`W` also clears) |
| `f` | fold / unfold the highlighted tag's subtree |
| `F` | fold every tag, or unfold every tag when all are folded (within the workspace if one is set) |
| `t` | triage: every open todo in the workspace, grouped by note |
| `c` | cycle the columns: hide tags, then notes too, then show all three (or click ▮▮▮ in the note header) |
| `r` | refresh from Bear now |
| `?` | this help · `q` quit (asks first) |

In triage: `space` marks, `x` ticks the marked (or highlighted) todos in Bear, `enter` goes to the note, `b` opens it in Bear at the section, `/` filters, `r` reloads, `esc` or `q` closes. With `[reminders] enabled = true`, `a` adds marked todos to Apple Reminders and rows show ⏰ (added) or ✓ (completed there).

Edits are hash-guarded: if the note changed in Bear while you were in the editor, nothing is written and your version is kept in a temp file.
";

/// What a confirmed dialog goes on to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Quit,
    Trash(Note),
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
    /// Title and tags for a new note; `field` is 0 for the title, 1 for the tags.
    NewNote {
        title: Field,
        tags: Field,
        field: usize,
    },
}

impl Overlay {
    pub fn name(&self) -> &'static str {
        match self {
            Overlay::Confirm { .. } => "Confirm",
            Overlay::Help { .. } => "Help",
            Overlay::Format { .. } => "Format",
            Overlay::Text { .. } => "Text",
            Overlay::NewNote { .. } => "NewNote",
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
