//! Bear wiki links: `[[Note title]]`, `[[Note title/Heading]]` and
//! `[[Note title|shown text]]`, and the backlink search built on them.
//!
//! Bear escapes punctuation that belongs to the title with a backslash
//! (`\/`, `\#` are in real libraries), so a `\` before any ASCII punctuation
//! is an escape, the first unescaped `/` starts the heading, and the first
//! unescaped `|` starts the text shown in place of the title.
//! `[[/Heading]]`, with no title, points at a heading in the same note.
//!
//! Some links carry a doubled escape, `[[Cloud Arch \\/ EA/Apr 19]]`, for the
//! note `Cloud Arch / EA`. Rather than guess Bear's rule for those, a link
//! has more than one reading (`WikiLink::readings`): as written, then with
//! each `\\` before punctuation taken as a single escape. Following a link
//! takes the first reading whose title is a note; only when none is does it
//! offer to create one, named by the first reading.
//!
//! A link that escapes no `/` may name a title with a slash in it:
//! `[[A/B testing]]` is the note `A/B testing` as readily as the heading
//! `B testing` in `A`. So such a link is first read whole, as a title, then
//! split. Once a link escapes a `/` (`\/` or `\\/`), its bare `/` is the
//! heading separator and there is no whole reading.
//!
//! A link runs from `[[` to the first `]]` after it on the same line; a `]`
//! right after that belongs to the title while a `[` in it is still open, so
//! `[[[Draft] Plan]]` and `[[Plan [v2]]]` name `[Draft] Plan` and `Plan [v2]`,
//! and `[see [[Plan]]](url)` keeps its own bracket. A title that itself
//! contains `]]` or `[[` cannot be linked. A code span that opens before a
//! `[[` hides it; a backtick inside a link is part of the title.
//!
//! The markdown parser never sees a link: `LinkTable::tokenize` swaps each
//! one on a line for an opaque token (two noncharacters around its index), so
//! backticks, `*`, `~`, `==`, `&`, `<` or `|` inside a title cannot turn into
//! markup. The renderer and `wiki_links` resolve tokens back through the
//! table; inside code they become the original text again.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::bear::Location;

/// Opens and closes a link token. Noncharacters: never in a note on purpose,
/// and `tokenize` replaces any that are.
const TOKEN_OPEN: char = '\u{FDD0}';
const TOKEN_CLOSE: char = '\u{FDD1}';

/// Backlink candidates read per search, and each phrase is searched in Notes
/// and in the Archive separately. The phrase is specific, so this is a
/// ceiling for pathological titles rather than a page size.
pub const BACKLINK_LIMIT: usize = 200;

/// A backlink phrase needs this many letters or digits after `[[`. Bear's
/// phrase match is unreliable for very short ones (`"[[A"` finds nothing
/// though links start with A, while `"[[1"` finds 15 notes and `"[["` none),
/// so with fewer the search is `@wikilinks`, which covers every note holding a
/// `[[` (224 of 224 in a real library), and the body check narrows that down.
const MIN_PHRASE_CHARS: usize = 3;

/// Title characters Bear is known to escape inside a link (`\/`, `\#`).
const BEAR_ESCAPES: [char; 2] = ['/', '#'];

static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\u{FDD0}([0-9]+)\u{FDD1}").unwrap());

/// One wiki link as written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WikiLink {
    /// The note's title, unescaped. Empty for `[[/Heading]]`.
    pub title: String,
    /// The heading after the first unescaped `/`, or empty.
    pub section: String,
    /// The text after `|`, or empty.
    pub alias: String,
    /// The text between the brackets, as written; `readings` starts here.
    /// Empty for a link built by hand, which then has only its own reading.
    pub raw: String,
}

impl WikiLink {
    /// Parse the text between `[[` and `]]`; None when it names nothing.
    pub fn parse(inner: &str) -> Option<WikiLink> {
        Self::parse_as(inner, inner, true)
    }

