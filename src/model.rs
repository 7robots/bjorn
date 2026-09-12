//! Pure functions over a `Snapshot`: views, tag tree, filtering, sorting.
//!
//! Everything the three panes show is derived here from one snapshot, so the
//! tag counts, the smart-filter counts and the notes list always agree with
//! each other. Nothing in this module touches bearcli.

use std::cmp::Reverse;
use std::collections::HashMap;

use chrono::{Local, NaiveDate};

use crate::bear::{Location, Note, Snapshot, normalize_tag};

/// Sidebar entries above the tag tree, in Bear's order plus Pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum View {
    #[default]
    All,
    Untagged,
    Todo,
    Today,
    Pinned,
    Archive,
    Trash,
}

impl View {
    pub const ALL: [View; 7] = [
        View::All,
        View::Untagged,
        View::Todo,
        View::Today,
        View::Pinned,
        View::Archive,
        View::Trash,
    ];

    pub fn value(self) -> &'static str {
        match self {
            View::All => "all",
            View::Untagged => "untagged",
            View::Todo => "todo",
            View::Today => "today",
            View::Pinned => "pinned",
            View::Archive => "archive",
            View::Trash => "trash",
        }
    }

    pub fn from_value(value: &str) -> Option<View> {
        View::ALL.iter().copied().find(|v| v.value() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            View::All => "Notes",
            View::Untagged => "Untagged",
            View::Todo => "Todo",
            View::Today => "Today",
            View::Pinned => "Pinned",
            View::Archive => "Archive",
            View::Trash => "Trash",
        }
    }

    pub fn location(self) -> Location {
        match self {
            View::Archive => Location::Archive,
            View::Trash => Location::Trash,
            _ => Location::Notes,
        }
    }

    /// `1`..`7`, the sidebar's digit keys.
    pub fn hotkey(self) -> char {
        let index = View::ALL.iter().position(|v| *v == self).unwrap_or(0);
        char::from_digit(index as u32 + 1, 10).unwrap()
    }
}

/// What the notes list is showing: a view, optionally narrowed to a tag, always
/// inside the workspace, optionally narrowed further by search ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub view: View,
    pub tag: String,
    pub workspace: String,
    pub search_ids: Option<Vec<String>>,
}

impl Selection {
    /// The tag that actually restricts the list: the selected tag if any, else
    /// the workspace.
    pub fn scope_tag(&self) -> &str {
        if self.tag.is_empty() {
            &self.workspace
        } else {
            &self.tag
        }
    }

    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.tag.is_empty() {
            parts.push(self.view.label().to_string());
        } else {
            parts.push(format!("#{}", self.tag));
        }
        if self.search_ids.is_some() {
            parts.push("search".to_string());
        }
        parts.join(" · ")
    }
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn in_workspace(note: &Note, workspace: &str) -> bool {
    workspace.is_empty() || note.has_tag(workspace)
}

pub fn matches_view(note: &Note, view: View, today: NaiveDate) -> bool {
    if note.location != view.location() {
        return false;
    }
    match view {
        View::Untagged => note.tags.is_empty(),
        View::Todo => note.todos > 0,
        View::Today => note.modified_local_date() == Some(today),
        View::Pinned => note.pinned(),
        _ => true,
    }
}

/// Bear's default: pinned on top, then newest modification first.
pub fn sort_notes(notes: &mut [Note], pinned_first: bool) {
    notes.sort_by_cached_key(|note| {
        let modified = note.modified.map(|m| m.timestamp_millis()).unwrap_or(0);
        (
            u8::from(!(pinned_first && note.pinned())),
            Reverse(modified),
            note.title.to_lowercase(),
        )
    });
}

/// The notes list for a selection, sorted.
pub fn select_notes(snapshot: &Snapshot, selection: &Selection, today: NaiveDate) -> Vec<Note> {
    let tag = normalize_tag(selection.scope_tag());
    let order: Option<HashMap<&str, usize>> = selection.search_ids.as_ref().map(|ids| {
        ids.iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect()
    });
    let mut picked: Vec<Note> = snapshot
        .notes
        .iter()
        .filter(|n| matches_view(n, selection.view, today))
        .filter(|n| tag.is_empty() || n.has_tag(&tag))
        .filter(|n| in_workspace(n, &selection.workspace))
        .filter(|n| order.as_ref().is_none_or(|o| o.contains_key(n.id.as_str())))
        .cloned()
        .collect();
    match order {
        Some(order) => {
            picked.sort_by_key(|n| order.get(n.id.as_str()).copied().unwrap_or(order.len()))
        }
        None => sort_notes(&mut picked, true),
    }
    picked
}

