//! Dated sections: the template, the day-tag time axis, and the rules for
//! where a new section goes and how an existing one is recognized.
//!
//! A topic note holds a title, a preamble and then one section per day, newest
//! first. Each section's heading is the date and the line under it carries a
//! day tag (`#log/2026/09/19` by default), so Bear's tag view is a time axis —
//! but it lists *notes*, not sections. Everything here is pure text work over a
//! note body; the writes live in `app`.
//!
//! **What counts as a dated section** (`find_sections`): a markdown heading
//! outside fenced code whose
//!
//! 1. heading text parses as the configured `heading_format`, **or**
//! 2. first non-blank line below it is a tag line carrying a tag that parses
//!    as the configured `day_tag`.
//!
//! Either alone is enough, so a note whose headings are written differently is
//! still found through its tags, and a note without day tags is still found
//! through its headings. Fenced code is skipped, which means an **unclosed**
//! fence hides every section below it — the same blind spot the todo parser
//! has, and the note reads as one long code block in Bear too.
//!
//! **What a write normalizes**: the note is rejoined with its own dominant line
//! ending (a CRLF note stays CRLF) and keeps its final newline, or its lack of
//! one. Blank lines directly around the insert point are collapsed to exactly
//! one; nothing else in the note is touched.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

use crate::bear::{display_tag, normalize_tag};
use crate::render::{HEADING_RE, is_fence, is_tag_line, strip_inline_markup, tags_in_line};
use crate::util::strip_control;

/// The day tag that carries the time axis, as a strftime pattern.
pub const DEFAULT_DAY_TAG: &str = "log/%Y/%m/%d";
/// The date heading, as a strftime pattern: `September 19, 2026 (Saturday)`.
pub const DEFAULT_HEADING_FORMAT: &str = "%B %-d, %Y (%A)";
/// The starting-point template, kept in the repo so it can be copied into a
/// config file and edited. `include_str!` means the two can never drift.
pub const DEFAULT_TEMPLATE: &str = include_str!("../config/templates/section.md");
/// How much of a section's body the day view shows.
pub const SNIPPET_LIMIT: usize = 90;
/// Shorter than this, a date heading is too common a string to search for.
pub const MIN_PHRASE: usize = 6;

static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([a-zA-Z]+)(?::([^}]*))?\s*\}\}").unwrap());
static BULLET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s*)?").unwrap());
static RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(-{3,}|\*{3,}|_{3,})\s*$").unwrap());

/// Where `s` puts a new section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InsertPosition {
    /// Straight after the title and its tag line.
    Top,
    /// At the end, above any tags Bear keeps at the bottom of the note.
    Bottom,
    /// Above the first dated section, so the newest stays on top; at the end
    /// when the note has none yet. The default.
    #[default]
    BeforeFirstDatedSection,
}

impl InsertPosition {
    /// `top`, `bottom`, `before-first-dated-section` (underscores accepted).
    /// Anything else is the default, the way the rest of the config is lenient.
    pub fn parse(text: &str) -> InsertPosition {
        match text.trim().to_lowercase().replace('_', "-").as_str() {
            "top" => InsertPosition::Top,
            "bottom" | "end" => InsertPosition::Bottom,
            _ => InsertPosition::BeforeFirstDatedSection,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            InsertPosition::Top => "top",
            InsertPosition::Bottom => "bottom",
            InsertPosition::BeforeFirstDatedSection => "before-first-dated-section",
        }
    }
}

/// The `[sections]` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionsConfig {
    pub template: String,
    pub day_tag: String,
    pub heading_format: String,
    pub insert: InsertPosition,
}

impl Default for SectionsConfig {
    fn default() -> Self {
        SectionsConfig {
            template: DEFAULT_TEMPLATE.to_string(),
            day_tag: DEFAULT_DAY_TAG.to_string(),
            heading_format: DEFAULT_HEADING_FORMAT.to_string(),
            insert: InsertPosition::default(),
        }
    }
}

/// `date.format(fmt)` without the panic an invalid pattern would cause.
fn format_date(date: NaiveDate, fmt: &str) -> Option<String> {
    let when = date.and_time(NaiveTime::MIN);
    format_time(when, fmt)
}

/// `when.format(fmt)`, or None for a pattern this cannot render.
///
/// Two ways a pattern fails, and both have to be caught: a malformed one
/// (`Item::Error`), and one that parses but has nothing to render from a naive
/// time — `%Z` and `%z` want a time zone. The second only fails when the value
/// is written, and `to_string()` turns that failure into a panic, so the text
/// is written into a String by hand instead.
fn format_time(when: NaiveDateTime, fmt: &str) -> Option<String> {
    use std::fmt::Write as _;

    let items: Vec<_> = chrono::format::StrftimeItems::new(fmt).collect();
    if items
        .iter()
        .any(|item| matches!(item, chrono::format::Item::Error))
    {
        return None;
    }
    let mut out = String::new();
    write!(out, "{}", when.format_with_items(items.into_iter())).ok()?;
    Some(out)
}

