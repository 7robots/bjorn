//! Left column: the smart views and the nested tag tree in one list, like
//! Bear's sidebar. One flat row list holds both, so the cursor runs from the
//! last smart view straight into the tags. Between the two groups sit three
//! rows the cursor skips: two blank ones and the TAGS heading.

use std::collections::HashMap;

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::bear::{Location, Snapshot, display_tag};
use crate::icons::IconSet;
use crate::model::{TagNode, View, build_tag_tree, view_counts};
use crate::ui::theme;

pub const TAGS_HEADING: &str = "TAGS";
pub const GAP_ROWS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    View(View),
    Gap,
    Heading,
    Tag {
        path: String,
        name: String,
        depth: usize,
        count: usize,
        has_children: bool,
        expanded: bool,
    },
}

impl Row {
    pub fn selectable(&self) -> bool {
        matches!(self, Row::View(_) | Row::Tag { .. })
    }

    pub fn tag(&self) -> Option<&str> {
        match self {
            Row::Tag { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn view(&self) -> Option<View> {
        match self {
            Row::View(v) => Some(*v),
            _ => None,
        }
    }
}

/// Highlighting a smart view or a tag is a selection, as clicking is in Bear.
#[derive(Debug, Clone)]
pub struct Sidebar {
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub scroll: usize,
    pub view_counts: HashMap<View, usize>,
    pub workspace: String,
    pub icons: IconSet,
    tree: TagNode,
    /// Fold state by tag path, remembered across rebuilds (reloads, entering
    /// and leaving a workspace) so a tree the user folded stays folded.
    expanded: HashMap<String, bool>,
}

impl Sidebar {
    pub fn new(icons: IconSet) -> Sidebar {
        let mut sidebar = Sidebar {
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            view_counts: HashMap::new(),
            workspace: String::new(),
            icons,
            tree: TagNode::default(),
            expanded: HashMap::new(),
        };
        sidebar.rebuild_rows();
        sidebar
    }

    pub fn header(&self) -> String {
        if self.workspace.is_empty() {
            "BJORN".to_string()
        } else {
            format!("WORKSPACE {}", display_tag(&self.workspace))
        }
    }

    pub fn set_workspace(&mut self, workspace: &str) {
        self.workspace = workspace.to_string();
    }

    /// Rebuild the column from a snapshot. The cursor lands on `keep_tag` when
    /// it is still there, else on `keep_view`.
    pub fn populate(
        &mut self,
        snapshot: &Snapshot,
        workspace: &str,
        keep_tag: &str,
        keep_view: View,
    ) {
        self.set_workspace(workspace);
        self.view_counts = view_counts(snapshot, workspace, crate::model::today());
        self.tree = build_tag_tree(snapshot, workspace, Location::Notes);
        // Every tag starts folded, as in a fresh Bear sidebar; a workspace is
        // one subtree, so it opens fully. Tags seen before keep their fold.
        let expand_depth = if workspace.is_empty() { 0 } else { 99 };
        let mut defaults: Vec<(String, bool)> = Vec::new();
        Self::collect_defaults(&self.tree, 0, expand_depth, &mut defaults);
        for (path, expanded) in defaults {
            self.expanded.entry(path).or_insert(expanded);
        }
        self.rebuild_rows();
        if !(keep_tag.is_empty() || self.move_to_tag(keep_tag)) || keep_tag.is_empty() {
            self.select_view(keep_view);
        }
    }

    fn collect_defaults(
        node: &TagNode,
        depth: usize,
        expand_depth: usize,
        out: &mut Vec<(String, bool)>,
    ) {
        for child in node.sorted_children() {
            if !child.children.is_empty() {
                out.push((child.path.clone(), depth < expand_depth));
                Self::collect_defaults(child, depth + 1, expand_depth, out);
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let identity = self.rows.get(self.cursor).cloned();
        let mut rows: Vec<Row> = View::ALL.iter().map(|v| Row::View(*v)).collect();
        rows.extend((0..GAP_ROWS).map(|_| Row::Gap));
        rows.push(Row::Heading);
        let tree = std::mem::take(&mut self.tree);
        self.fill(&tree, 0, &mut rows);
        self.tree = tree;
        self.rows = rows;
        self.cursor = match identity {
            Some(Row::Tag { path, .. }) => self.index_of_tag(&path).unwrap_or(0),
            Some(Row::View(v)) => self.index_of_view(v),
            _ => 0,
        };
    }

    fn fill(&self, node: &TagNode, depth: usize, rows: &mut Vec<Row>) {
        for child in node.sorted_children() {
            let has_children = !child.children.is_empty();
            let expanded = has_children && self.expanded.get(&child.path).copied().unwrap_or(false);
            rows.push(Row::Tag {
                path: child.path.clone(),
                name: child.name.clone(),
                depth,
                count: child.count,
                has_children,
                expanded,
            });
            if expanded {
                self.fill(child, depth + 1, rows);
            }
        }
    }

    fn index_of_tag(&self, tag: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.tag() == Some(tag))
    }

    fn index_of_view(&self, view: View) -> usize {
        self.rows
            .iter()
            .position(|r| r.view() == Some(view))
            .unwrap_or(0)
    }

    /// The top-level tag paths, below the smart views and the heading.
    pub fn tag_roots(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Tag { path, depth: 0, .. } => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every tag path in the tree that has children, folded or not.
    pub fn branches(&self) -> Vec<String> {
        let mut out = Vec::new();
        for node in self.tree.walk() {
            if !node.children.is_empty() {
                out.push(node.path.clone());
            }
        }
        out
    }

    pub fn is_expanded(&self, tag: &str) -> bool {
        self.expanded.get(tag).copied().unwrap_or(false)
    }

    pub fn set_expanded(&mut self, tag: &str, expanded: bool) {
        self.expanded.insert(tag.to_string(), expanded);
        self.rebuild_rows();
    }

    /// Put the cursor on a tag path, expanding ancestors. False if absent.
    pub fn move_to_tag(&mut self, tag: &str) -> bool {
        if self.tree.walk().iter().all(|n| n.path != tag) {
            return false;
        }
        let parts: Vec<&str> = tag.split('/').collect();
        for i in 1..parts.len() {
            let ancestor = parts[..i].join("/");
            self.expanded.insert(ancestor, true);
        }
        self.rebuild_rows();
        if let Some(i) = self.index_of_tag(tag) {
            self.cursor = i;
            return true;
        }
        false
    }

    /// Put the cursor on a smart view without announcing it.
    pub fn select_view(&mut self, view: View) {
        self.cursor = self.index_of_view(view);
    }

    pub fn highlighted(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    pub fn highlighted_tag(&self) -> String {
        self.highlighted()
            .and_then(Row::tag)
            .unwrap_or("")
            .to_string()
    }

    pub fn highlighted_view(&self) -> Option<View> {
        self.highlighted().and_then(Row::view)
    }

    /// Move the cursor by `delta` rows, never resting on a gap row: carry on
    /// in the direction of travel, or, at the edge, stay put.
    pub fn move_cursor(&mut self, delta: i32) -> bool {
        let mut i = self.cursor as i32 + delta;
        while i >= 0 && (i as usize) < self.rows.len() {
            if self.rows[i as usize].selectable() {
                self.cursor = i as usize;
                return true;
            }
            i += delta.signum();
        }
        false
    }

    /// Land on row `index` from a click; gap rows are ignored.
    pub fn click(&mut self, index: usize) -> bool {
        if self.rows.get(index).is_some_and(Row::selectable) {
            self.cursor = index;
            return true;
        }
        false
    }

    /// `f`: collapse or expand the highlighted tag's subtree; on a leaf, fold
    /// the parent and move the cursor to it. Returns whether anything changed.
    pub fn fold_at_cursor(&mut self) -> bool {
        let Some(Row::Tag {
            path,
            has_children,
            expanded,
            depth,
            ..
        }) = self.highlighted().cloned()
        else {
            return false;
        };
        if has_children {
            self.set_expanded(&path, !expanded);
            return true;
        }
        if depth > 0 {
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            self.set_expanded(&parent, false);
            if let Some(i) = self.index_of_tag(&parent) {
                self.cursor = i;
            }
            return true;
        }
        false
    }

    /// `F`: collapse every tag if any is open, else expand every one. Scoped by
    /// construction: inside a workspace the tree holds only that subtree.
    /// Returns true when the result is expanded.
    pub fn toggle_all_folds(&mut self) -> bool {
        let branches = self.branches();
        let expand = !branches.iter().any(|b| self.is_expanded(b));
        for branch in &branches {
            self.expanded.insert(branch.clone(), expand);
        }
        let cursor_tag = self.highlighted_tag();
        self.rebuild_rows();
        if !expand && !cursor_tag.is_empty() && cursor_tag.contains('/') {
            let top = cursor_tag.split('/').next().unwrap_or("").to_string();
            if let Some(i) = self.index_of_tag(&top) {
                self.cursor = i;
            }
        }
        expand
    }

    /// Keep the cursor inside a viewport of `height` rows.
    pub fn ensure_visible(&mut self, height: usize) {
        let height = height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
        let max_scroll = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// One row as a line filling `width` cells: label on the left, count (and
    /// hotkey for views) flush against the right edge.
    pub fn render_row(
        &self,
        index: usize,
        width: usize,
        cursor_style: Option<ratatui::style::Style>,
    ) -> Line<'static> {
        let row = &self.rows[index];
        let base = cursor_style.unwrap_or_default();
        let dim = if cursor_style.is_some() {
            base
        } else {
            theme::dim()
        };
        let (label, count, tail) = match row {
            Row::View(view) => (
                format!("{}{}", self.icons.for_view(view.value()), view.label()),
                self.view_counts.get(view).copied().unwrap_or(0).to_string(),
                format!("  {}", view.hotkey()),
            ),
            Row::Gap => return Line::from(Span::styled(" ".repeat(width), base)),
            Row::Heading => {
                let mut spans = vec![Span::styled(TAGS_HEADING.to_string(), theme::bold())];
                spans.push(Span::raw(
                    " ".repeat(width.saturating_sub(TAGS_HEADING.len())),
                ));
                return Line::from(spans);
            }
            Row::Tag {
                path,
                name,
                depth,
                count,
                has_children,
                expanded,
            } => {
                let arrow = if *has_children {
                    if *expanded { "▾ " } else { "▸ " }
                } else {
                    "  "
                };
                let icon = if *depth == 0 {
                    self.icons.for_tag(path)
                } else {
                    String::new()
                };
                (
                    format!("{}{arrow}{icon}{name}", " ".repeat(depth * 2)),
                    count.to_string(),
                    String::new(),
                )
            }
        };
        let used = UnicodeWidthStr::width(label.as_str()) + count.len() + tail.len();
        let pad = width.saturating_sub(used).max(1);
        let mut spans = vec![Span::styled(label, base)];
        spans.push(Span::styled(" ".repeat(pad), base));
        spans.push(Span::styled(
            count,
            dim.add_modifier(if cursor_style.is_some() {
                Modifier::empty()
            } else {
                Modifier::DIM
            }),
        ));
        if !tail.is_empty() {
            spans.push(Span::styled(
                tail,
                dim.add_modifier(if cursor_style.is_some() {
                    Modifier::empty()
                } else {
                    Modifier::DIM
                }),
            ));
        }
        Line::from(spans)
    }
}
