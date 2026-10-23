//! A Craft document as Bear-style markdown, and a markdown edit as block writes.
//!
//! Craft stores a document as a page block whose children are blocks, each
//! with its own markdown (`## Heading`, `- [ ] task`, `  - nested`, a fenced
//! code block, a table). Bjorn shows the document as one note: `# Title`, a
//! blank line, then the blocks, a blank line between them except inside a run
//! of list items. Writing a note back does not replace the document: the new
//! text is split into the same kind of chunks and lined up against the blocks
//! it came from, so only the blocks that changed are written, and the rest
//! keep their ids, comments and history.
//!
//! Blocks Bjorn cannot show as markdown (sub-pages, collections, whiteboards,
//! files) are drawn as one placeholder line and never written: an edit that
//! drops the line leaves the block in place.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// How a block takes an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A text block: a changed one is rewritten in place (`PUT /blocks`).
    Text { list: bool },
    /// Code, tables and images: a changed one is replaced by a new block.
    Replace,
    /// Anything else: shown as a placeholder, never written.
    Opaque,
}

/// One top-level block, as the note shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub id: String,
    pub kind: Kind,
    pub text: String,
    /// A callout, shown as a quote; an edit that keeps the quote keeps the
    /// callout.
    pub callout: bool,
}

/// A document as a note: its title, the blocks under it, and the note text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub title: String,
    pub segments: Vec<Segment>,
    pub content: String,
}

/// Titles and ids of the documents a link can point at, for turning Craft's
/// `[Title](block://id)` into Bear's `[[Title]]` and back.
#[derive(Debug, Default, Clone)]
pub struct Links {
    titles: HashMap<String, String>,
    ids: HashMap<String, Option<String>>,
}

impl Links {
    pub fn new<'a>(docs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Links {
        let mut links = Links::default();
        for (id, title) in docs {
            links.titles.insert(id.to_string(), title.to_string());
            // Two documents with one title: neither is the link's target.
            links
                .ids
                .entry(title.to_lowercase())
                .and_modify(|slot| *slot = None)
                .or_insert_with(|| Some(id.to_string()));
        }
        links
    }

    fn title_of(&self, id: &str) -> Option<&str> {
        self.titles.get(id).map(String::as_str)
    }

    fn id_of(&self, title: &str) -> Option<&str> {
        self.ids.get(&title.to_lowercase())?.as_deref()
    }
}

static BLOCK_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\[\n]+)\]\(block://([A-Za-z0-9-]+)\)").unwrap());
static WIKI_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\[\n]+)\]\]").unwrap());
static LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])(?:\s|$)").unwrap());
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s").unwrap());

/// `[Title](block://id)` to `[[Title]]` when the link names the document it
/// points at; any other block link is left alone.
pub fn to_wiki(text: &str, links: &Links) -> String {
    BLOCK_LINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            match links.title_of(&caps[2]) {
                Some(title) if title == &caps[1] => format!("[[{title}]]"),
                _ => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// `[[Title]]` back to a Craft link, when exactly one document has the title.
pub fn from_wiki(text: &str, links: &Links) -> String {
    WIKI_LINK_RE
        .replace_all(text, |caps: &regex::Captures| match links.id_of(&caps[1]) {
            Some(id) => format!("[{}](block://{id})", &caps[1]),
            None => caps[0].to_string(),
        })
        .into_owned()
}

fn text_of(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One child block as a segment.
pub fn segment(block: &Value, links: &Links) -> Segment {
    let id = text_of(block.get("id"));
    let markdown = text_of(block.get("markdown"));
    let mut callout = false;
    let (kind, text) = match block.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" => {
            let mut text = to_wiki(&from_craft_inline(&markdown), links);
            if let Some(inner) = text
                .strip_prefix("<callout>")
                .and_then(|t| t.strip_suffix("</callout>"))
            {
                callout = true;
                text = quote(inner);
            }
            (
                Kind::Text {
                    list: block
                        .get("listStyle")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s != "none"),
                },
                text,
            )
        }
        "code" | "table" | "image" if !markdown.trim().is_empty() => (Kind::Replace, markdown),
        "page" => {
            let title = markdown.lines().next().unwrap_or("").trim();
            let title = if title.is_empty() { "Untitled" } else { title };
            (Kind::Opaque, format!("[⧉ {title}](block://{id})"))
        }
        other => (
            Kind::Opaque,
            format!("⧉ Craft {other} (open in Craft to see it)"),
        ),
    };
    Segment {
        id,
        kind,
        text,
        callout,
    }
}