impl SectionsConfig {
    /// The day tag for a date, bare (no `#`). Empty when the pattern is bad.
    pub fn day_tag_for(&self, date: NaiveDate) -> String {
        normalize_tag(&format_date(date, &self.day_tag).unwrap_or_default())
    }

    /// The day tag as Bear writes it: `#log/2026/09/19`.
    pub fn day_tag_display(&self, date: NaiveDate) -> String {
        let tag = self.day_tag_for(date);
        if tag.is_empty() {
            String::new()
        } else {
            display_tag(&tag)
        }
    }

    /// The heading text (no `#` markers) for a date.
    pub fn heading_text(&self, date: NaiveDate) -> String {
        format_date(date, &self.heading_format).unwrap_or_default()
    }

    /// The date heading as a search phrase, or "" when it is not worth
    /// searching for. Bear matches a quoted phrase literally, so a heading
    /// that renders a `"` cannot be quoted, and a very short one (`%d` alone:
    /// "19") would match half the library; in both cases the day screen falls
    /// back to the tag search on its own.
    pub fn heading_phrase(&self, date: NaiveDate) -> String {
        let text = self.heading_text(date).trim().to_string();
        if text.chars().count() < MIN_PHRASE || text.contains('"') {
            return String::new();
        }
        text
    }

    /// Does this tag name the given day? Used to match a tag line under a
    /// heading, and to recognize a note's day tags.
    pub fn date_of_tag(&self, tag: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&normalize_tag(tag), &self.day_tag).ok()
    }

    /// Does this heading text parse as a date under `heading_format`?
    pub fn date_of_heading(&self, text: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(text.trim(), &self.heading_format).ok()
    }

    /// The template with its placeholders filled in:
    /// `{{date}}`, `{{date:FMT}}`, `{{time}}`, `{{time:FMT}}`, `{{tag}}`,
    /// `{{title}}`. An unknown name is left as written, so a typo shows up in
    /// the note instead of silently vanishing.
    pub fn render(&self, when: NaiveDateTime, note_title: &str) -> String {
        let date = when.date();
        let body = PLACEHOLDER_RE.replace_all(&self.template, |caps: &regex::Captures| {
            let name = caps[1].to_lowercase();
            let arg = caps.get(2).map(|m| m.as_str());
            match (name.as_str(), arg) {
                ("date", None) => self.heading_text(date),
                ("date", Some(fmt)) => format_date(date, fmt).unwrap_or_default(),
                ("time", None) => format_time(when, "%H:%M").unwrap_or_default(),
                ("time", Some(fmt)) => format_time(when, fmt).unwrap_or_default(),
                ("tag", _) => self.day_tag_display(date),
                ("title", _) => note_title.to_string(),
                _ => caps[0].to_string(),
            }
        });
        body.trim_end_matches('\n').to_string()
    }
}

impl SectionsConfig {
    /// What is wrong with the patterns, in one sentence, or None when they
    /// work. Checked once at start-up: a `day_tag` with no date in it (or a
    /// heading format that renders nothing) leaves the day screen empty
    /// forever with nothing to say why.
    pub fn problem(&self, today: NaiveDate) -> Option<String> {
        let tag = self.day_tag_for(today);
        if tag.is_empty() {
            return Some(format!(
                "[sections] day_tag = {:?} is not a date pattern Bjorn can render; the day screen (T) will stay empty.",
                self.day_tag
            ));
        }
        if self.date_of_tag(&tag) != Some(today) {
            return Some(format!(
                "[sections] day_tag = {:?} does not carry a whole date ({tag} does not read back as a day); the day screen (T) will stay empty.",
                self.day_tag
            ));
        }
        if self.heading_text(today).trim().is_empty() {
            return Some(format!(
                "[sections] heading_format = {:?} renders nothing; s would write a section with no heading.",
                self.heading_format
            ));
        }
        None
    }
}

/// One dated section found in a note body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub date: NaiveDate,
    /// The heading line as written, `#` markers and all: a bearcli section address.
    pub heading: String,
    /// Index of the heading line within the body.
    pub line: usize,
    /// The first line or two of the section's body, flattened.
    pub snippet: String,
}

impl Section {
    /// The heading without its `#` markers, for `app open --header`.
    pub fn header(&self) -> String {
        self.heading.trim_start_matches('#').trim().to_string()
    }

    fn level(&self) -> usize {
        self.heading.len() - self.heading.trim_start_matches('#').len()
    }
}