    /// Parse `text`, keeping `raw` as the link as written. With `split`
    /// false a bare `/` is part of the title, for the whole reading.
    fn parse_as(text: &str, raw: &str, split: bool) -> Option<WikiLink> {
        // 0 title, 1 section, 2 alias.
        let mut parts = [String::new(), String::new(), String::new()];
        let mut at = 0;
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' if chars.peek().is_some_and(char::is_ascii_punctuation) => {
                    parts[at].push(chars.next().unwrap_or(ch));
                }
                '/' if at == 0 && split => at = 1,
                '|' if at < 2 => at = 2,
                other => parts[at].push(other),
            }
        }
        let [title, section, alias] = parts.map(|p| p.trim().to_string());
        (!title.is_empty() || !section.is_empty()).then(|| WikiLink {
            title,
            section,
            alias,
            raw: raw.to_string(),
        })
    }

    /// The ways to read this link, most literal first: as written, then, when
    /// it holds a doubled escape (`\\/`), with each `\\` before punctuation
    /// taken as one escape. A link that escapes no `/` and names a heading is
    /// read whole, as a title, before it is split (`[[A/B testing]]`). A
    /// caller takes the first whose title resolves.
    pub fn readings(&self) -> Vec<WikiLink> {
        let mut out: Vec<WikiLink> = Vec::new();
        let whole = !self.raw.contains("\\/");
        for (text, reading) in self.spellings() {
            let mut add = |link: WikiLink| {
                if !out
                    .iter()
                    .any(|r| (&r.title, &r.section) == (&link.title, &link.section))
                {
                    out.push(link);
                }
            };
            if whole
                && !reading.title.is_empty()
                && !reading.section.is_empty()
                && let Some(entire) = Self::parse_as(&text, &self.raw, false)
            {
                add(entire);
            }
            add(reading);
        }
        out
    }

    /// The link as written and, when it holds a doubled escape, with each
    /// `\\` before punctuation collapsed to one: each with the text it was
    /// parsed from. A link built by hand (no `raw`) has only itself.
    fn spellings(&self) -> Vec<(String, WikiLink)> {
        let mut out = vec![(self.raw.clone(), self.clone())];
        if self.raw.contains("\\\\") {
            let mut collapsed = String::with_capacity(self.raw.len());
            let mut chars = self.raw.chars().peekable();
            while let Some(ch) = chars.next() {
                collapsed.push(ch);
                if ch == '\\' && chars.peek() == Some(&'\\') {
                    let mut ahead = chars.clone();
                    ahead.next();
                    if ahead
                        .peek()
                        .is_some_and(|c| c.is_ascii_punctuation() && *c != '\\')
                    {
                        chars.next();
                    }
                }
            }
            if let Some(other) = Self::parse_as(&collapsed, &self.raw, true)
                && (&other.title, &other.section) != (&self.title, &self.section)
            {
                out.push((collapsed, other));
            }
        }
        out
    }

    /// What the reader shows in place of the brackets. A link with more than
    /// one spelling (a doubled escape) is shown as written, since which one
    /// Bear meant is only known once a title resolves. The whole reading of
    /// `[[A/B testing]]` does not count: it is drawn `A › B testing`, as
    /// Bear's own heading-link syntax reads it.
    pub fn label(&self) -> String {
        if !self.alias.is_empty() {
            return self.alias.clone();
        }
        if self.spellings().len() > 1 {
            return self.raw.clone();
        }
        self.target_label()
    }

    /// The target in words: `Title`, `Title › Heading`, or `› Heading`.
    pub fn target_label(&self) -> String {
        match (self.title.is_empty(), self.section.is_empty()) {
            (_, true) => self.title.clone(),
            (true, false) => format!("› {}", self.section),
            (false, false) => format!("{} › {}", self.title, self.section),
        }
    }

    /// Does this link, in any reading, point at the note called `title`?
    pub fn points_to(&self, title: &str) -> bool {
        self.readings().iter().any(|r| same_title(&r.title, title))
    }
}

/// The key a title is looked up by: trimmed, ignoring case.
pub fn title_key(title: &str) -> String {
    title.trim().to_lowercase()
}

/// Titles compare as Bear's link lookup does: trimmed, ignoring case.
pub fn same_title(a: &str, b: &str) -> bool {
    title_key(a) == title_key(b)
}