static HIGHLIGHT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<highlight(?: [^>]*)?>([^<\n]*)</highlight>"#).unwrap());

/// Craft's inline tags as markdown: a highlight of any color is `==text==`.
fn from_craft_inline(markdown: &str) -> String {
    HIGHLIGHT_RE.replace_all(markdown, "==$1==").into_owned()
}

fn quote(text: &str) -> String {
    text.split('\n')
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A quoted chunk back to a callout's markdown, when it is still a quote.
fn unquote_callout(chunk: &str) -> Option<String> {
    let lines: Option<Vec<&str>> = chunk
        .split('\n')
        .map(|l| l.strip_prefix("> ").or_else(|| (l == ">").then_some("")))
        .collect();
    Some(format!("<callout>{}</callout>", lines?.join("\n")))
}

/// What goes between two blocks: a run of list items is one list.
fn separator(prev: Kind, next: Kind) -> &'static str {
    match (prev, next) {
        (Kind::Text { list: true }, Kind::Text { list: true }) => "\n",
        _ => "\n\n",
    }
}

fn join(segments: &[Segment]) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str(separator(segments[i - 1].kind, seg.kind));
        }
        out.push_str(&seg.text);
    }
    out
}

/// A document (`GET /blocks?id=<doc>`) as a note.
pub fn render(root: &Value, links: &Links) -> Rendered {
    let title = text_of(root.get("markdown")).trim().to_string();
    let segments: Vec<Segment> = root
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().map(|b| segment(b, links)).collect())
        .unwrap_or_default();
    let content = note_text(&title, &segments);
    Rendered {
        title,
        segments,
        content,
    }
}

fn note_text(title: &str, segments: &[Segment]) -> String {
    let body = join(segments);
    if body.is_empty() {
        format!("# {title}\n")
    } else {
        format!("# {title}\n\n{body}\n")
    }
}

/// Split note text into its title and body. The first line is the title, as
/// in Bear; a leading `# ` is the heading mark, not part of it.
pub fn split_title(content: &str) -> (String, String) {
    let content = content.trim_start_matches('\n');
    let (first, rest) = content.split_once('\n').unwrap_or((content, ""));
    let title = first.trim();
    let title = title.strip_prefix("# ").unwrap_or(title).trim();
    (title.to_string(), rest.trim_matches('\n').to_string())
}

/// Does this line start a list item (`- `, `* `, `1. `, `- [ ] `)?
pub fn is_list_item(line: &str) -> bool {
    LIST_RE.is_match(line)
}

fn is_fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    ["```", "~~~"].into_iter().find(|f| t.starts_with(f))
}

/// Does this line open a block of its own, whatever came before it?
fn starts_block(line: &str) -> bool {
    LIST_RE.is_match(line)
        || HEADING_RE.is_match(line)
        || is_fence(line).is_some()
        || line.trim_start().starts_with('|')
}

/// The body as chunks, one per block it would make. A run of lines that is
/// exactly an existing block's text, ending where a block may end, is kept as
/// that one chunk, so a block whose text the rules below would cut differently
/// still lines up with itself.
pub fn chunks(body: &str, known: &[&str]) -> Vec<String> {
    let lines: Vec<&str> = body.split('\n').collect();
    let mut by_first: HashMap<&str, Vec<Vec<&str>>> = HashMap::new();
    for text in known {
        let parts: Vec<&str> = text.split('\n').collect();
        by_first.entry(parts[0]).or_default().push(parts);
    }
    let boundary =
        |j: usize| j >= lines.len() || lines[j].trim().is_empty() || starts_block(lines[j]);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let matched = by_first
            .get(line)
            .into_iter()
            .flatten()
            .filter(|parts| {
                i + parts.len() <= lines.len()
                    && lines[i..i + parts.len()] == parts[..]
                    && boundary(i + parts.len())
            })
            .map(Vec::len)
            .max();
        let end = if let Some(len) = matched {
            i + len
        } else if let Some(fence) = is_fence(line) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with(fence) {
                j += 1;
            }
            (j + 1).min(lines.len())
        } else if line.trim_start().starts_with('|') {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with('|') {
                j += 1;
            }
            j
        } else if HEADING_RE.is_match(line) {
            i + 1
        } else {
            // A list item or a paragraph runs on to the next blank line or
            // the next line that opens a block.
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().is_empty() && !starts_block(lines[j]) {
                j += 1;
            }
            j
        };
        out.push(lines[i..end].join("\n"));
        i = end;
    }
    out
}