/// Every dated section in a note body, in document order. See the module note
/// for what makes a heading a dated section.
pub fn find_sections(config: &SectionsConfig, content: &str) -> Vec<Section> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<Section> = Vec::new();
    let mut in_fence = false;
    for (index, raw) in lines.iter().enumerate() {
        if is_fence(raw) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !HEADING_RE.is_match(raw) {
            continue;
        }
        let text = raw.trim().trim_start_matches('#').trim();
        let Some(date) = section_date(config, text, &lines[index + 1..]) else {
            continue;
        };
        out.push(Section {
            date,
            heading: raw.trim_end().to_string(),
            line: index,
            snippet: snippet(&lines[index + 1..]),
        });
    }
    out
}

/// The heading's own date, else the day tag on the line under it.
fn section_date(config: &SectionsConfig, text: &str, rest: &[&str]) -> Option<NaiveDate> {
    if let Some(date) = config.date_of_heading(text) {
        return Some(date);
    }
    for line in rest {
        if line.trim().is_empty() {
            continue;
        }
        if !is_tag_line(line) {
            return None;
        }
        return tags_in_line(line)
            .iter()
            .find_map(|tag| config.date_of_tag(tag));
    }
    None
}

/// The first two lines of a section's body worth showing, markers stripped.
fn snippet(rest: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for line in rest {
        if HEADING_RE.is_match(line) {
            break;
        }
        if line.trim().is_empty() || is_tag_line(line) || RULE_RE.is_match(line) {
            continue;
        }
        let text = strip_inline_markup(&BULLET_RE.replace(line, ""));
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        parts.push(text);
        if parts.len() == 2 {
            break;
        }
    }
    let joined = parts.join(" · ");
    if joined.chars().count() > SNIPPET_LIMIT {
        format!(
            "{}…",
            joined.chars().take(SNIPPET_LIMIT).collect::<String>()
        )
    } else {
        joined
    }
}

/// What `insert_section` did, or refused to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Insertion {
    /// The new body, and the heading line of the section that was added.
    Added { content: String, heading: String },
    /// That day already has a section; nothing was written. Carries its heading.
    Exists { heading: String },
    /// The template does not produce a section this module would find again,
    /// so writing it would add another one every time. Nothing was written.
    Unrecognized { reason: String },
}

/// Insert a section for `when`'s date into `content`, unless one is already
/// there. The insert point follows `config.insert`; see `InsertPosition`.
///
/// The result is checked before it is handed back: the note must now hold
/// exactly one section for that date, read by the same `find_sections` the day
/// screen uses. A template that does not produce one Bjorn can find again
/// would otherwise add a section every single time `s` is pressed.
pub fn insert_section(
    config: &SectionsConfig,
    content: &str,
    when: NaiveDateTime,
    note_title: &str,
) -> Insertion {
    let date = when.date();
    let sections = find_sections(config, content);
    if let Some(existing) = sections.iter().find(|s| s.date == date) {
        return Insertion::Exists {
            heading: existing.heading.clone(),
        };
    }
    let block = config.render(when, note_title);
    let lines: Vec<&str> = content.lines().collect();
    let at = insert_index(config, &lines, &sections);
    let (eol, trailing) = line_style(content);
    let grown = splice(&lines, at, &block, eol, trailing);
    let written: Vec<Section> = find_sections(config, &grown)
        .into_iter()
        .filter(|s| s.date == date)
        .collect();
    match written.len() {
        1 => Insertion::Added {
            content: grown,
            heading: written[0].heading.clone(),
        },
        // Nothing came back. Either the template never makes a section this
        // could find, or it does and this note's markup swallowed it — an
        // unclosed code fence hides everything below it. Say which.
        0 if block_reads_back(config, &block, date) => Insertion::Unrecognized {
            reason: "this note's own markup hides the new section — an unclosed ``` fence \
                     swallows everything below it, in Bear as well as here"
                .to_string(),
        },
        0 => Insertion::Unrecognized {
            reason: format!(
                "the [sections] template does not produce a dated section: it needs a heading that \
                 reads as heading_format ({:?}), or the day tag on the line straight under the heading",
                config.heading_format
            ),
        },
        n => Insertion::Unrecognized {
            reason: format!(
                "the [sections] template produces {n} sections for one day; it should produce one"
            ),
        },
    }
}

/// Does the rendered block, read on its own, hold exactly one section for the
/// day? If it does and the grown note holds none, the note is what hid it.
fn block_reads_back(config: &SectionsConfig, block: &str, date: NaiveDate) -> bool {
    find_sections(config, block)
        .iter()
        .filter(|s| s.date == date)
        .count()
        == 1
}

/// Where the new section's first line goes, as a line index.
fn insert_index(config: &SectionsConfig, lines: &[&str], sections: &[Section]) -> usize {
    let at = match config.insert {
        InsertPosition::Top => {
            let mut i = usize::from(lines.first().is_some_and(|l| l.starts_with("# ")));
            while i < lines.len() && (lines[i].trim().is_empty() || is_tag_line(lines[i])) {
                i += 1;
            }
            i
        }
        InsertPosition::Bottom => bottom_index(lines),
        InsertPosition::BeforeFirstDatedSection => match sections.first() {
            Some(section) => section.line,
            None => bottom_index(lines),
        },
    };
    // Bear takes a note's title from its first heading. A section written at
    // the very top of a note that has anything in it would make the date the
    // title and silently rename the note, so the first line always stays first.
    if at == 0 && !lines.is_empty() { 1 } else { at }
}