pub fn view_counts(snapshot: &Snapshot, workspace: &str, today: NaiveDate) -> HashMap<View, usize> {
    let mut counts: HashMap<View, usize> = View::ALL.iter().map(|v| (*v, 0)).collect();
    for note in &snapshot.notes {
        if !in_workspace(note, workspace) {
            continue;
        }
        for view in View::ALL {
            if matches_view(note, view, today) {
                *counts.entry(view).or_default() += 1;
            }
        }
    }
    counts
}

/// One tag in the nested tree, with the count of notes carrying it or a child.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagNode {
    pub name: String,
    pub path: String,
    pub count: usize,
    pub children: HashMap<String, TagNode>,
}

impl TagNode {
    pub fn sorted_children(&self) -> Vec<&TagNode> {
        let mut children: Vec<&TagNode> = self.children.values().collect();
        children.sort_by_cached_key(|c| c.name.to_lowercase());
        children
    }

    /// Every descendant, depth first, children sorted case-insensitively.
    pub fn walk(&self) -> Vec<&TagNode> {
        let mut out = Vec::new();
        for child in self.sorted_children() {
            out.push(child);
            out.extend(child.walk());
        }
        out
    }
}

/// Nested tags from the active notes.
///
/// bearcli lists ancestors alongside leaf tags (`#a`, `#a/b`, `#a/b/c`), so a
/// note counts once for each level it carries. With a workspace, the root is
/// the workspace tag itself and only its subtree is present.
pub fn build_tag_tree(snapshot: &Snapshot, workspace: &str, location: Location) -> TagNode {
    let mut root = TagNode::default();
    let workspace = normalize_tag(workspace);
    let prefix = format!("{workspace}/");
    for note in &snapshot.notes {
        if note.location != location || !in_workspace(note, &workspace) {
            continue;
        }
        for tag in &note.tags {
            let outside = !workspace.is_empty() && *tag != workspace && !tag.starts_with(&prefix);
            if outside {
                continue;
            }
            let parts: Vec<&str> = tag.split('/').collect();
            let mut node = &mut root;
            for depth in 0..parts.len() {
                let path = parts[..=depth].join("/");
                node = node
                    .children
                    .entry(parts[depth].to_string())
                    .or_insert_with(|| TagNode {
                        name: parts[depth].to_string(),
                        path,
                        ..TagNode::default()
                    });
            }
            node.count += 1;
        }
    }
    root
}

pub fn tag_count(tree: &TagNode, tag: &str) -> usize {
    let mut node = tree;
    for part in normalize_tag(tag).split('/') {
        match node.children.get(part) {
            Some(child) => node = child,
            None => return 0,
        }
    }
    node.count
}