/// One write against the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Rewrite the title (the page block's own markdown).
    Title(String),
    /// Rewrite a block's markdown in place.
    Update {
        id: String,
        markdown: String,
    },
    /// Insert markdown after `after` (at the start of the page when `None`).
    Insert {
        after: Option<String>,
        markdown: String,
    },
    Delete(Vec<String>),
}

/// The writes that turn `old` into the note text `new`. An unchanged note
/// needs none. `Insert`s name the block they follow by id, so they must run
/// in order: a later insert may follow a block an earlier one made, which is
/// named by the marker `@<n>` (the last block of the `n`th op).
pub fn plan(old: &Rendered, new: &str, links: &Links) -> Vec<Op> {
    let mut ops = Vec::new();
    let (title, body) = split_title(new);
    if !title.is_empty() && title != old.title {
        ops.push(Op::Title(title));
    }
    let known: Vec<&str> = old.segments.iter().map(|s| s.text.as_str()).collect();
    let fresh = chunks(&body, &known);
    let pairs = lcs(&known, &fresh);

    let mut deletes: Vec<String> = Vec::new();
    // The block the next insert follows: an id, or the op that made it.
    let mut cursor: Option<String> = None;
    let mut pending: Vec<String> = Vec::new();
    let flush = |pending: &mut Vec<String>, cursor: &mut Option<String>, ops: &mut Vec<Op>| {
        if pending.is_empty() {
            return;
        }
        let markdown = from_wiki(&join_chunks(pending), links);
        pending.clear();
        ops.push(Op::Insert {
            after: cursor.clone(),
            markdown,
        });
        *cursor = Some(format!("@{}", ops.len() - 1));
    };

    let (mut oi, mut ni) = (0usize, 0usize);
    let anchors = pairs
        .into_iter()
        .chain(std::iter::once((known.len(), fresh.len())));
    for (oa, na) in anchors {
        // The gap: old blocks oi..oa against new chunks ni..na.
        let olds: Vec<&Segment> = old.segments[oi..oa]
            .iter()
            .filter(|s| s.kind != Kind::Opaque)
            .collect();
        let news = &fresh[ni..na];
        for (k, chunk) in news.iter().enumerate() {
            match olds.get(k) {
                Some(seg) if matches!(seg.kind, Kind::Text { .. }) => {
                    flush(&mut pending, &mut cursor, &mut ops);
                    let chunk = match seg.callout.then(|| unquote_callout(chunk)).flatten() {
                        Some(callout) => callout,
                        None => chunk.clone(),
                    };
                    ops.push(Op::Update {
                        id: seg.id.clone(),
                        markdown: from_wiki(&chunk, links),
                    });
                    cursor = Some(seg.id.clone());
                }
                Some(seg) => {
                    deletes.push(seg.id.clone());
                    pending.push(chunk.clone());
                }
                None => pending.push(chunk.clone()),
            }
        }
        for seg in olds.iter().skip(news.len()) {
            deletes.push(seg.id.clone());
        }
        flush(&mut pending, &mut cursor, &mut ops);
        if oa < known.len() {
            cursor = Some(old.segments[oa].id.clone());
        }
        oi = oa + 1;
        ni = na + 1;
    }
    if !deletes.is_empty() {
        ops.push(Op::Delete(deletes));
    }
    ops
}