/// The end of the note, above the blank lines and the tag line Bear keeps at
/// the bottom.
///
/// Only a tag line that stands on its own — separated from the body by a blank
/// line, or the whole note — is the note's tags. A tag line pressed straight
/// against content belongs to the section above it (a template may well end
/// with `{{tag}}`), and climbing over that one would drop the new section
/// inside the old one.
fn bottom_index(lines: &[&str]) -> usize {
    let mut i = lines.len();
    while i > 0 && lines[i - 1].trim().is_empty() {
        i -= 1;
    }
    let mut j = i;
    while j > 0 && is_tag_line(lines[j - 1]) {
        j -= 1;
    }
    if j < i && (j == 0 || lines[j - 1].trim().is_empty()) {
        i = j;
        while i > 0 && lines[i - 1].trim().is_empty() {
            i -= 1;
        }
    }
    i
}

/// The line ending a note is written with, and whether it ends with one.
/// Splicing rejoins with these, so a CRLF note stays CRLF and a note without a
/// final newline does not gain one.
fn line_style(content: &str) -> (&'static str, bool) {
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    let eol = if crlf > 0 && crlf * 2 >= lf {
        "\r\n"
    } else {
        "\n"
    };
    (eol, content.ends_with('\n'))
}

/// Put `block` in at line `at`, with exactly one blank line either side of it
/// (and none at the very start or end of the note), rejoined with `eol`.
fn splice(lines: &[&str], at: usize, block: &str, eol: &str, trailing: bool) -> String {
    let at = at.min(lines.len());
    let mut out: Vec<String> = lines[..at].iter().map(|l| l.to_string()).collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if !out.is_empty() {
        out.push(String::new());
    }
    out.extend(block.lines().map(|l| l.to_string()));
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let rest: Vec<&str> = lines[at..]
        .iter()
        .skip_while(|l| l.trim().is_empty())
        .copied()
        .collect();
    if !rest.is_empty() {
        out.push(String::new());
        out.extend(rest.iter().map(|l| l.to_string()));
    }
    let mut text = out.join(eol);
    if trailing {
        text.push_str(eol);
    }
    text
}

/// Append `line` to the end of the section for `when`'s date, creating that
/// section first when the note has none. The line lands above the section's
/// closing rule and blank lines, so the shape of the template survives.
///
/// Nothing calls this yet: `bjorn capture -- "text"` will, once the two
/// branches carrying a `capture` subcommand agree on one (see docs/ROADMAP.md).
/// The day screen and `s` use the pieces above.
pub fn append_to_section(
    config: &SectionsConfig,
    content: &str,
    when: NaiveDateTime,
    note_title: &str,
    line: &str,
) -> String {
    let body = match insert_section(config, content, when, note_title) {
        Insertion::Added { content, .. } => content,
        Insertion::Exists { .. } | Insertion::Unrecognized { .. } => content.to_string(),
    };
    let sections = find_sections(config, &body);
    let Some(section) = sections.iter().find(|s| s.date == when.date()) else {
        return body;
    };
    let lines: Vec<&str> = body.lines().collect();
    let level = section.level();
    let mut end = lines.len();
    for (i, raw) in lines.iter().enumerate().skip(section.line + 1) {
        if HEADING_RE.is_match(raw) && heading_level(raw) <= level {
            end = i;
            break;
        }
    }
    // Above the closing rule and any blank lines under it.
    while end > section.line + 1
        && (lines[end - 1].trim().is_empty() || RULE_RE.is_match(lines[end - 1]))
    {
        end -= 1;
    }
    let mut out: Vec<String> = lines[..end].iter().map(|l| l.to_string()).collect();
    out.push(line.to_string());
    out.extend(lines[end..].iter().map(|l| l.to_string()));
    let (eol, trailing) = line_style(&body);
    let mut text = out.join(eol);
    if trailing {
        text.push_str(eol);
    }
    text
}

fn heading_level(line: &str) -> usize {
    let trimmed = line.trim_start();
    trimmed.len() - trimmed.trim_start_matches('#').len()
}

/// One section on the day screen: which note it is in, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayRow {
    pub note_id: String,
    pub note_title: String,
    pub heading: String,
    pub snippet: String,
    /// The note sits in Bear's archive. A finished topic note is archived with
    /// its whole history, so the day it was written still counts; the screen
    /// just says where it is.
    pub archived: bool,
}

impl DayRow {
    /// The heading without its `#` markers, for `app open --header`.
    pub fn header(&self) -> String {
        self.heading.trim_start_matches('#').trim().to_string()
    }
}