/// Titles among `titles` that more than one active note carries -> ids.
///
/// The iCloud sync trap: a whole-note overwrite while Bear is syncing can
/// resurrect the previous version as a second note with the same title.
pub fn duplicate_titles(snapshot: &Snapshot, titles: &[String]) -> HashMap<String, Vec<String>> {
    let wanted: std::collections::HashSet<String> =
        titles.iter().map(|t| t.to_lowercase()).collect();
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for note in snapshot.in_location(Location::Notes) {
        let key = note.title.to_lowercase();
        if wanted.contains(&key) {
            seen.entry(key).or_default().push(note.id.clone());
        }
    }
    seen.retain(|_, ids| ids.len() > 1);
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bear::parse_time_str;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 8).unwrap()
    }

    fn mk(
        id: &str,
        title: &str,
        tags: &[&str],
        modified: &str,
        pins: &[&str],
        todos: i64,
        location: Location,
    ) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            pins: pins.iter().map(|p| p.to_string()).collect(),
            todos,
            location,
            modified: parse_time_str(modified),
            ..Note::default()
        }
    }

    fn snap() -> Snapshot {
        Snapshot::new(vec![
            mk(
                "1",
                "A",
                &["work", "work/sprint"],
                "2026-09-08T12:00:00Z",
                &["global"],
                2,
                Location::Notes,
            ),
            mk(
                "2",
                "B",
                &["home", "home/garden"],
                "2026-09-07T12:00:00Z",
                &["#home"],
                0,
                Location::Notes,
            ),
            mk(
                "3",
                "C",
                &["home"],
                "2026-09-06T12:00:00Z",
                &[],
                0,
                Location::Notes,
            ),
            mk(
                "4",
                "D",
                &[],
                "2026-09-05T12:00:00Z",
                &[],
                0,
                Location::Notes,
            ),
            mk(
                "5",
                "E",
                &["work"],
                "2026-09-01T12:00:00Z",
                &[],
                0,
                Location::Trash,
            ),
            mk(
                "6",
                "F",
                &["work"],
                "2026-09-01T12:00:00Z",
                &[],
                0,
                Location::Archive,
            ),
            mk(
                "7",
                "A",
                &["work"],
                "2026-09-04T12:00:00Z",
                &[],
                0,
                Location::Notes,
            ),
        ])
    }

    fn ids(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn view_counts_and_pinned_means_any_pin() {
        // Today is judged in local time; 12:00 UTC on the 8th is the 8th anywhere in the Americas and Europe.
        let counts = view_counts(&snap(), "", day());
        assert_eq!(counts[&View::All], 5);
        assert_eq!(counts[&View::Untagged], 1);
        assert_eq!(counts[&View::Todo], 1);
        assert_eq!(counts[&View::Today], 1);
        assert_eq!(counts[&View::Pinned], 2);
        assert_eq!(counts[&View::Archive], 1);
        assert_eq!(counts[&View::Trash], 1);
    }

    #[test]
    fn workspace_scopes_counts_and_tree() {
        let counts = view_counts(&snap(), "home", day());
        assert_eq!(counts[&View::All], 2);
        assert_eq!(counts[&View::Untagged], 0);
        assert_eq!(counts[&View::Pinned], 1);
        let tree = build_tag_tree(&snap(), "home", Location::Notes);
        assert_eq!(
            tree.sorted_children()
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            vec!["home"]
        );
        assert_eq!(tag_count(&tree, "home"), 2);
        assert_eq!(tag_count(&tree, "home/garden"), 1);
        let full = build_tag_tree(&snap(), "", Location::Notes);
        assert_eq!(
            full.sorted_children()
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            vec!["home", "work"]
        );
        assert_eq!(tag_count(&full, "work"), 2);
        assert_eq!(
            full.walk()
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>(),
            vec!["home", "home/garden", "work", "work/sprint"]
        );
    }

    #[test]
    fn select_notes_sorts_pinned_first_then_newest() {
        assert_eq!(
            ids(&select_notes(&snap(), &Selection::default(), day())),
            vec!["1", "2", "3", "4", "7"]
        );
        let by_tag = Selection {
            tag: "home".into(),
            ..Selection::default()
        };
        assert_eq!(ids(&select_notes(&snap(), &by_tag, day())), vec!["2", "3"]);
        let trash = Selection {
            view: View::Trash,
            ..Selection::default()
        };
        assert_eq!(ids(&select_notes(&snap(), &trash, day())), vec!["5"]);
        let todo_home = Selection {
            view: View::Todo,
            workspace: "home".into(),
            ..Selection::default()
        };
        assert!(select_notes(&snap(), &todo_home, day()).is_empty());
    }

    #[test]
    fn search_ids_keep_bearcli_order_and_respect_scope() {
        let sel = Selection {
            search_ids: Some(vec!["4".into(), "2".into(), "5".into(), "1".into()]),
            ..Selection::default()
        };
        assert_eq!(
            ids(&select_notes(&snap(), &sel, day())),
            vec!["4", "2", "1"]
        );
        let sel = Selection {
            workspace: "home".into(),
            search_ids: Some(vec!["4".into(), "2".into(), "1".into()]),
            ..Selection::default()
        };
        assert_eq!(ids(&select_notes(&snap(), &sel, day())), vec!["2"]);
    }

    #[test]
    fn duplicate_titles_only_among_active_notes() {
        let dups = duplicate_titles(&snap(), &["a".to_string()]);
        assert_eq!(dups["a"], vec!["1", "7"]);
        assert!(duplicate_titles(&snap(), &["e".to_string()]).is_empty());
        let sel = Selection {
            tag: "x".into(),
            search_ids: Some(vec![]),
            ..Selection::default()
        };
        assert_eq!(sel.describe(), "#x · search");
        assert_eq!(View::Trash.hotkey(), '7');
        assert_eq!(View::from_value("todo"), Some(View::Todo));
    }
}
