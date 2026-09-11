//! Open todo items in Bear notes: the model, the parser, and the keys.
//!
//! Ported from remtui so that the key a reminder carries (`bear-todo: <key>`)
//! is identical between the two tools and existing reminders join up unchanged.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use sha1::Digest;

use crate::bear::normalize_tag;
use crate::render::is_fence;

static TODO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<indent>\s*)(?P<bullet>[-*+]) \[ \]\s+(?P<text>\S.*?)\s*$").unwrap()
});
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6} \S").unwrap());

/// One open `- [ ]` line in a Bear note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    pub note_id: String,
    pub note_title: String,
    pub note_tags: Vec<String>,
    pub text: String,
    /// The full line as written, indentation included: what `edit --find` needs.
    pub line: String,
    /// The nearest heading line above the todo, `#` markers and all, or "" when
    /// the todo sits above every heading. Doubles as a bearcli section address.
    pub section: String,
}

impl Todo {
    pub fn new(
        note_id: &str,
        note_title: &str,
        note_tags: &[&str],
        text: &str,
        line: &str,
        section: &str,
    ) -> Todo {
        Todo {
            note_id: note_id.into(),
            note_title: note_title.into(),
            note_tags: note_tags.iter().map(|t| t.to_string()).collect(),
            text: text.into(),
            line: line.into(),
            section: section.into(),
        }
    }

    pub fn normalized(&self) -> String {
        self.text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// Stable id shared with the reminder created from this todo. Rewording
    /// the item in Bear yields a new key (and orphans the old reminder).
    pub fn key(&self) -> String {
        let digest =
            sha1::Sha1::digest(format!("{}\n{}", self.note_id, self.normalized()).as_bytes());
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..12]
            .to_string()
    }

    pub fn done_line(&self) -> String {
        self.line.replacen("[ ]", "[x]", 1)
    }

    /// The section heading without its `#` markers, for `app open --header`.
    pub fn header(&self) -> String {
        self.section.trim_start_matches('#').trim().to_string()
    }
}

/// Extract open todos from a note body, in document order.
///
/// Fenced code blocks are skipped. Nested todos count like top-level ones; a
/// checked parent does not hide its open children.
pub fn parse_todos(
    content: &str,
    note_id: &str,
    note_title: &str,
    note_tags: &[String],
) -> Vec<Todo> {
    let mut todos = Vec::new();
    let mut section = String::new();
    let mut in_fence = false;
    for raw in content.lines() {
        if is_fence(raw) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if HEADING_RE.is_match(raw) {
            section = raw.trim().to_string();
            continue;
        }
        let Some(caps) = TODO_RE.captures(raw) else {
            continue;
        };
        todos.push(Todo {
            note_id: note_id.to_string(),
            note_title: note_title.to_string(),
            note_tags: note_tags.to_vec(),
            text: caps["text"].to_string(),
            line: raw.trim_end().to_string(),
            section: section.clone(),
        });
    }
    todos
}

/// What a triage read of Bear produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoScan {
    pub todos: Vec<Todo>,
    /// Notes whose content bearcli could not read (locked or encrypted).
    pub locked: usize,
    /// Notes with open todos, in list order.
    pub notes: usize,
}

fn is_yes(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "yes" | "true" | "1"),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

/// Turn `bearcli search "@todo" --fields id,title,tags,locked,content` rows
/// into todos, in the order bearcli returned the notes.
pub fn scan_rows(rows: &[Value]) -> TodoScan {
    let mut scan = TodoScan::default();
    for row in rows {
        let content = row.get("content");
        if is_yes(row.get("locked")) || content.is_none_or(Value::is_null) {
            scan.locked += 1;
            continue;
        }
        let title = row
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let tags: Vec<String> = row
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(normalize_tag)
                    .collect()
            })
            .unwrap_or_default();
        let found = parse_todos(
            content.and_then(Value::as_str).unwrap_or(""),
            row.get("id").and_then(Value::as_str).unwrap_or(""),
            if title.is_empty() { "Untitled" } else { title },
            &tags,
        );
        if !found.is_empty() {
            scan.notes += 1;
        }
        scan.todos.extend(found);
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BODY: &str = "# Sprint\n#work\n\n- [ ] top level, no section\n## Tasks\n- [x] done already\n- [ ] write the release notes\n  - [ ] nested child\n* [ ] star bullet\n1. not a todo\n```\n- [ ] inside a fence\n```\n### Sub\n- [ ]  double  spaced   text\n";

    #[test]
    fn parse_todos_in_order_with_sections() {
        let todos = parse_todos(BODY, "N", "Sprint", &["work".to_string()]);
        let texts: Vec<&str> = todos.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "top level, no section",
                "write the release notes",
                "nested child",
                "star bullet",
                "double  spaced   text"
            ]
        );
        let sections: Vec<&str> = todos.iter().map(|t| t.section.as_str()).collect();
        assert_eq!(
            sections,
            vec!["# Sprint", "## Tasks", "## Tasks", "## Tasks", "### Sub"]
        );
        assert_eq!(todos[2].line, "  - [ ] nested child");
        assert_eq!(todos[2].done_line(), "  - [x] nested child");
        assert_eq!(todos[1].header(), "Tasks");
        assert_eq!(todos[0].header(), "Sprint");
    }

    #[test]
    fn key_matches_remtui_scheme_and_ignores_spacing() {
        let a = Todo::new(
            "N",
            "T",
            &[],
            "Write  the notes",
            "- [ ] Write  the notes",
            "",
        );
        let b = Todo::new(
            "N",
            "T",
            &[],
            "write the notes",
            "- [ ] write the notes",
            "",
        );
        let c = Todo::new(
            "M",
            "T",
            &[],
            "write the notes",
            "- [ ] write the notes",
            "",
        );
        assert_eq!(a.key(), b.key());
        assert_eq!(a.key().len(), 12);
        assert_ne!(a.key(), c.key());
        let digest = sha1::Sha1::digest(b"N\nwrite the notes");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(a.key(), hex[..12]);
    }

    #[test]
    fn scan_rows_counts_locked_and_notes() {
        let rows = vec![
            json!({"id": "A", "title": "A", "tags": ["#work"], "locked": "no", "content": "- [ ] one\n- [ ] two\n"}),
            json!({"id": "B", "title": "B", "tags": [], "locked": "yes", "content": null}),
            json!({"id": "C", "title": "C", "tags": ["#home"], "locked": "no", "content": "nothing open\n"}),
        ];
        let scan = scan_rows(&rows);
        assert_eq!(
            scan.todos
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(scan.todos[0].note_tags, vec!["work"]);
        assert_eq!((scan.locked, scan.notes), (1, 1));
    }
}