/// The links on one line of markdown, as byte ranges with what they parse
/// to. Code spans on the line hide what is inside them.
///
/// This runs on every line of a note each time it is drawn, so it is one
/// pass over the line plus a few lookup tables, whatever the brackets and
/// backticks in it: a line of four thousand `[` and as many `]`, or a run of
/// unclosed `[[`, once took minutes. Each position asks its questions (where
/// is the next `]]`, how many `[` are open before it, is there a `[[` inside,
/// where does this code span close) of the tables instead of rescanning.
pub fn scan_line(line: &str) -> Vec<(usize, usize, WikiLink)> {
    if !line.contains("[[") {
        return Vec::new();
    }
    let bytes = line.as_bytes();
    let n = bytes.len();
    let pair_at = |i: usize, b: u8| i + 1 < n && bytes[i] == b && bytes[i + 1] == b;
    // `next_close[p]` / `next_open[p]`: the first `]]` / `[[` at or after p;
    // `closers[p]`: how many `]` in a row start at p.
    let mut next_close = vec![n; n + 1];
    let mut next_open = vec![n; n + 1];
    let mut closers = vec![0; n + 1];
    for p in (0..n).rev() {
        next_close[p] = if pair_at(p, b']') {
            p
        } else {
            next_close[p + 1]
        };
        next_open[p] = if pair_at(p, b'[') {
            p
        } else {
            next_open[p + 1]
        };
        closers[p] = if bytes[p] == b']' {
            closers[p + 1] + 1
        } else {
            0
        };
    }
    // `depth[p]`: `[` minus `]` in `line[..p]`, so a slice's balance is a
    // subtraction.
    let mut depth = vec![0i64; n + 1];
    for (p, b) in bytes.iter().enumerate() {
        depth[p + 1] = depth[p]
            + match b {
                b'[' => 1,
                b']' => -1,
                _ => 0,
            };
    }
    let run_at = |i: usize| bytes[i..].iter().take_while(|b| **b == b'`').count();
    // Every backtick run's start, by its length, in line order.
    let mut runs: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    let mut p = 0;
    while p < n {
        if bytes[p] == b'`' {
            let run = run_at(p);
            runs.entry(run).or_default().push(p);
            p += run;
        } else {
            p += 1;
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'`' {
            // A code span closes on the next run of exactly as many backticks.
            let run = run_at(i);
            let close = runs.get(&run).and_then(|starts| {
                let k = starts.partition_point(|s| *s < i + run);
                starts.get(k).map(|s| s + run)
            });
            i = close.unwrap_or(i + run);
            continue;
        }
        if pair_at(i, b'[') && next_close[i + 2] < n {
            let first = next_close[i + 2];
            // A `]` after the `]]` closes a `[` in the title, while any is
            // open: `[[Plan [v2]]]`, but `[see [[Plan]]](url)`. Each one
            // taken closes one more, so how many is a subtraction too.
            let open = (depth[first] - depth[i + 2]).max(0) as usize;
            let end = first + open.min(closers[first] - 2);
            let nested = next_open[i + 2] + 2 <= end;
            if !nested && let Some(link) = WikiLink::parse(&line[i + 2..end]) {
                out.push((i, end + 2, link));
                i = end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}
/// `line` with any token character already in it shown as U+FFFD, so the
/// only tokens the renderer meets are the ones `tokenize` made.
pub fn neutralize(line: &str) -> Cow<'_, str> {
    if line.contains([TOKEN_OPEN, TOKEN_CLOSE]) {
        Cow::Owned(line.replace([TOKEN_OPEN, TOKEN_CLOSE], "\u{FFFD}"))
    } else {
        Cow::Borrowed(line)
    }
}

/// A run of text: plain, or a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Link(WikiLink),
}

/// The links taken out of a note by `tokenize`: the text as written, and what
/// it parses to, by token index.
#[derive(Debug, Clone, Default)]
pub struct LinkTable {
    links: Vec<(String, WikiLink)>,
}

impl LinkTable {
    /// Swap every link on `line` for a token into this table. The token's
    /// own characters are only ever tokens: one already in the note is shown
    /// as the replacement character.
    pub fn tokenize<'a>(&mut self, line: &'a str) -> Cow<'a, str> {
        let line = neutralize(line);
        let found = scan_line(&line);
        if found.is_empty() {
            return line;
        }
        let mut out = String::with_capacity(line.len());
        let mut last = 0;
        for (start, end, link) in found {
            out.push_str(&line[last..start]);
            out.push(TOKEN_OPEN);
            out.push_str(&self.links.len().to_string());
            out.push(TOKEN_CLOSE);
            self.links.push((line[start..end].to_string(), link));
            last = end;
        }
        out.push_str(&line[last..]);
        Cow::Owned(out)
    }

    fn get(&self, token: &regex::Captures) -> Option<&(String, WikiLink)> {
        token[1]
            .parse::<usize>()
            .ok()
            .and_then(|i| self.links.get(i))
    }

    /// Split rendered text into plain pieces and links.
    pub fn split(&self, text: &str) -> Vec<Piece> {
        if !text.contains(TOKEN_OPEN) {
            return vec![Piece::Text(text.to_string())];
        }
        let mut out = Vec::new();
        let mut last = 0;
        for caps in TOKEN_RE.captures_iter(text) {
            let whole = caps.get(0).expect("match");
            let Some((_, link)) = self.get(&caps) else {
                continue;
            };
            if whole.start() > last {
                out.push(Piece::Text(text[last..whole.start()].to_string()));
            }
            out.push(Piece::Link(link.clone()));
            last = whole.end();
        }
        if last < text.len() {
            out.push(Piece::Text(text[last..].to_string()));
        }
        out
    }

    /// Tokens back to the text as written, for code and image captions.
    pub fn restore<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !text.contains(TOKEN_OPEN) {
            return Cow::Borrowed(text);
        }
        TOKEN_RE.replace_all(text, |caps: &regex::Captures| {
            self.get(caps)
                .map(|(raw, _)| raw.clone())
                .unwrap_or_else(|| caps[0].to_string())
        })
    }
}