/// New chunks as one markdown insert, laid out the way a note shows them.
fn join_chunks(chunks: &[String]) -> String {
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i > 0 {
            let both_list = LIST_RE.is_match(&chunks[i - 1]) && LIST_RE.is_match(chunk);
            out.push_str(if both_list { "\n" } else { "\n\n" });
        }
        out.push_str(chunk);
    }
    out
}

/// Index pairs of a longest common subsequence of equal strings.
fn lcs(a: &[&str], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    // Equal heads and tails need no table, which keeps a long note with one
    // edit cheap.
    let mut head = 0;
    while head < n && head < m && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < n - head && tail < m - head && a[n - 1 - tail] == b[m - 1 - tail] {
        tail += 1;
    }
    let (a_mid, b_mid) = (&a[head..n - tail], &b[head..m - tail]);
    let (p, q) = (a_mid.len(), b_mid.len());
    let mut table = vec![vec![0u32; q + 1]; p + 1];
    for i in (0..p).rev() {
        for j in (0..q).rev() {
            table[i][j] = if a_mid[i] == b_mid[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut out: Vec<(usize, usize)> = (0..head).map(|i| (i, i)).collect();
    let (mut i, mut j) = (0, 0);
    while i < p && j < q {
        if a_mid[i] == b_mid[j] {
            out.push((head + i, head + j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out.extend((0..tail).map(|k| (n - tail + k, m - tail + k)));
    out
}

/// Is this Bear todo syntax (`- [ ]`, `- [x]`)? Counted for the note list.
pub fn todo_counts(content: &str) -> (i64, i64) {
    static OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*+] \[ \]").unwrap());
    static DONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*+] \[[xX]\]").unwrap());
    let (mut open, mut done) = (0, 0);
    let mut fenced = false;
    for line in content.lines() {
        if is_fence(line).is_some() {
            fenced = !fenced;
        } else if fenced {
        } else if OPEN.is_match(line) {
            open += 1;
        } else if DONE.is_match(line) {
            done += 1;
        }
    }
    (open, done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn doc() -> Value {
        json!({
            "id": "doc", "type": "page", "markdown": "Plan",
            "content": [
                {"id": "p1", "type": "text", "markdown": "First line\nsecond line"},
                {"id": "h1", "type": "text", "textStyle": "h2", "markdown": "## Tasks"},
                {"id": "l1", "type": "text", "listStyle": "task", "markdown": "- [ ] one"},
                {"id": "l2", "type": "text", "listStyle": "bullet", "markdown": "  - nested\ncontinued"},
                {"id": "c1", "type": "code", "markdown": "```\nx = 1\n\ny = 2\n```"},
                {"id": "s1", "type": "page", "markdown": "Sub page"},
                {"id": "p2", "type": "text", "markdown": "See [Other](block://doc2) and [x](block://zzz)"},
            ]
        })
    }

    fn links() -> Links {
        Links::new([("doc", "Plan"), ("doc2", "Other")])
    }

    #[test]
    fn renders_as_a_bear_note() {
        let r = render(&doc(), &links());
        assert_eq!(
            r.content,
            "# Plan\n\nFirst line\nsecond line\n\n## Tasks\n\n- [ ] one\n  - nested\ncontinued\n\n```\nx = 1\n\ny = 2\n```\n\n[⧉ Sub page](block://s1)\n\nSee [[Other]] and [x](block://zzz)\n"
        );
    }

    #[test]
    fn an_unchanged_note_writes_nothing() {
        let r = render(&doc(), &links());
        assert_eq!(plan(&r, &r.content, &links()), vec![]);
        // Trailing blank lines and a missing final newline are not edits.
        assert_eq!(plan(&r, r.content.trim_end(), &links()), vec![]);
    }

    #[test]
    fn an_edited_block_is_rewritten_in_place() {
        let r = render(&doc(), &links());
        let new = r.content.replace("- [ ] one", "- [x] one");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![Op::Update {
                id: "l1".into(),
                markdown: "- [x] one".into()
            }]
        );
    }

    #[test]
    fn new_lines_are_inserted_after_the_block_before_them() {
        let r = render(&doc(), &links());
        let new = r
            .content
            .replace("- [ ] one\n", "- [ ] one\n- [ ] two\n- [ ] three\n");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![Op::Insert {
                after: Some("l1".into()),
                markdown: "- [ ] two\n- [ ] three".into()
            }]
        );
        let new = r.content.replace("# Plan\n\n", "# Plan\n\nIntro\n\n");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![Op::Insert {
                after: None,
                markdown: "Intro".into()
            }]
        );
    }

    #[test]
    fn a_removed_block_is_deleted_but_a_placeholder_never_is() {
        let r = render(&doc(), &links());
        let new = r
            .content
            .replace("## Tasks\n\n", "")
            .replace("[⧉ Sub page](block://s1)\n\n", "");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![Op::Delete(vec!["h1".into()])]
        );
    }

    #[test]
    fn a_changed_code_block_is_replaced() {
        let r = render(&doc(), &links());
        let new = r.content.replace("y = 2", "y = 3");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![
                Op::Insert {
                    after: Some("l2".into()),
                    markdown: "```\nx = 1\n\ny = 3\n```".into()
                },
                Op::Delete(vec!["c1".into()])
            ]
        );
    }

    #[test]
    fn title_and_wiki_links_round_trip() {
        let r = render(&doc(), &links());
        let new = r
            .content
            .replace("# Plan", "# Plan B")
            .replace("See [[Other]]", "See [[other]] and [[Plan]] and [[Nope]]");
        assert_eq!(
            plan(&r, &new, &links()),
            vec![
                Op::Title("Plan B".into()),
                Op::Update {
                    id: "p2".into(),
                    markdown: "See [other](block://doc2) and [Plan](block://doc) and [[Nope]] and [x](block://zzz)".into()
                }
            ]
        );
    }

    #[test]
    fn a_replaced_paragraph_followed_by_inserts_keeps_order() {
        let r = render(&doc(), &links());
        let new = r.content.replace(
            "First line\nsecond line",
            "Changed\n\nAdded para\n\n- added item",
        );
        assert_eq!(
            plan(&r, &new, &links()),
            vec![
                Op::Update {
                    id: "p1".into(),
                    markdown: "Changed".into()
                },
                Op::Insert {
                    after: Some("p1".into()),
                    markdown: "Added para\n\n- added item".into()
                }
            ]
        );
    }

    #[test]
    fn chunks_follow_the_blocks_a_note_would_make() {
        assert_eq!(
            chunks(
                "para\nmore\n- a\n  - b\ncont\n# H\n```\nx\n\ny\n```\n| a |\n| b |\nend",
                &[]
            ),
            vec![
                "para\nmore",
                "- a",
                "  - b\ncont",
                "# H",
                "```\nx\n\ny\n```",
                "| a |\n| b |",
                "end"
            ]
        );
        // A known block is kept whole even where the rules would cut it.
        assert_eq!(
            chunks("odd\n- inside\n\nnext", &["odd\n- inside"]),
            vec!["odd\n- inside", "next"]
        );
    }

    #[test]
    fn callouts_read_as_quotes_and_stay_callouts() {
        let root = json!({"id": "d", "markdown": "T", "content": [
            {"id": "c", "type": "text", "decorations": ["callout"],
             "markdown": "<callout>Watch <highlight color=\"red\">this</highlight>\nnext</callout>"}
        ]});
        let r = render(&root, &Links::default());
        assert_eq!(r.content, "# T\n\n> Watch ==this==\n> next\n");
        let new = r.content.replace("next", "then");
        assert_eq!(
            plan(&r, &new, &Links::default()),
            vec![Op::Update {
                id: "c".into(),
                markdown: "<callout>Watch ==this==\nthen</callout>".into()
            }]
        );
    }

    #[test]
    fn todos_are_counted_outside_code() {
        assert_eq!(
            todo_counts("- [ ] a\n  - [x] b\n```\n- [ ] c\n```\n"),
            (1, 1)
        );
    }

    #[test]
    fn the_first_line_is_the_title() {
        assert_eq!(split_title("# T\n\nbody\n"), ("T".into(), "body".into()));
        assert_eq!(split_title("Plain\nbody"), ("Plain".into(), "body".into()));
    }
}