/// What a day read of Bear produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayScan {
    pub rows: Vec<DayRow>,
    /// Notes whose content bearcli could not read (locked or encrypted).
    pub locked: usize,
    /// Notes with a section on the day, in list order.
    pub notes: usize,
}

fn is_yes(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "yes" | "true" | "1"),
        Some(Value::Bool(b)) => *b,
        _ => false,
    }
}

/// Turn the rows of a day search into the sections written that day.
///
/// The search finds *notes*; each note's body is parsed here and only the
/// sections for that date are kept — which is the whole point of the screen,
/// since a topic note carries a tag (and a section) per day it was written in.
/// A note in the trash is dropped; an archived one is kept and marked.
/// Everything that reaches a row is stripped of control characters: it is
/// drawn straight into the terminal.
pub fn scan_day_rows(config: &SectionsConfig, rows: &[Value], date: NaiveDate) -> DayScan {
    let mut scan = DayScan::default();
    for row in rows {
        let location = row
            .get("location")
            .and_then(Value::as_str)
            .unwrap_or("notes")
            .trim()
            .to_lowercase();
        if location == "trash" {
            continue;
        }
        let content = row.get("content");
        if is_yes(row.get("locked")) || content.is_none_or(Value::is_null) {
            scan.locked += 1;
            continue;
        }
        let title = strip_control(
            row.get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim(),
        );
        let note_id = row.get("id").and_then(Value::as_str).unwrap_or("");
        let body = content.and_then(Value::as_str).unwrap_or("");
        let found: Vec<DayRow> = find_sections(config, body)
            .into_iter()
            .filter(|section| section.date == date)
            .map(|section| DayRow {
                note_id: note_id.to_string(),
                note_title: if title.is_empty() {
                    "Untitled".to_string()
                } else {
                    title.clone()
                },
                heading: strip_control(&section.heading),
                snippet: strip_control(&section.snippet),
                archived: location == "archive",
            })
            .collect();
        if !found.is_empty() {
            scan.notes += 1;
        }
        scan.rows.extend(found);
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn noon(y: i32, m: u32, d: u32) -> NaiveDateTime {
        date(y, m, d).and_hms_opt(12, 30, 0).unwrap()
    }

    const NOTE: &str = "# Topic\n#topic\n\nA preamble line.\n\n\
## September 12, 2026 (Saturday)\n#log/2026/09/12\n* People: two of us\n* Topic: the rollout\n\n---\n\n\
## September 10, 2026 (Thursday)\n#log/2026/09/10\n* People:\n* Topic: kickoff\n\n---\n";

    #[test]
    fn the_default_template_fills_in() {
        let config = SectionsConfig::default();
        let rendered = config.render(noon(2026, 9, 19), "Topic");
        assert_eq!(
            rendered,
            "## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n\n---"
        );
    }

    #[test]
    fn placeholders_cover_dates_times_tag_and_title() {
        let config = SectionsConfig {
            template: "## {{date:%Y-%m-%d}} {{time}} {{tag}} {{title}} {{nonsense}}".into(),
            ..SectionsConfig::default()
        };
        assert_eq!(
            config.render(noon(2026, 9, 19), "Field Notes"),
            "## 2026-09-19 12:30 #log/2026/09/19 Field Notes {{nonsense}}"
        );
    }

    #[test]
    fn a_dated_section_is_found_by_heading_or_by_tag() {
        let config = SectionsConfig::default();
        let sections = find_sections(&config, NOTE);
        assert_eq!(
            sections.iter().map(|s| s.date).collect::<Vec<_>>(),
            vec![date(2026, 9, 12), date(2026, 9, 10)]
        );
        assert_eq!(sections[0].heading, "## September 12, 2026 (Saturday)");
        assert_eq!(
            sections[0].snippet,
            "People: two of us · Topic: the rollout"
        );

        // The tag alone is enough when the heading is written another way.
        let tagged = "# T\n\n## Monday standup\n#log/2026/09/14\nnotes here\n";
        assert_eq!(
            find_sections(&config, tagged)
                .iter()
                .map(|s| s.date)
                .collect::<Vec<_>>(),
            vec![date(2026, 9, 14)]
        );
        // The heading alone is enough when there is no day tag.
        let untagged = "# T\n\n## September 14, 2026 (Monday)\nnotes here\n";
        assert_eq!(find_sections(&config, untagged).len(), 1);
        // A plain heading is not a dated section, and neither is one inside a fence.
        let plain = "# T\n\n## Tasks\n- [ ] one\n\n```\n## September 14, 2026 (Monday)\n#log/2026/09/14\n```\n";
        assert!(find_sections(&config, plain).is_empty());
    }

    #[test]
    fn another_day_tag_pattern_works() {
        let config = SectionsConfig {
            day_tag: "journal/%Y-%m-%d".into(),
            ..SectionsConfig::default()
        };
        let body = "# Trail\n#trail\n\n## Day eleven\n#journal/2026-09-18\nRain all morning.\n";
        let sections = find_sections(&config, body);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].date, date(2026, 9, 18));
        assert_eq!(
            config.day_tag_display(date(2026, 9, 18)),
            "#journal/2026-09-18"
        );
        // The default pattern does not see it.
        assert!(find_sections(&SectionsConfig::default(), body).is_empty());
    }

    #[test]
    fn insert_goes_above_the_first_dated_section() {
        let config = SectionsConfig::default();
        let Insertion::Added { content, heading } =
            insert_section(&config, NOTE, noon(2026, 9, 19), "Topic")
        else {
            panic!("expected an insertion");
        };
        assert_eq!(heading, "## September 19, 2026 (Saturday)");
        assert_eq!(
            content,
            "# Topic\n#topic\n\nA preamble line.\n\n\
## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n\n---\n\n\
## September 12, 2026 (Saturday)\n#log/2026/09/12\n* People: two of us\n* Topic: the rollout\n\n---\n\n\
## September 10, 2026 (Thursday)\n#log/2026/09/10\n* People:\n* Topic: kickoff\n\n---\n"
        );
        assert_eq!(find_sections(&config, &content).len(), 3);
    }

    #[test]
    fn a_note_with_no_dated_section_gets_one_at_the_end() {
        let config = SectionsConfig::default();
        let body = "# Topic\n#topic\n\nJust a preamble.\n";
        let Insertion::Added { content, .. } =
            insert_section(&config, body, noon(2026, 9, 19), "Topic")
        else {
            panic!("expected an insertion");
        };
        assert_eq!(
            content,
            "# Topic\n#topic\n\nJust a preamble.\n\n## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n\n---\n"
        );
    }

    #[test]
    fn bottom_stays_above_the_notes_own_tags_and_top_below_them() {
        let body = "# Topic\n#topic\n\nPreamble.\n\n## September 10, 2026 (Thursday)\n#log/2026/09/10\nold\n\n#topic/archive\n";
        let bottom = SectionsConfig {
            insert: InsertPosition::Bottom,
            ..SectionsConfig::default()
        };
        let Insertion::Added { content, .. } =
            insert_section(&bottom, body, noon(2026, 9, 19), "Topic")
        else {
            panic!("expected an insertion");
        };
        assert!(content.ends_with("---\n\n#topic/archive\n"), "{content}");

        let top = SectionsConfig {
            insert: InsertPosition::Top,
            ..SectionsConfig::default()
        };
        let Insertion::Added { content, .. } =
            insert_section(&top, body, noon(2026, 9, 19), "Topic")
        else {
            panic!("expected an insertion");
        };
        assert!(
            content.starts_with("# Topic\n#topic\n\n## September 19, 2026 (Saturday)\n"),
            "{content}"
        );
    }

    #[test]
    fn a_second_section_for_the_same_day_is_refused() {
        let config = SectionsConfig::default();
        assert_eq!(
            insert_section(&config, NOTE, noon(2026, 9, 12), "Topic"),
            Insertion::Exists {
                heading: "## September 12, 2026 (Saturday)".into()
            }
        );
    }

    #[test]
    fn append_lands_inside_todays_section_above_the_rule() {
        let config = SectionsConfig::default();
        let grown = append_to_section(&config, NOTE, noon(2026, 9, 12), "Topic", "* Note: added");
        assert!(
            grown.contains("* Topic: the rollout\n* Note: added\n\n---"),
            "{grown}"
        );
        // A day with no section yet gets one, then the line.
        let fresh = append_to_section(&config, NOTE, noon(2026, 9, 19), "Topic", "* Note: fresh");
        assert!(
            fresh.contains("## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n* Note: fresh\n\n---"),
            "{fresh}"
        );
    }

    #[test]
    fn day_rows_keep_only_that_days_sections() {
        let config = SectionsConfig::default();
        let rows = vec![
            json!({"id": "A", "title": "Topic", "locked": "no", "content": NOTE}),
            json!({"id": "B", "title": "Locked", "locked": "yes", "content": null}),
            json!({"id": "C", "title": "Other", "locked": "no", "content": "# Other\n\n## September 12, 2026 (Saturday)\n#log/2026/09/12\nshort\n"}),
        ];
        let scan = scan_day_rows(&config, &rows, date(2026, 9, 12));
        assert_eq!(
            scan.rows
                .iter()
                .map(|r| (r.note_title.as_str(), r.snippet.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Topic", "People: two of us · Topic: the rollout"),
                ("Other", "short")
            ]
        );
        assert_eq!((scan.locked, scan.notes), (1, 2));
        assert_eq!(scan.rows[0].header(), "September 12, 2026 (Saturday)");
        assert!(
            scan_day_rows(&config, &rows, date(2026, 9, 11))
                .rows
                .is_empty()
        );
    }

    #[test]
    fn a_pattern_that_cannot_be_rendered_is_empty_not_a_panic() {
        // %Z and %z parse but have no value in a naive time: formatting them
        // fails when it is written, which `to_string()` would panic on.
        for fmt in ["%B %-d, %Y %Z", "%Y-%m-%d %z", "%"] {
            let config = SectionsConfig {
                heading_format: fmt.into(),
                ..SectionsConfig::default()
            };
            assert_eq!(config.heading_text(date(2026, 9, 19)), "", "{fmt}");
            assert_eq!(config.heading_phrase(date(2026, 9, 19)), "", "{fmt}");
            assert!(config.problem(date(2026, 9, 19)).is_some(), "{fmt}");
        }
        let config = SectionsConfig {
            template: "## {{date:%Z}}|{{time:%z}}|end".into(),
            ..SectionsConfig::default()
        };
        assert_eq!(config.render(noon(2026, 9, 19), "T"), "## ||end");
        let config = SectionsConfig {
            day_tag: "log/%Z".into(),
            ..SectionsConfig::default()
        };
        assert_eq!(config.day_tag_for(date(2026, 9, 19)), "");
        assert!(config.day_tag_display(date(2026, 9, 19)).is_empty());
    }

    #[test]
    fn the_patterns_are_checked_for_sense() {
        let today = date(2026, 9, 19);
        assert!(SectionsConfig::default().problem(today).is_none());
        // A tag with no date in it finds nothing, for ever.
        let config = SectionsConfig {
            day_tag: "log/daily".into(),
            ..SectionsConfig::default()
        };
        let problem = config.problem(today).unwrap();
        assert!(problem.contains("day_tag"), "{problem}");
        // Nor does one that carries only part of a date.
        let config = SectionsConfig {
            day_tag: "log/%Y/%m".into(),
            ..SectionsConfig::default()
        };
        assert!(config.problem(today).is_some());
    }

    #[test]
    fn a_heading_too_short_or_unquotable_is_not_searched_for() {
        let day = date(2026, 9, 19);
        assert_eq!(
            SectionsConfig::default().heading_phrase(day),
            "September 19, 2026 (Saturday)"
        );
        for fmt in ["%d", "%b", "\"%Y-%m-%d\""] {
            let config = SectionsConfig {
                heading_format: fmt.into(),
                ..SectionsConfig::default()
            };
            assert_eq!(config.heading_phrase(day), "", "{fmt}");
        }
    }

    #[test]
    fn a_template_that_reads_back_as_nothing_is_refused() {
        // No heading at all: `find_sections` would never see it, so every `s`
        // would add another one.
        let config = SectionsConfig {
            template: "{{tag}}\n* People:".into(),
            ..SectionsConfig::default()
        };
        let Insertion::Unrecognized { reason } =
            insert_section(&config, "# T\n#t\n\nbody\n", noon(2026, 9, 19), "T")
        else {
            panic!("a template with no heading must be refused");
        };
        assert!(reason.contains("dated section"), "{reason}");

        // A heading that does not read as a date, with the tag pushed away
        // from it by a line of text.
        let config = SectionsConfig {
            template: "## Log\nSomething first.\n{{tag}}".into(),
            ..SectionsConfig::default()
        };
        assert!(matches!(
            insert_section(&config, "# T\n#t\n\nbody\n", noon(2026, 9, 19), "T"),
            Insertion::Unrecognized { .. }
        ));

        // The tag directly under a heading of its own wording is fine.
        let config = SectionsConfig {
            template: "## Log\n{{tag}}\n* People:".into(),
            ..SectionsConfig::default()
        };
        assert!(matches!(
            insert_section(&config, "# T\n#t\n\nbody\n", noon(2026, 9, 19), "T"),
            Insertion::Added { .. }
        ));
    }

    #[test]
    fn a_note_that_hides_the_new_section_is_blamed_instead_of_the_template() {
        let config = SectionsConfig::default();
        // An unclosed fence swallows everything below it — here and in Bear.
        let body = "# T\n#t\n\n```\nnot closed\n";
        let Insertion::Unrecognized { reason } =
            insert_section(&config, body, noon(2026, 9, 19), "T")
        else {
            panic!("a section written into an open fence must be refused");
        };
        assert!(reason.contains("this note's own markup"), "{reason}");
        assert!(!reason.contains("template"), "{reason}");
        // The same template is fine in a note that closes its fence.
        assert!(matches!(
            insert_section(
                &config,
                "# T\n#t\n\n```\nclosed\n```\n",
                noon(2026, 9, 19),
                "T"
            ),
            Insertion::Added { .. }
        ));
    }

    #[test]
    fn a_section_never_becomes_the_notes_title() {
        // Bear takes the title from the first heading. A note that opens with
        // body text (no H1) must keep that line first, or writing a section
        // would rename the note to the date.
        for insert in [
            InsertPosition::Top,
            InsertPosition::Bottom,
            InsertPosition::BeforeFirstDatedSection,
        ] {
            let config = SectionsConfig {
                insert,
                ..SectionsConfig::default()
            };
            let body = "## September 12, 2026 (Saturday)\n#log/2026/09/12\nold\n";
            let Insertion::Added { content, .. } =
                insert_section(&config, body, noon(2026, 9, 19), "T")
            else {
                panic!("expected an insertion for {insert:?}");
            };
            assert!(
                content.starts_with("## September 12, 2026 (Saturday)"),
                "{insert:?}: {content}"
            );
        }
        // An empty note has no first line to protect.
        let Insertion::Added { content, .. } =
            insert_section(&SectionsConfig::default(), "", noon(2026, 9, 19), "T")
        else {
            panic!("expected an insertion");
        };
        assert!(
            content.starts_with("## September 19, 2026 (Saturday)"),
            "{content}"
        );
    }

    #[test]
    fn bottom_keeps_a_templates_own_trailing_tag_line_inside_its_section() {
        // A template that ends with the tag leaves a tag line pressed against
        // the section's body; that is the section's, not the note's.
        let config = SectionsConfig {
            insert: InsertPosition::Bottom,
            template: "## {{date}}\n* People:\n{{tag}}".into(),
            ..SectionsConfig::default()
        };
        let body = "# T\n#t\n\n## September 12, 2026 (Saturday)\n* People: Ada\n#log/2026/09/12\n";
        let Insertion::Added { content, .. } =
            insert_section(&config, body, noon(2026, 9, 19), "T")
        else {
            panic!("expected an insertion");
        };
        assert!(
            content.contains("* People: Ada\n#log/2026/09/12\n\n## September 19, 2026 (Saturday)"),
            "the new section goes after the old one, not into it:\n{content}"
        );
        // The note's own bottom tags, on their own after a blank line, are
        // still stepped over.
        let body =
            "# T\n\n## September 12, 2026 (Saturday)\n* People: Ada\n#log/2026/09/12\n\n#t\n";
        let Insertion::Added { content, .. } =
            insert_section(&config, body, noon(2026, 9, 19), "T")
        else {
            panic!("expected an insertion");
        };
        assert!(content.ends_with("#log/2026/09/19\n\n#t\n"), "{content}");
    }

    #[test]
    fn a_notes_line_endings_survive_a_write() {
        let config = SectionsConfig::default();
        let crlf = "# T\r\n#t\r\n\r\nbody\r\n";
        let Insertion::Added { content, .. } =
            insert_section(&config, crlf, noon(2026, 9, 19), "T")
        else {
            panic!("expected an insertion");
        };
        assert!(!content.contains("\n\n"), "no bare LF is left: {content:?}");
        assert!(content.ends_with("---\r\n"), "{content:?}");
        assert_eq!(find_sections(&config, &content).len(), 1);

        // A note that ends without a newline does not gain one.
        let bare = "# T\n#t\n\nbody";
        let Insertion::Added { content, .. } =
            insert_section(&config, bare, noon(2026, 9, 19), "T")
        else {
            panic!("expected an insertion");
        };
        assert!(content.ends_with("---"), "{content:?}");
    }

    #[test]
    fn day_rows_skip_the_trash_mark_the_archive_and_drop_control_characters() {
        let config = SectionsConfig::default();
        let body =
            "# X\n\n## September 12, 2026 (Saturday)\n#log/2026/09/12\n* Topic: a\u{1b}[31mb\n";
        let rows = vec![
            json!({"id": "A", "title": "Kept", "locked": "no", "location": "notes", "content": body}),
            json!({"id": "B", "title": "Filed", "locked": "no", "location": "archive", "content": body}),
            json!({"id": "C", "title": "Gone", "locked": "no", "location": "trash", "content": body}),
            json!({"id": "D", "title": "Noisy\u{7}", "locked": "no", "content": body}),
        ];
        let scan = scan_day_rows(&config, &rows, date(2026, 9, 12));
        assert_eq!(
            scan.rows
                .iter()
                .map(|r| (r.note_title.as_str(), r.archived))
                .collect::<Vec<_>>(),
            vec![("Kept", false), ("Filed", true), ("Noisy", false)],
            "the trash is not history, the archive is"
        );
        assert!(
            scan.rows.iter().all(|r| r.snippet == "Topic: a[31mb"),
            "{:?}",
            scan.rows[0].snippet
        );
        assert_eq!(scan.notes, 3);
    }

    #[test]
    fn insert_position_parses_leniently() {
        assert_eq!(InsertPosition::parse("Top"), InsertPosition::Top);
        assert_eq!(InsertPosition::parse("bottom"), InsertPosition::Bottom);
        assert_eq!(
            InsertPosition::parse("before_first_dated_section"),
            InsertPosition::BeforeFirstDatedSection
        );
        assert_eq!(
            InsertPosition::parse("nonsense"),
            InsertPosition::BeforeFirstDatedSection
        );
    }
}