/// The Bear searches that find candidate backlinks to `title`: the phrase
/// `[[Title`, with the title's `/` and `#` escaped the way Bear writes them
/// (`\/`, `\#`; an escaped phrase matches escaped text and an unescaped one
/// does not). Links with a doubled escape (`\\/`) exist too, so a title that
/// needs escaping gets a second phrase written that way, and a title with a
/// `/` a third with the slash bare, for `[[A/B testing]]` (see `readings`).
/// Bear matches a
/// phrase ignoring case and as a prefix, so `[[Title 2]]` and
/// `[[Title/Heading]]` come back too; `backlinks` sorts them out.
///
/// The phrase stops before a `"` (it would end the phrase), a `\` and a `|`
/// (how Bear escapes those in a link is unconfirmed); the prefix before them
/// is enough. When too little of the title is left, the search is
/// `@wikilinks`, every note with a wiki link.
pub fn backlink_queries(title: &str) -> Vec<String> {
    let title = title.trim();
    let usable = title.split(['"', '\\', '|']).next().unwrap_or("");
    if usable.chars().filter(|c| c.is_alphanumeric()).count() < MIN_PHRASE_CHARS {
        return vec!["@wikilinks".to_string()];
    }
    let escaped = |escape: &str| {
        let mut out = String::from("\"[[");
        for ch in usable.chars() {
            if BEAR_ESCAPES.contains(&ch) {
                out.push_str(escape);
            }
            out.push(ch);
        }
        out.push('"');
        out
    };
    let mut out = vec![escaped("\\")];
    if usable.contains(BEAR_ESCAPES) {
        out.push(escaped("\\\\"));
    }
    if usable.contains('/') {
        out.push(format!("\"[[{usable}\""));
    }
    out
}

/// A note that links here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub id: String,
    pub title: String,
    pub location: Location,
    /// The headings it links to, if it names any.
    pub sections: Vec<String>,
}

/// What a backlink search found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Backlinks {
    pub notes: Vec<Backlink>,
    /// The search hit `BACKLINK_LIMIT`, so notes past it went unchecked.
    pub capped: bool,
}

/// Keep the search rows (`id`, `title`, `location`, `content`) whose body
/// really links to `title` outside code, dropping `exclude_id` (the note
/// itself) and anything in the trash. `links_in` is the reader's own link
/// extraction, so what counts as a link here is what the reader draws as one.
/// `capped` says the search behind `rows` hit its limit.
pub fn backlinks(
    rows: &[Value],
    capped: bool,
    title: &str,
    exclude_id: &str,
    links_in: impl Fn(&str) -> Vec<WikiLink>,
) -> Backlinks {
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut notes = Vec::new();
    for row in rows {
        let id = text(row, "id");
        let location = Location::parse(&text(row, "location"));
        if id.is_empty() || id == exclude_id || location == Location::Trash {
            continue;
        }
        // The reading that names this title, for its heading.
        let hits: Vec<WikiLink> = links_in(&text(row, "content"))
            .into_iter()
            .filter_map(|l| {
                l.readings()
                    .into_iter()
                    .find(|r| same_title(&r.title, title))
            })
            .collect();
        if hits.is_empty() {
            continue;
        }
        let mut sections: Vec<String> = Vec::new();
        for link in hits {
            if !link.section.is_empty() && !sections.contains(&link.section) {
                sections.push(link.section);
            }
        }
        notes.push(Backlink {
            id,
            title: text(row, "title"),
            location,
            sections,
        });
    }
    Backlinks { notes, capped }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(title: &str, section: &str, alias: &str) -> WikiLink {
        WikiLink {
            title: title.into(),
            section: section.into(),
            alias: alias.into(),
            raw: String::new(),
        }
    }

    /// `WikiLink::parse`, without the text as written, to compare readings.
    fn parse(inner: &str) -> Option<WikiLink> {
        WikiLink::parse(inner).map(bare)
    }

    fn bare(link: WikiLink) -> WikiLink {
        WikiLink {
            raw: String::new(),
            ..link
        }
    }

    fn titles(line: &str) -> Vec<String> {
        scan_line(line)
            .into_iter()
            .map(|(_, _, l)| l.title)
            .collect()
    }

    #[test]
    fn parses_title_section_and_alias() {
        assert_eq!(parse("Garden Plan"), Some(link("Garden Plan", "", "")));
        assert_eq!(
            parse("Garden Plan/Next spring"),
            Some(link("Garden Plan", "Next spring", ""))
        );
        assert_eq!(
            parse("Reading Queue|the list"),
            Some(link("Reading Queue", "", "the list"))
        );
        assert_eq!(
            parse("Plan/Spring|see spring"),
            Some(link("Plan", "Spring", "see spring"))
        );
        assert_eq!(
            parse(r"New\/Modern DNS/ESXi - Web Server"),
            Some(link("New/Modern DNS", "ESXi - Web Server", ""))
        );
        assert_eq!(parse(r"A \| B|shown"), Some(link("A | B", "", "shown")));
        assert_eq!(parse("/Tasks"), Some(link("", "Tasks", "")));
        // Any ASCII punctuation can be escaped, as Bear does with `#`.
        assert_eq!(
            parse(r"Presentation Session \#1"),
            Some(link("Presentation Session #1", "", ""))
        );
        assert_eq!(parse(r"a\*b\/c"), Some(link("a*b/c", "", "")));
        assert_eq!(parse(r"a\b"), Some(link(r"a\b", "", "")));
        assert_eq!(parse("   "), None);
        assert_eq!(parse("|alias only"), None);
    }

    #[test]
    fn a_doubled_escape_has_a_second_reading() {
        let written = r"Cloud Arch \\/ EA/Apr 19, 2023 (Wednesday)";
        let link = WikiLink::parse(written).unwrap();
        let readings: Vec<WikiLink> = link.readings().into_iter().map(bare).collect();
        assert_eq!(
            readings,
            vec![
                super::WikiLink {
                    title: r"Cloud Arch \".into(),
                    section: "EA/Apr 19, 2023 (Wednesday)".into(),
                    ..Default::default()
                },
                link_of("Cloud Arch / EA", "Apr 19, 2023 (Wednesday)"),
            ]
        );
        assert!(link.points_to("cloud arch / ea"));
        assert_eq!(link.label(), written, "shown as written");
        assert_eq!(WikiLink::parse("Plain").unwrap().readings().len(), 1);
    }

    #[test]
    fn a_bare_slash_is_read_whole_before_it_is_split() {
        let ab = WikiLink::parse("A/B testing").unwrap();
        let readings: Vec<WikiLink> = ab.readings().into_iter().map(bare).collect();
        assert_eq!(
            readings,
            vec![link_of("A/B testing", ""), link_of("A", "B testing")]
        );
        assert!(ab.points_to("a/b testing"));
        assert!(ab.points_to("A"));
        assert_eq!(ab.label(), "A › B testing", "drawn as Bear's heading link");
        // The alias rides along on the whole reading.
        let aliased = WikiLink::parse("Plan/Spring|see spring").unwrap();
        assert_eq!(
            aliased.readings().into_iter().map(bare).collect::<Vec<_>>(),
            vec![
                link("Plan/Spring", "", "see spring"),
                link("Plan", "Spring", "see spring")
            ]
        );
        // Nothing to split, or no title before the slash: one reading.
        assert_eq!(WikiLink::parse("Plan").unwrap().readings().len(), 1);
        assert_eq!(WikiLink::parse("/Tasks").unwrap().readings().len(), 1);
    }

    #[test]
    fn a_link_that_escapes_a_slash_is_never_read_whole() {
        // It spells the title's own slash `\/`, so the bare one is a heading.
        let escaped = WikiLink::parse(r"New\/Modern DNS/ESXi").unwrap();
        assert_eq!(
            escaped.readings().into_iter().map(bare).collect::<Vec<_>>(),
            vec![link_of("New/Modern DNS", "ESXi")]
        );
        let doubled = WikiLink::parse(r"Cloud Arch \\/ EA/Apr 19").unwrap();
        assert!(
            doubled.readings().iter().all(|r| !r.section.is_empty()),
            "{:?}",
            doubled.readings()
        );
    }

    #[test]
    fn a_hand_built_link_has_only_its_own_reading() {
        let built = link_of("Sprint Planning", "Notes");
        assert_eq!(built.readings(), vec![built.clone()]);
    }

    fn link_of(title: &str, section: &str) -> WikiLink {
        link(title, section, "")
    }

    #[test]
    fn labels() {
        assert_eq!(link("A", "", "").label(), "A");
        assert_eq!(link("A", "B", "").label(), "A › B");
        assert_eq!(link("A", "B", "shown").label(), "shown");
        assert_eq!(link("", "B", "").label(), "› B");
    }

    #[test]
    fn scanning_finds_links_with_markup_in_their_titles() {
        assert_eq!(
            titles("[[September 6, 2021 - September `0, 2021 (Weekly)]]"),
            vec!["September 6, 2021 - September `0, 2021 (Weekly)"]
        );
        assert_eq!(
            titles("a [[*x* ~y~ ==z== & <b>]] b"),
            vec!["*x* ~y~ ==z== & <b>"]
        );
        assert_eq!(titles("[[[Draft] Plan]]"), vec!["[Draft] Plan"]);
        assert_eq!(titles("[[Plan [v2]]] and [[B]]"), vec!["Plan [v2]", "B"]);
        assert_eq!(titles("[[a [[b]]"), vec!["b"]);
        assert_eq!(titles("[see [[Plan]]](url)"), vec!["Plan"]);
        assert!(titles("`[[in code]]` and ``[[x]]``").is_empty());
        assert_eq!(titles("` unclosed [[yes]]"), vec!["yes"]);
        assert!(titles("[[ ]] [[]]").is_empty());
    }

    #[test]
    fn tokens_split_and_restore() {
        let mut table = LinkTable::default();
        let line = "see [[A\\/B/C|x]] and [[D]] \u{FDD0}";
        let tokenized = table.tokenize(line).into_owned();
        assert!(!tokenized.contains("[["));
        assert!(
            tokenized.ends_with('\u{FFFD}'),
            "a stray noncharacter is neutralized"
        );
        let pieces: Vec<Piece> = table
            .split(&tokenized)
            .into_iter()
            .map(|p| match p {
                Piece::Link(l) => Piece::Link(bare(l)),
                text => text,
            })
            .collect();
        assert_eq!(pieces[0], Piece::Text("see ".into()));
        assert_eq!(pieces[1], Piece::Link(link("A/B", "C", "x")));
        assert_eq!(pieces[3], Piece::Link(link("D", "", "")));
        assert_eq!(
            table.restore(&tokenized),
            "see [[A\\/B/C|x]] and [[D]] \u{FFFD}"
        );
    }

    #[test]
    fn the_backlink_phrase_is_a_safe_prefix_or_falls_back() {
        assert_eq!(backlink_queries("Garden Plan"), vec!["\"[[Garden Plan\""]);
        assert_eq!(
            backlink_queries("New/Modern"),
            vec![
                r#""[[New\/Modern""#,
                r#""[[New\\/Modern""#,
                r#""[[New/Modern""#
            ]
        );
        assert_eq!(
            backlink_queries("Presentation Session #1"),
            vec![
                r#""[[Presentation Session \#1""#,
                r#""[[Presentation Session \\#1""#
            ]
        );
        assert_eq!(backlink_queries("The \"best\" one"), vec!["\"[[The \""]);
        assert_eq!(backlink_queries("Path C:\\"), vec!["\"[[Path C:\""]);
        assert_eq!(backlink_queries("Left | Right"), vec!["\"[[Left \""]);
        assert_eq!(backlink_queries("\"Quoted\" title"), vec!["@wikilinks"]);
        assert_eq!(backlink_queries("Q3"), vec!["@wikilinks"]);
    }

    #[test]
    fn titles_compare_without_case() {
        assert!(same_title("Garden Plan", "garden plan "));
        assert!(!same_title("Garden Plan", "Garden Plan 2027"));
    }

    /// The scanner as it was before it became linear: plain and obviously
    /// right, and minutes long on a line of brackets. `scan_line` must agree
    /// with it on every line.
    fn reference_scan(line: &str) -> Vec<(usize, usize, WikiLink)> {
        let bytes = line.as_bytes();
        let run_at = |i: usize| bytes[i..].iter().take_while(|b| **b == b'`').count();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                let run = run_at(i);
                let mut j = i + run;
                let mut close = None;
                while j < bytes.len() {
                    if bytes[j] == b'`' {
                        let other = run_at(j);
                        if other == run {
                            close = Some(j + other);
                            break;
                        }
                        j += other;
                    } else {
                        j += 1;
                    }
                }
                i = close.unwrap_or(i + run);
                continue;
            }
            if bytes[i] == b'[' && bytes.get(i + 1) == Some(&b'[') {
                if let Some(rel) = line[i + 2..].find("]]") {
                    let mut end = i + 2 + rel;
                    let open =
                        |inner: &str| inner.matches('[').count() > inner.matches(']').count();
                    while bytes.get(end + 2) == Some(&b']') && open(&line[i + 2..end]) {
                        end += 1;
                    }
                    let inner = &line[i + 2..end];
                    if !inner.contains("[[")
                        && let Some(link) = WikiLink::parse(inner)
                    {
                        out.push((i, end + 2, link));
                        i = end + 2;
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            i += 1;
        }
        out
    }

    #[test]
    fn scanning_agrees_with_the_reference_on_tricky_brackets() {
        for line in [
            "[[[Draft] Plan]]",
            "[[Plan [v2]]]]]",
            "[[a [[b]]",
            "[[[[x]]]]",
            "[[x]]]]]",
            "[[[[[[",
            "]]]]]] [[a]]",
            "[[a]] [[b] ]] [[ [c ]]]",
            "[[x`y]] `[[z]]` ``[[w]]` [[v]]``",
            "` [[a]] `` [[b]] ` [[c]]",
            r"[[A\/B/C|x]] [[C \\/ D]] [[|only]] [[/Heading]]",
            "[[é [ü]]] [[ñ]]",
            "[[]] [[ ]] [[]]]",
        ] {
            assert_eq!(scan_line(line), reference_scan(line), "{line:?}");
        }
    }

    #[test]
    fn scanning_agrees_with_the_reference_on_random_lines() {
        // A fixed-seed xorshift, so a failure names a line that reproduces.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = ['[', '[', ']', ']', '`', 'a', ' ', '|', '/', '\\', 'é'];
        let mut linked = 0;
        for _ in 0..20_000 {
            let len = (next() % 41) as usize;
            let line: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            let found = scan_line(&line);
            assert_eq!(found, reference_scan(&line), "{line:?}");
            linked += usize::from(!found.is_empty());
        }
        // Enough of the lines hold a link for the comparison to mean something.
        assert!(linked > 500, "{linked}");
    }

    #[test]
    fn a_line_of_brackets_scans_in_linear_time() {
        for line in [
            format!("{}{}", "[".repeat(4000), "]".repeat(4000)),
            "[[".repeat(64_000),
            format!("{}{}", "[[".repeat(32_000), "]".repeat(64_000)),
            "[[a` ".repeat(20_000),
            (1..400).map(|k| "`".repeat(k) + "[[x").collect::<String>(),
        ] {
            let started = std::time::Instant::now();
            scan_line(&line);
            let took = started.elapsed();
            eprintln!("{} bytes: {took:?}", line.len());
            // Milliseconds in a debug build; the bound leaves room for a slow CI.
            assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        }
    }
}
