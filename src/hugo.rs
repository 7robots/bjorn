//! Publishing a note as a Hugo post.
//!
//! `plan` turns a note into the file it would write (front matter, body,
//! images) and only reads the disk; the app shows the plan in the confirm
//! dialog and `write` carries it out. Nothing here makes a network call:
//! images are copied to a staging folder for the user's own upload step (or
//! beside the post in a page bundle), and the links point at where they will
//! be served.
//!
//! Front matter is read and validated with a YAML parser, but written as
//! text edits, the way the config file is: a post's comments, key order and
//! hand-added keys (`cover`, `description`, anything else) survive a
//! republish. What Bjorn is about to write is parsed again before it is
//! written, so it never emits YAML that has not been checked. A post whose
//! front matter it cannot read (TOML, JSON, broken YAML, not UTF-8) is
//! refused, never rewritten.
//!
//! Which note wrote which file is recorded in a ledger outside the site
//! (beside the config file), so nothing that identifies the note lands in the
//! post: no Bear id, no local path.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, NaiveDateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha1::Digest;
use unicode_normalization::UnicodeNormalization;
use yaml_rust2::{Yaml, YamlLoader};

use crate::bear::{Note, normalize_tag};
use crate::export::ExportError;
use crate::render::{is_tag_line, percent_decode};
use crate::util::{expand_tilde, home_dir};

pub const DEFAULT_SECTION: &str = "posts";
pub const DEFAULT_PATH: &str = "{year}/{month}/{slug}.md";
/// Hugo's permalink for the section, used only to show the URL in the dialog.
pub const DEFAULT_PERMALINK: &str = "/:year/:month/:day/:slug/";
/// Staged images wait here for the user's upload step. Outside any site on
/// purpose: a site repository may be public, and the staging copies are not
/// meant to be committed.
pub const DEFAULT_MEDIA_DIR: &str = "~/Downloads/bjorn-media";
/// The ledger's file name, beside the config file.
pub const LEDGER_NAME: &str = "hugo-published.json";
pub const MORE: &str = "<!--more-->";
/// Attachments that are published; anything else (svg, html, pdf...) is not.
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "avif"];
/// The note's own front matter keys that reach the post. Everything else
/// (`layout`, `url`, `aliases`, `markup`, `outputs`, `build`, `type`...) could
/// change how or where the site renders, and is dropped.
pub const NOTE_KEYS: [&str; 9] = [
    "title",
    "slug",
    "date",
    "description",
    "cover",
    "tags",
    "draft",
    "summary",
    "showtoc",
];

/// The keys Bjorn writes, in the order a new post gets them (the site
/// archetype's order).
const MANAGED: [&str; 7] = [
    "title",
    "slug",
    "date",
    "lastmod",
    "draft",
    "tags",
    "description",
];
/// The longest title the dialog shows.
const DIALOG_TITLE_CHARS: usize = 70;

static KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_][A-Za-z0-9_.-]*)[ \t]*:(?:[ \t]|$)").unwrap());
/// A markdown link or image: `[text](target "title")`, `![alt](<a b.png>)`.
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(!?)\[([^\]\n]*)\]\([ \t]*(<[^>\n]*>|[^)\s]+)((?:[ \t]+(?:"[^"\n]*"|'[^'\n]*'|\([^)\n]*\)))?[ \t]*)\)"#,
    )
    .unwrap()
});
static WIKI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\[\]\n]+)\]\]").unwrap());
static BEAR_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[([^\]\n]*)\]\([ \t]*<?bear://[^)\n]*\)").unwrap());
static BEAR_BARE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<bear://[^>\s]*>|bear://\S+").unwrap());

/// `[hugo]` in the config file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HugoConfig {
    /// The Hugo site's root (the folder with `content/`). Empty: not set up.
    pub site: String,
    /// The section under `content/`.
    pub section: String,
    /// Where a new post goes under the section: `{year}` `{month}` `{day}`
    /// `{slug}`. Ending in `/index.md` makes a page bundle, with the images
    /// beside the post.
    pub path: String,
    /// The site's permalink pattern for the section, for the dialog's URL:
    /// `:year` `:month` `:day` `:slug` `:section`.
    pub permalink: String,
    /// Only tags under this one are published, without it: `blog/rust` is
    /// `rust`. Empty publishes no tags at all.
    pub tag_prefix: String,
    /// A note carrying this tag is written with `draft: false`; every other
    /// note is a draft. Empty: always a draft.
    pub publish_tag: String,
    /// Where uploaded images are served from; links become
    /// `{media_url}/{year}/{month}/{file}`. Empty and not a bundle: a note
    /// with images is refused rather than published with broken links.
    pub media_url: String,
    /// Where images wait for upload, as `{media_dir}/{year}/{month}/{file}`.
    pub media_dir: PathBuf,
    /// Put `<!--more-->` after the lead paragraph when the note has none.
    pub summary_divider: bool,
}

impl Default for HugoConfig {
    fn default() -> Self {
        Self {
            site: String::new(),
            section: DEFAULT_SECTION.into(),
            path: DEFAULT_PATH.into(),
            permalink: DEFAULT_PERMALINK.into(),
            tag_prefix: String::new(),
            publish_tag: String::new(),
            media_url: String::new(),
            media_dir: expand_tilde(DEFAULT_MEDIA_DIR),
            summary_divider: true,
        }
    }
}

impl HugoConfig {
    pub fn configured(&self) -> bool {
        !self.site.trim().is_empty()
    }

    pub fn site_dir(&self) -> PathBuf {
        expand_tilde(self.site.trim())
    }

    fn bundle(&self) -> bool {
        let path = self.path.trim();
        path.ends_with("/index.md") || path == "index.md"
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, ExportError> {
    Err(ExportError(message.into()))
}

/// `new-post.sh`'s slug: ASCII lower case, every run of anything but `a-z`
/// and `0-9` a single hyphen, none at either end. A non-ASCII letter is a
/// separator, as it is in that script under the C locale.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in text.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(ch);
        } else {
            gap = true;
        }
    }
    out
}

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Bidi controls, zero-width and other invisible format characters, which
/// could make the dialog say something other than what is written.
fn is_invisible(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            0xAD | 0x61C | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F
            | 0xFEFF | 0xFFF9..=0xFFFB | 0xE0000..=0xE007F)
}

/// Text from the note made safe for the dialog: invisible characters gone,
/// at most `max` characters.
pub fn clean_display(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| !is_invisible(*c)).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > max {
        let cut: String = clean.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    } else {
        clean
    }
}

// -- front matter ----------------------------------------------------------------

/// One top-level key of YAML front matter as text: the comment and blank
/// lines above it, then its own lines (the key line and anything indented
/// under it). A block without a key holds trailing comments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    key: Option<String>,
    lead: Vec<String>,
    lines: Vec<String>,
}

impl Block {
    fn new(key: &str, lines: Vec<String>) -> Block {
        Block {
            key: Some(key.to_string()),
            lead: Vec::new(),
            lines,
        }
    }
}

/// The front matter as text blocks, and as the parser read it.
#[derive(Debug, Clone)]
struct Front {
    blocks: Vec<Block>,
    map: yaml_rust2::yaml::Hash,
}

impl Front {
    /// A key's value; keys compare case-insensitively, as Hugo's do.
    fn get(&self, key: &str) -> Option<&Yaml> {
        get(&self.map, key)
    }
}

fn get<'a>(map: &'a yaml_rust2::yaml::Hash, key: &str) -> Option<&'a Yaml> {
    map.iter()
        .find(|(k, _)| k.as_str().is_some_and(|k| k.eq_ignore_ascii_case(key)))
        .map(|(_, v)| v)
}

/// The lines between the `---` fences, as blocks. `None` when something in
/// there is not shaped like top-level YAML keys.
fn parse_blocks(lines: &[&str]) -> Option<Vec<Block>> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for line in lines {
        if let Some(caps) = KEY_RE.captures(line) {
            blocks.push(Block {
                key: Some(caps[1].to_string()),
                lead: std::mem::take(&mut pending),
                lines: vec![line.to_string()],
            });
        } else if line.trim().is_empty() {
            pending.push(line.to_string());
        } else if line.starts_with([' ', '\t', '-']) {
            let current = blocks.last_mut()?;
            current.lines.append(&mut pending);
            current.lines.push(line.to_string());
        } else if line.starts_with('#') {
            pending.push(line.to_string());
        } else {
            return None;
        }
    }
    if !pending.is_empty() {
        blocks.push(Block {
            key: None,
            lead: pending,
            lines: Vec::new(),
        });
    }
    Some(blocks)
}

/// Parse YAML front matter text into a mapping with text keys, none of which
/// differ only in case.
fn parse_mapping(inner: &str) -> Result<yaml_rust2::yaml::Hash, String> {
    let docs = YamlLoader::load_from_str(inner).map_err(|e| e.to_string())?;
    let map = match docs.into_iter().next() {
        None | Some(Yaml::Null) => yaml_rust2::yaml::Hash::new(),
        Some(Yaml::Hash(map)) => map,
        Some(_) => return Err("it is not a list of keys".into()),
    };
    let mut seen = HashSet::new();
    for key in map.keys() {
        let Some(key) = key.as_str() else {
            return Err(format!("the key {key:?} is not text"));
        };
        if !seen.insert(key.to_lowercase()) {
            return Err(format!("“{key}” is there twice (keys ignore case)"));
        }
    }
    Ok(map)
}

/// Split `text` into its YAML front matter and the rest. `Ok(None)` when it
/// has none; an error when it has front matter Bjorn cannot read safely.
fn read_front(text: &str) -> Result<Option<(Front, String)>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.first().map(|l| l.trim_end()).unwrap_or("");
    if first == "+++" {
        return Err("it has TOML front matter (+++); Bjorn only edits YAML".into());
    }
    if first.starts_with('{') {
        return Err("it has JSON front matter; Bjorn only edits YAML".into());
    }
    if first != "---" {
        return Ok(None);
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|l| matches!(l.trim_end(), "---" | "..."))
        .map(|n| n + 1)
    else {
        return Err("its front matter has no closing ---".into());
    };
    let inner = &lines[1..end];
    let map = parse_mapping(&inner.join("\n"))?;
    let blocks = parse_blocks(inner)
        .ok_or("its front matter is laid out in a way Bjorn cannot edit as text")?;
    let text_keys: HashSet<String> = blocks
        .iter()
        .filter_map(|b| b.key.as_ref().map(|k| k.to_lowercase()))
        .collect();
    let yaml_keys: HashSet<String> = map
        .keys()
        .filter_map(|k| k.as_str().map(str::to_lowercase))
        .collect();
    if text_keys != yaml_keys {
        return Err("its front matter is laid out in a way Bjorn cannot edit as text".into());
    }
    Ok(Some((Front { blocks, map }, lines[end + 1..].join("\n"))))
}

fn find(blocks: &[Block], key: &str) -> Option<usize> {
    blocks.iter().position(|b| {
        b.key
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case(key))
    })
}

/// Put `block` in `blocks`: over the block with its key (keeping the
/// comments above that one), or after the nearest managed key that comes
/// before it, or at the end ahead of any trailing comments.
fn upsert(blocks: &mut Vec<Block>, block: Block) {
    let key = block.key.clone().unwrap_or_default();
    if let Some(i) = find(blocks, &key) {
        let lead = std::mem::take(&mut blocks[i].lead);
        blocks[i] = Block { lead, ..block };
        return;
    }
    let order = MANAGED.iter().position(|k| *k == key);
    let after = order.and_then(|o| {
        MANAGED[..o]
            .iter()
            .rev()
            .find_map(|k| find(blocks, k))
            .map(|i| i + 1)
    });
    let at = after.unwrap_or_else(|| {
        if blocks.last().is_some_and(|b| b.key.is_none()) {
            blocks.len() - 1
        } else {
            blocks.len()
        }
    });
    blocks.insert(at, block);
}

fn render_blocks(blocks: &[Block]) -> String {
    let mut out = String::from("---\n");
    for block in blocks {
        for line in block.lead.iter().chain(&block.lines) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("---\n");
    out
}

/// A YAML double-quoted scalar. JSON's string syntax is a subset of it.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// A tag as the site writes them: bare when it is plain lower-case words,
/// quoted otherwise (or when YAML would read it as something else).
fn tag_item(tag: &str) -> String {
    let plain = tag.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let special = matches!(
        YamlLoader::load_from_str(tag)
            .ok()
            .and_then(|d| d.into_iter().next()),
        Some(Yaml::Boolean(_) | Yaml::Null | Yaml::Integer(_) | Yaml::Real(_))
    );
    if plain && !special {
        tag.to_string()
    } else {
        quoted(tag)
    }
}

fn scalar_text(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(s) => Some(quoted(s)),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Real(r) => r.parse::<f64>().ok().map(|_| r.clone()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `key: value` lines written fresh from a parsed value: scalars, a list of
/// scalars, or one level of keys with scalar values. Anything else is not
/// written.
fn emit(key: &str, value: &Yaml) -> Option<Vec<String>> {
    match value {
        Yaml::Array(items) if items.is_empty() => Some(vec![format!("{key}: []")]),
        Yaml::Array(items) => {
            let mut lines = vec![format!("{key}:")];
            for item in items {
                lines.push(format!("  - {}", scalar_text(item)?));
            }
            Some(lines)
        }
        Yaml::Hash(map) => {
            let mut lines = vec![format!("{key}:")];
            for (k, v) in map {
                let k = k.as_str().filter(|k| KEY_RE.is_match(&format!("{k}: ")))?;
                lines.push(format!("  {k}: {}", scalar_text(v)?));
            }
            Some(lines)
        }
        other => Some(vec![format!("{key}: {}", scalar_text(other)?)]),
    }
}

/// Hugo's reading of a draft value: a boolean, or the text of one.
fn as_bool(value: &Yaml) -> Option<bool> {
    match value {
        Yaml::Boolean(b) => Some(*b),
        Yaml::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
        Yaml::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn as_text(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Real(r) => Some(r.clone()),
        _ => None,
    }
}

// -- dates ---------------------------------------------------------------------

/// A front matter date: RFC 3339, `YYYY-MM-DD HH:MM:SS -04:00`, or without an
/// offset (local time), or just a day (local midnight). Anything else is
/// `None`, and the caller refuses rather than guess.
pub fn parse_date(text: &str) -> Option<DateTime<FixedOffset>> {
    let text = text.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt);
    }
    for fmt in [
        "%Y-%m-%d %H:%M:%S%.f %:z",
        "%Y-%m-%d %H:%M:%S%.f %z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S %:z",
        "%Y-%m-%d %H:%M:%S %z",
        "%Y-%m-%d %H:%M:%S%:z",
    ] {
        if let Ok(dt) = DateTime::parse_from_str(text, fmt) {
            return Some(dt);
        }
    }
    let naive = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ]
    .iter()
    .find_map(|fmt| NaiveDateTime::parse_from_str(text, fmt).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
    })?;
    naive
        .and_local_timezone(Local)
        .earliest()
        .map(|dt| dt.fixed_offset())
}

fn stamp(dt: &DateTime<FixedOffset>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

// -- from the note ------------------------------------------------------------

/// Post tags from the note's Bear tags: only tags under `prefix`, with it
/// removed; never the publish tag; only the most specific of a nested path;
/// lower case with hyphens for spaces and slashes. An empty prefix publishes
/// none: a public site gets only tags the note was deliberately given.
pub fn post_tags(note_tags: &[String], prefix: &str, publish_tag: &str) -> Vec<String> {
    let prefix = normalize_tag(prefix).trim_matches('/').to_lowercase();
    if prefix.is_empty() {
        return Vec::new();
    }
    let publish = normalize_tag(publish_tag).to_lowercase();
    let mut kept: Vec<String> = Vec::new();
    for tag in note_tags {
        let tag = normalize_tag(tag);
        let lower = tag.to_lowercase();
        if !publish.is_empty() && lower == publish {
            continue;
        }
        if !lower.starts_with(&format!("{prefix}/")) {
            continue;
        }
        let rest = tag[prefix.len() + 1..].trim_matches('/');
        if !rest.is_empty() {
            kept.push(rest.to_string());
        }
    }
    let leaves: Vec<&String> = kept
        .iter()
        .filter(|t| {
            let parent = format!("{}/", t.to_lowercase());
            !kept.iter().any(|o| o.to_lowercase().starts_with(&parent))
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    for tag in leaves {
        let words: Vec<String> = tag
            .split(|c: char| c == '/' || c.is_whitespace())
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().filter(|c| !is_invisible(*c)).collect::<String>())
            .map(|w| w.to_lowercase())
            .collect();
        let tag = words.join("-");
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

fn has_tag(note_tags: &[String], tag: &str) -> bool {
    let tag = normalize_tag(tag).to_lowercase();
    !tag.is_empty()
        && note_tags
            .iter()
            .any(|t| normalize_tag(t).to_lowercase() == tag)
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Run `f` over the parts of `body` that are not code: fenced blocks and
/// inline code spans pass through untouched.
fn outside_code(
    body: &str,
    mut f: impl FnMut(&str) -> Result<String, ExportError>,
) -> Result<String, ExportError> {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in body.split('\n') {
        if is_fence(line) {
            in_fence = !in_fence;
            out.push(line.to_string());
        } else if in_fence {
            out.push(line.to_string());
        } else {
            out.push(outside_inline_code(line, &mut f)?);
        }
    }
    Ok(out.join("\n"))
}

fn outside_inline_code(
    line: &str,
    f: &mut impl FnMut(&str) -> Result<String, ExportError>,
) -> Result<String, ExportError> {
    let mut out = String::new();
    let mut text_start = 0;
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
        let fence = &line[i..i + run];
        // The span closes at the next run of exactly as many backticks.
        let mut j = i + run;
        let mut close = None;
        while let Some(off) = line[j..].find(fence) {
            let at = j + off;
            let len = bytes[at..].iter().take_while(|b| **b == b'`').count();
            if len == run {
                close = Some(at);
                break;
            }
            j = at + len;
        }
        match close {
            Some(at) => {
                out.push_str(&f(&line[text_start..i])?);
                out.push_str(&line[i..at + run]);
                i = at + run;
                text_start = i;
            }
            None => i += run,
        }
    }
    out.push_str(&f(&line[text_start..])?);
    Ok(out)
}

/// Take the note's own tags out of `text` where they start a word: `#tag`,
/// `#multi word#`, `#tag.` (the period stays) and `(#tag)` (both parentheses
/// go). A `#` inside a word, as in a URL's `/#anchor`, is left alone.
fn strip_inline_tags(text: &str, known: &[String]) -> String {
    let mut known: Vec<&String> = known.iter().filter(|k| !k.is_empty()).collect();
    known.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    for (i, _) in text.match_indices('#') {
        let before = text[..i].chars().next_back();
        if !before.is_none_or(|c| c.is_whitespace() || c == '(' || c == '[') {
            continue;
        }
        let rest = &text[i + 1..];
        for tag in &known {
            let Some(head) = rest.get(..tag.len()) else {
                continue;
            };
            if head.to_lowercase() != **tag {
                continue;
            }
            let after = &rest[tag.len()..];
            let mut end = i + 1 + tag.len();
            let ok = if tag.contains(' ') {
                // A multi-word tag needs its closing `#`.
                if after.starts_with('#') {
                    end += 1;
                    true
                } else {
                    false
                }
            } else {
                match after.chars().next() {
                    None => true,
                    Some('#') => {
                        end += 1;
                        true
                    }
                    Some(c) if c.is_whitespace() => true,
                    Some(_) => {
                        // Trailing punctuation belongs to the sentence.
                        let punct: String = after
                            .chars()
                            .take_while(|c| ".,;:!?)]}\"'".contains(*c))
                            .collect();
                        !punct.is_empty()
                            && after[punct.len()..]
                                .chars()
                                .next()
                                .is_none_or(char::is_whitespace)
                    }
                }
            };
            if ok {
                cuts.push((i, end));
                break;
            }
        }
    }
    if cuts.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    let mut last = 0;
    for (start, end) in cuts {
        let (mut start, mut end) = (start, end);
        if text[..start].ends_with('(') && text[end..].starts_with(')') {
            start -= 1;
            end += 1;
        }
        if start < last {
            continue;
        }
        out.push_str(&text[last..start]);
        last = end;
        // One space is enough where the tag sat between two words, and none
        // before punctuation or at either end.
        let next = text[end..].chars().next();
        if next.is_none_or(|c| c == ' ' || ".,;:!?)".contains(c)) {
            if out.ends_with(' ') {
                out.pop();
            } else if out.is_empty() && next == Some(' ') {
                last += 1;
            }
        }
    }
    out.push_str(&text[last..]);
    out
}

/// One stretch of prose made public-safe: `file://` refused, `bear://` links
/// reduced to their text, wiki links to their display text, the note's tags
/// removed.
fn clean_prose(text: &str, known: &[String]) -> Result<String, ExportError> {
    if text.to_lowercase().contains("file://") {
        return err(
            "The note links to a file:// path, which would publish a local path; remove the link first.",
        );
    }
    let text = BEAR_LINK_RE.replace_all(text, "$1");
    let text = BEAR_BARE_RE.replace_all(&text, "");
    let text = WIKI_RE.replace_all(&text, |caps: &regex::Captures| {
        let inner = &caps[1];
        inner
            .rsplit_once('|')
            .map_or(inner, |(_, alias)| alias)
            .trim()
            .to_string()
    });
    Ok(strip_inline_tags(&text, known))
}

/// `clean_prose` over every string inside a front matter value.
fn clean_value(value: &Yaml, known: &[String]) -> Result<Yaml, ExportError> {
    Ok(match value {
        Yaml::String(s) => {
            // A value is not markdown: where a link went, one space is enough.
            let mut text = clean_prose(s, known)?;
            while text.contains("  ") {
                text = text.replace("  ", " ");
            }
            Yaml::String(text.trim().to_string())
        }
        Yaml::Array(items) => Yaml::Array(
            items
                .iter()
                .map(|i| clean_value(i, known))
                .collect::<Result<_, _>>()?,
        ),
        Yaml::Hash(map) => {
            let mut out = yaml_rust2::yaml::Hash::new();
            for (k, v) in map {
                out.insert(k.clone(), clean_value(v, known)?);
            }
            Yaml::Hash(out)
        }
        other => other.clone(),
    })
}

/// What the note says, before any file is looked at.
#[derive(Debug, Clone)]
struct Parsed {
    title: String,
    front: Option<Front>,
    body: String,
}

/// Title, the note's own front matter (only at the very top), and the body
/// with the title line, tag lines and the note's tags taken out and links
/// made public-safe. Code is left alone.
fn parse_note(content: &str, note: &Note) -> Result<Parsed, ExportError> {
    let known: Vec<String> = note
        .tags
        .iter()
        .map(|t| normalize_tag(t).to_lowercase())
        .collect();
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (front, text) = if content.lines().next().map(str::trim_end) == Some("---") {
        match read_front(content) {
            Ok(Some((mut front, rest))) => {
                // Every value the note gives is public prose.
                for value in front.map.values_mut() {
                    *value = clean_value(value, &known)?;
                }
                (Some(front), rest)
            }
            Ok(None) => (None, content.to_string()),
            Err(why) => {
                return err(format!(
                    "The note starts with --- but that is not front matter Bjorn can read: {why}. \
                     Front matter must be YAML keys at the very top of the note."
                ));
            }
        }
    } else {
        (None, content.to_string())
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    let mut title = String::new();
    let note_title = note.title.trim();
    if let Some(line) = lines.get(i) {
        if let Some(rest) = line.strip_prefix("# ") {
            title = rest.trim().to_string();
            i += 1;
        } else if !note_title.is_empty()
            && line.trim_start_matches('#').trim() == note_title.trim_start_matches('#').trim()
        {
            // The title as a plain line or a lower heading.
            i += 1;
        }
    }
    while i < lines.len() && (lines[i].trim().is_empty() || is_tag_line(lines[i])) {
        i += 1;
    }
    let title = front
        .as_ref()
        .and_then(|f| f.get("title"))
        .and_then(as_text)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| (!title.is_empty()).then_some(title))
        .unwrap_or_else(|| note_title.to_string());
    // The title is public prose too, and the slug comes from it.
    let title = clean_prose(&title, &known)?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // Tag lines go (outside fences), then prose is cleaned outside code.
    let mut kept: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in &lines[i.min(lines.len())..] {
        if is_fence(line) {
            in_fence = !in_fence;
        } else if !in_fence && is_tag_line(line) {
            continue;
        }
        kept.push(line);
    }
    let body = outside_code(&kept.join("\n"), |t| clean_prose(t, &known))?;
    let body: Vec<&str> = body.lines().collect();
    let start = body
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(body.len());
    let end = body
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(start, |i| i + 1);
    Ok(Parsed {
        title,
        front,
        body: body[start..end].join("\n"),
    })
}

/// `<!--more-->` after the first paragraph, when the body starts with a
/// plain paragraph and has no divider yet.
fn with_divider(body: &str) -> String {
    if body.contains(MORE) {
        return body.to_string();
    }
    let lines: Vec<&str> = body.lines().collect();
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return body.to_string();
    };
    let raw = lines[first];
    let lead = raw.trim_start();
    let starts_block = raw.starts_with("    ")
        || raw.starts_with('\t')
        || [
            "#", "```", "~~~", "- ", "* ", "+ ", ">", "|", "<", "{{", "![",
        ]
        .iter()
        .any(|p| lead.starts_with(p))
        || lead
            .split_once(['.', ')'])
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if starts_block {
        return body.to_string();
    }
    let end = lines[first..]
        .iter()
        .position(|l| l.trim().is_empty())
        .map_or(lines.len(), |n| first + n);
    // A setext underline makes the "paragraph" a heading; a fence inside it
    // means the blank line that ends it may be inside code.
    let heading_or_code = lines[first + 1..end].iter().any(|l| {
        let t = l.trim();
        is_fence(l)
            || (!t.is_empty() && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-')))
    });
    if heading_or_code {
        return body.to_string();
    }
    let mut out: Vec<&str> = lines[..end].to_vec();
    out.extend(["", MORE]);
    if end < lines.len() {
        out.push("");
        out.extend(lines[end..].iter().skip_while(|l| l.trim().is_empty()));
    }
    out.join("\n")
}

// -- the ledger ------------------------------------------------------------------

/// Which note wrote which file, and what it wrote: the key to finding a post
/// again after its title changed, and to telling a file Bjorn wrote from one
/// it did not.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ledger {
    #[serde(default)]
    pub posts: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub sha1: String,
}

impl Ledger {
    pub fn load(path: &Path) -> Result<Ledger, ExportError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                ExportError(format!(
                    "{} is not readable ({e}); move it aside to start a new one",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
            Err(e) => Err(ExportError(format!("{}: {e}", path.display()))),
        }
    }
}

/// The ledger for a config file: beside it.
pub fn ledger_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(LEDGER_NAME)
}

fn digest(bytes: &[u8]) -> String {
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn file_digest(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| digest(&bytes))
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Is `path` inside `root` (canonical) once its existing part is resolved,
/// and not itself a symlink?
fn contained(path: &Path, root: &Path) -> bool {
    if is_symlink(path) {
        return false;
    }
    let mut existing = path.parent();
    let mut tail: Vec<&std::ffi::OsStr> = path.file_name().into_iter().collect();
    while let Some(dir) = existing {
        if dir.exists() {
            let Ok(canon) = dir.canonicalize() else {
                return false;
            };
            let full = tail.iter().rev().fold(canon, |p, part| p.join(part));
            return full.starts_with(root)
                && tail
                    .iter()
                    .all(|p| p.to_str().is_none_or(|s| s != ".." && s != "."));
        }
        tail.extend(dir.file_name());
        existing = dir.parent();
    }
    false
}

/// Write through a temp file in the same folder, with `mode` permissions.
fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), ExportError> {
    use std::os::unix::fs::PermissionsExt;
    let fail = |e: std::io::Error| ExportError(format!("{}: {e}", path.display()));
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(fail)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".bjorn-")
        .tempfile_in(parent)
        .map_err(fail)?;
    std::io::Write::write_all(&mut tmp, bytes).map_err(fail)?;
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode)).map_err(fail)?;
    tmp.persist(path).map_err(|e| fail(e.error))?;
    Ok(())
}

// -- the plan ------------------------------------------------------------------

/// What is at the target already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nothing: a new post.
    New,
    /// The file this note wrote last time, unchanged since.
    Update,
    /// The file this note wrote, edited since by someone else.
    Edited,
    /// A file Bjorn did not write for this note.
    Foreign,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub note_id: String,
    pub title: String,
    pub target: PathBuf,
    /// `target` relative to the site, for the dialog and the toast.
    pub display: String,
    /// The post's URL path from the permalink pattern.
    pub url: String,
    pub text: String,
    pub draft: bool,
    pub tags: Vec<String>,
    pub status: Status,
    /// Keys of the note's own front matter that were not passed on.
    pub dropped: Vec<String>,
    /// Anything else the dialog must say.
    pub warnings: Vec<String>,
    /// The target's digest when the plan was made; the write refuses if it
    /// moved in between.
    pub seen: Option<String>,
    /// Images to write, renamed for the web.
    pub media: Vec<(PathBuf, Vec<u8>)>,
    /// Where the images wait for upload; `None` in a bundle, where they are
    /// part of the post.
    pub staging: Option<PathBuf>,
    /// The canonical `content/<section>` the post and a bundle's images must
    /// stay inside.
    pub root: PathBuf,
    pub ledger: PathBuf,
}

impl Plan {
    /// The confirm dialog's text: what happens, then the note's title on its
    /// own line (cleaned and capped), then the facts, one per line.
    pub fn question(&self) -> String {
        let head = match self.status {
            Status::New => "Publish a new Hugo post?".to_string(),
            Status::Update => "Update this Hugo post?".to_string(),
            Status::Edited => "Update this Hugo post? It was edited since Bjorn wrote it: \
                               the body is replaced, front matter keys are kept."
                .to_string(),
            Status::Foreign => "Merge into a post Bjorn did not write for this note? \
                                Its front matter keys are kept; the body is replaced."
                .to_string(),
        };
        let mut lines = vec![
            head,
            format!("“{}”", clean_display(&self.title, DIALOG_TITLE_CHARS)),
            format!("File: {}", clean_display(&self.display, 200)),
            format!("URL: {}", clean_display(&self.url, 200)),
            format!(
                "State: {}",
                if self.draft {
                    "draft"
                } else {
                    "published (live)"
                }
            ),
            format!(
                "Tags: {}",
                if self.tags.is_empty() {
                    "none".to_string()
                } else {
                    clean_display(&self.tags.join(", "), 200)
                }
            ),
        ];
        if !self.dropped.is_empty() {
            lines.push(format!(
                "Not published from the note's front matter: {}",
                clean_display(&self.dropped.join(", "), 200)
            ));
        }
        lines.extend(self.warnings.iter().map(|w| clean_display(w, 300)));
        lines.join("\n")
    }
}

fn is_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// A web-safe name for an image, unique within the note, never `index.*` or
/// `_index.*` (a bundle's own page).
fn media_name(name: &str, prefix: &str, taken: &mut Vec<String>) -> String {
    let path = Path::new(name);
    let ext = path
        .extension()
        .map(|e| slugify(&e.to_string_lossy()))
        .unwrap_or_default();
    let stem = slugify(&path.file_stem().unwrap_or_default().to_string_lossy());
    let stem = if stem.is_empty() {
        "image".to_string()
    } else {
        stem
    };
    let base = if prefix.is_empty() {
        stem
    } else {
        format!("{prefix}-{stem}")
    };
    let mut n = 1;
    loop {
        let candidate_stem = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let candidate = format!("{candidate_stem}.{ext}");
        let reserved = matches!(candidate_stem.as_str(), "index" | "_index");
        if !reserved && !taken.contains(&candidate) {
            taken.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

/// The post's path under `content/{section}` from the pattern.
fn relative_path(
    pattern: &str,
    date: &DateTime<FixedOffset>,
    slug: &str,
) -> Result<PathBuf, ExportError> {
    let rel = pattern
        .trim()
        .replace("{year}", &format!("{:04}", date.year()))
        .replace("{month}", &format!("{:02}", date.month()))
        .replace("{day}", &format!("{:02}", date.day()))
        .replace("{slug}", slug);
    let path = PathBuf::from(&rel);
    let safe = path.components().all(|c| matches!(c, Component::Normal(_)));
    if !safe || path.extension().is_none_or(|e| e != "md") {
        return err(format!(
            "[hugo] path must be a relative .md path, got “{pattern}”"
        ));
    }
    Ok(path)
}

fn permalink(pattern: &str, section: &str, date: &DateTime<FixedOffset>, slug: &str) -> String {
    pattern
        .trim()
        .replace(":year", &format!("{:04}", date.year()))
        .replace(":month", &format!("{:02}", date.month()))
        .replace(":day", &format!("{:02}", date.day()))
        .replace(":slug", slug)
        .replace(":section", section)
}

/// The slug a post answers to: its `slug:`, or its file name (a bundle's
/// folder name).
fn post_slug(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    if let Ok(Some((front, _))) = read_front(&text)
        && let Some(slug) = front.get("slug").and_then(as_text)
    {
        return Some(slugify(&slug));
    }
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    if matches!(stem.as_str(), "index" | "_index") {
        return path
            .parent()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
    }
    Some(stem)
}

/// Every post under `dir` answering to `slug`. Symlinks are not followed.
fn posts_with_slug(dir: &Path, slug: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut budget = 20_000;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            budget -= 1;
            if budget == 0 {
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth < 8 {
                stack.push((path, depth + 1));
            } else if kind.is_file()
                && path.extension().is_some_and(|e| e == "md")
                && post_slug(&path).as_deref() == Some(slug)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The canonical site and `content/<section>` folders, refusing a section
/// that leaves `content/`.
fn site_dirs(cfg: &HugoConfig) -> Result<(PathBuf, PathBuf, String), ExportError> {
    let site = cfg.site_dir();
    if !site.is_dir() {
        return err(format!("{} is not a folder", site.display()));
    }
    let site = site
        .canonicalize()
        .map_err(|e| ExportError(format!("{}: {e}", site.display())))?;
    let content = site.join("content");
    if !content.is_dir() {
        return err(format!(
            "{} has no content folder; is it a Hugo site?",
            site.display()
        ));
    }
    let content = content
        .canonicalize()
        .map_err(|e| ExportError(e.to_string()))?;
    let section = cfg.section.trim().trim_matches('/');
    let section = if section.is_empty() {
        DEFAULT_SECTION
    } else {
        section
    };
    let rel = Path::new(section);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return err(format!(
            "[hugo] section must be a folder inside content/, got “{section}”"
        ));
    }
    let dir = content.join(rel);
    let dir = if dir.exists() {
        let canon = dir.canonicalize().map_err(|e| ExportError(e.to_string()))?;
        if !canon.starts_with(&content) || is_symlink(&dir) {
            return err(format!(
                "content/{section} leads outside the site's content folder"
            ));
        }
        canon
    } else {
        dir
    };
    Ok((site, dir, section.to_string()))
}

/// Work out what publishing `note` would write. Reads the ledger and the
/// site, writes nothing. `now` is the publish time: a new post's date.
pub fn plan(
    cfg: &HugoConfig,
    ledger_path: &Path,
    note: &Note,
    content: &str,
    attachments: &HashMap<String, Vec<u8>>,
    now: DateTime<Utc>,
) -> Result<Plan, ExportError> {
    if !cfg.configured() {
        return err("Set site under [hugo] in the config to publish.");
    }
    let (site, root, section) = site_dirs(cfg)?;
    let ledger = Ledger::load(ledger_path)?;
    let parsed = parse_note(content, note)?;
    let now = now.with_timezone(&Local).fixed_offset();
    let rel = |p: &Path| {
        p.strip_prefix(&site)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    };
    let mut warnings: Vec<String> = Vec::new();

    // Ledger entries count only inside this site's section.
    let valid = |e: &Entry| contained(&e.path, &root);
    let owned_by_other = |p: &Path| {
        ledger
            .posts
            .iter()
            .any(|(id, e)| id != &note.id && e.path == p && valid(e))
    };
    let mine = ledger.posts.get(&note.id).filter(|e| valid(e));

    // The note's own front matter: slug and date shape a new post only.
    let note_front = parsed.front.as_ref();
    let note_slug = note_front
        .and_then(|f| f.get("slug"))
        .and_then(as_text)
        .map(|s| slugify(&s))
        .filter(|s| !s.is_empty());
    let note_date = match note_front.and_then(|f| f.get("date")) {
        None => None,
        Some(value) => Some(as_text(value).as_deref().and_then(parse_date).ok_or_else(|| {
            ExportError(format!(
                "The note's date: {value:?} is not a date Bjorn can read (use 2026-09-15 or 2026-09-15T06:15:00-04:00)."
            ))
        })?),
    };

    let (target, status, chosen_slug) = match mine.filter(|e| e.path.is_file()) {
        Some(entry) => {
            let status = if file_digest(&entry.path).as_deref() == Some(entry.sha1.as_str()) {
                Status::Update
            } else {
                Status::Edited
            };
            (entry.path.clone(), status, None)
        }
        None => {
            if let Some(gone) = mine {
                warnings.push(format!(
                    "The post Bjorn wrote at {} is gone (moved or deleted); this writes it again.",
                    rel(&gone.path)
                ));
            }
            let base = note_slug.clone().unwrap_or_else(|| slugify(&parsed.title));
            if base.is_empty() {
                return err(format!(
                    "“{}” makes no slug; give the note a title with letters or digits, or a slug: in its front matter",
                    clean_display(&parsed.title, DIALOG_TITLE_CHARS)
                ));
            }
            let date = note_date.unwrap_or(now);
            let mut chosen = None;
            for n in 1..=99 {
                let slug = if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                };
                let matches = if root.is_dir() {
                    posts_with_slug(&root, &slug)
                } else {
                    Vec::new()
                };
                let computed = root.join(relative_path(&cfg.path, &date, &slug)?);
                if owned_by_other(&computed) || matches.iter().any(|p| owned_by_other(p)) {
                    continue;
                }
                if n > 1 {
                    warnings.push(format!(
                        "Another note's post already uses the slug {base}; this one gets {slug}."
                    ));
                }
                chosen = Some(match matches.into_iter().next() {
                    Some(found) => {
                        if found != computed {
                            warnings.push(format!(
                                "A post with the slug {slug} already exists at {}; this updates it.",
                                rel(&found)
                            ));
                        }
                        (found, Status::Foreign, Some(slug))
                    }
                    None if computed.exists() || is_symlink(&computed) => {
                        let theirs = post_slug(&computed).unwrap_or_default();
                        warnings.push(format!(
                            "{} already exists with the slug “{theirs}”, not “{slug}”; it keeps its own slug and URL.",
                            rel(&computed)
                        ));
                        (computed, Status::Foreign, Some(slug))
                    }
                    None => (computed, Status::New, Some(slug)),
                });
                break;
            }
            chosen.ok_or_else(|| ExportError(format!("No free slug for {base}")))?
        }
    };
    if !contained(&target, &root) {
        return err(format!(
            "{} is outside content/{section} or is a symlink; refusing to write it",
            target.display()
        ));
    }

    // The post as it is now, if there is one: it must read cleanly.
    let (existing, seen) = if target.exists() {
        let bytes =
            std::fs::read(&target).map_err(|e| ExportError(format!("{}: {e}", rel(&target))))?;
        let seen = Some(digest(&bytes));
        let text = String::from_utf8(bytes).map_err(|_| {
            ExportError(format!(
                "{} is not UTF-8 text; refusing to edit it",
                rel(&target)
            ))
        })?;
        let front = read_front(&text)
            .map_err(|why| ExportError(format!("{}: {why}; refusing to edit it", rel(&target))))?;
        (Some(front.map(|(f, _)| f)), seen)
    } else {
        (None, None)
    };
    let existing_front = existing.clone().flatten();
    let exists = existing.is_some();

    // Draft: the publish tag, then the note's own draft:, and a live post
    // (draft false or unset) stays live whatever the note says.
    let mut draft = !has_tag(&note.tags, &cfg.publish_tag);
    let mut dropped: Vec<String> = Vec::new();
    if let Some(value) = note_front.and_then(|f| f.get("draft")) {
        match as_bool(value) {
            Some(b) => draft = b,
            None => dropped.push("draft (not true or false)".into()),
        }
    }
    let was_live = exists
        && existing_front
            .as_ref()
            .and_then(|f| f.get("draft"))
            .and_then(as_bool)
            != Some(true);
    if was_live {
        draft = false;
    }

    // Date and slug: a post keeps its own; a new one takes the note's or now.
    let existing_date =
        match existing_front.as_ref().and_then(|f| f.get("date")) {
            None => None,
            Some(value) => Some(as_text(value).as_deref().and_then(parse_date).ok_or_else(
                || {
                    ExportError(format!(
                        "{}: date: {value:?} is not a date Bjorn can read; refusing to guess",
                        rel(&target)
                    ))
                },
            )?),
        };
    let date = existing_date
        .or(note_date.filter(|_| !exists))
        .unwrap_or(now);
    let target_slug = existing_front
        .as_ref()
        .and_then(|f| f.get("slug"))
        .and_then(as_text)
        .map(|s| slugify(&s))
        .filter(|s| !s.is_empty())
        .or(chosen_slug)
        .or_else(|| post_slug(&target))
        .unwrap_or_default();

    let mut explicit_tags = false;
    let tags: Vec<String> = match note_front.and_then(|f| f.get("tags")) {
        Some(Yaml::Array(items)) if items.iter().all(|i| as_text(i).is_some()) => {
            explicit_tags = true;
            items.iter().filter_map(as_text).collect()
        }
        Some(_) => {
            dropped.push("tags (not a list)".into());
            post_tags(&note.tags, &cfg.tag_prefix, &cfg.publish_tag)
        }
        None => post_tags(&note.tags, &cfg.tag_prefix, &cfg.publish_tag),
    };

    let mut generated = vec![
        Block::new("title", vec![format!("title: {}", quoted(&parsed.title))]),
        Block::new("slug", vec![format!("slug: {}", quoted(&target_slug))]),
        Block::new("date", vec![format!("date: {}", stamp(&date))]),
    ];
    if exists {
        generated.push(Block::new(
            "lastmod",
            vec![format!("lastmod: {}", stamp(&now))],
        ));
    }
    generated.push(Block::new("draft", vec![format!("draft: {draft}")]));
    generated.push(Block::new(
        "tags",
        if tags.is_empty() {
            vec!["tags: []".into()]
        } else {
            std::iter::once("tags:".to_string())
                .chain(tags.iter().map(|t| format!("  - {}", tag_item(t))))
                .collect()
        },
    ));
    generated.push(Block::new("description", vec!["description: \"\"".into()]));

    // A post's own tags are never wiped by a note that has none to give,
    // nor replaced in a post Bjorn did not write, unless the note's front
    // matter names tags itself.
    let existing_tags = existing_front
        .as_ref()
        .and_then(|f| f.get("tags"))
        .is_some_and(|t| t.as_vec().is_some_and(|v| !v.is_empty()));
    let keep_tags =
        existing_tags && !explicit_tags && (tags.is_empty() || status == Status::Foreign);
    let mut blocks = existing_front
        .as_ref()
        .map(|f| f.blocks.clone())
        .unwrap_or_default();
    for block in generated {
        let key = block.key.clone().unwrap_or_default();
        let keep = find(&blocks, &key).is_some()
            && (matches!(key.as_str(), "slug" | "date" | "description")
                || (key == "tags" && keep_tags));
        if !keep {
            upsert(&mut blocks, block);
        }
    }
    // The rest of the note's own keys: allowed ones, written fresh.
    if let Some(front) = note_front {
        for (key, value) in &front.map {
            let key = key.as_str().unwrap_or_default();
            let lower = key.to_lowercase();
            if !NOTE_KEYS.contains(&lower.as_str()) {
                dropped.push(key.to_string());
                continue;
            }
            // Consumed above.
            if matches!(lower.as_str(), "title" | "slug" | "date" | "draft" | "tags") {
                continue;
            }
            match emit(&lower, value) {
                Some(lines) => upsert(&mut blocks, Block::new(&lower, lines)),
                None => dropped.push(format!("{key} (not a simple value)")),
            }
        }
    }

    // Images: renamed for the web, links pointed at where they will live.
    let bundle = cfg.bundle() && target.file_name().is_some_and(|n| n == "index.md");
    let media_base = cfg.media_url.trim().trim_end_matches('/');
    let staging = cfg
        .media_dir
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()));
    let by_nfc: HashMap<String, &String> = attachments.keys().map(|k| (nfc(k), k)).collect();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut media: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    let body = outside_code(&parsed.body, |text| {
        let mut out = String::new();
        let mut last = 0;
        for caps in LINK_RE.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            out.push_str(&text[last..whole.start()]);
            last = whole.end();
            let image = &caps[1] == "!";
            let raw = caps[3].trim_start_matches('<').trim_end_matches('>');
            let decoded = nfc(&percent_decode(raw));
            let Some(name) = by_nfc.get(&decoded).copied() else {
                let relative = !decoded.contains("://")
                    && !decoded.starts_with(['/', '#'])
                    && !decoded.to_lowercase().starts_with("data:")
                    && !decoded.to_lowercase().starts_with("mailto:");
                if image && relative {
                    return err(format!(
                        "The image {} is not an attachment of this note, so it would be a broken link.",
                        clean_display(&decoded, 80)
                    ));
                }
                out.push_str(whole.as_str());
                continue;
            };
            if !is_image(name) {
                if image {
                    return err(format!(
                        "{} is not a {} image; only those are published.",
                        clean_display(name, 80),
                        IMAGE_EXTENSIONS.join("/")
                    ));
                }
                warnings.push(format!(
                    "The link to the attachment {} became plain text: only images are published.",
                    clean_display(name, 80)
                ));
                out.push_str(&caps[2]);
                continue;
            }
            if !bundle && media_base.is_empty() {
                return err(
                    "The note has images: set media_url under [hugo], or use a page bundle path ({year}/{month}/{slug}/index.md).",
                );
            }
            let url = match renamed.get(name) {
                Some(url) => url.clone(),
                None => {
                    let file = media_name(name, if bundle { "" } else { &target_slug }, &mut taken);
                    let (dest, url) = if bundle {
                        let dir = target.parent().unwrap_or(Path::new("."));
                        (dir.join(&file), file.clone())
                    } else {
                        (
                            staging.join(&file),
                            format!("{media_base}/{:04}/{:02}/{file}", date.year(), date.month()),
                        )
                    };
                    if dest == target {
                        return err("An image would overwrite the post itself.");
                    }
                    media.push((dest, attachments[name].clone()));
                    renamed.insert(name.clone(), url.clone());
                    url
                }
            };
            out.push_str(&format!("{}[{}]({url}{})", &caps[1], &caps[2], &caps[4]));
        }
        out.push_str(&text[last..]);
        Ok(out)
    })?;
    let body = if cfg.summary_divider {
        with_divider(&body)
    } else {
        body
    };

    // Check what is about to be written parses, and says what was meant.
    let front_text = render_blocks(&blocks);
    let text = if body.trim().is_empty() {
        front_text
    } else {
        format!("{front_text}\n{}\n", body.trim_end())
    };
    let check = validated(&text, draft).map_err(|why| {
        ExportError(format!(
            "The front matter for {} did not validate ({why}); nothing was written.",
            rel(&target)
        ))
    })?;
    let final_tags: Vec<String> = match check.get("tags") {
        Some(Yaml::Array(items)) => items.iter().filter_map(as_text).collect(),
        _ => Vec::new(),
    };
    dropped.sort();
    dropped.dedup();

    Ok(Plan {
        note_id: note.id.clone(),
        title: check.get("title").and_then(as_text).unwrap_or(parsed.title),
        display: rel(&target),
        url: permalink(&cfg.permalink, &section, &date, &target_slug),
        target,
        text,
        draft,
        tags: final_tags,
        status,
        dropped,
        warnings,
        seen,
        staging: (!bundle && !media.is_empty()).then_some(staging),
        media,
        root,
        ledger: ledger_path.to_path_buf(),
    })
}

/// The last check before anything is written: the whole text reads back as
/// YAML front matter, with the draft state that was meant and a real date.
fn validated(text: &str, draft: bool) -> Result<Front, String> {
    let front = read_front(text)?.ok_or("no front matter")?.0;
    if front.get("draft").and_then(as_bool) != Some(draft) {
        return Err("draft does not read back as written".into());
    }
    if front
        .get("date")
        .and_then(as_text)
        .and_then(|d| parse_date(&d))
        .is_none()
    {
        return Err("date does not read back as a date".into());
    }
    Ok(front)
}

/// Carry out `plan`: images first, then the post, then the ledger, so a
/// failure never leaves a post pointing at images that are not there or a
/// ledger naming a file that was not written. Returns the toast.
pub fn write(plan: &Plan) -> Result<String, ExportError> {
    let current = if plan.target.exists() {
        file_digest(&plan.target)
    } else {
        None
    };
    if current != plan.seen {
        return err(format!(
            "{} changed while the dialog was up; publish again to see it",
            plan.display
        ));
    }
    for (path, bytes) in &plan.media {
        if is_symlink(path) {
            return err(format!(
                "{} is a symlink; refusing to write it",
                path.display()
            ));
        }
        if plan.staging.is_none() && !contained(path, &plan.root) {
            return err(format!("{} is outside the site", path.display()));
        }
        write_atomic(path, bytes, 0o644)?;
    }
    if let Some(parent) = plan.target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ExportError(format!("{}: {e}", parent.display())))?;
    }
    if !contained(&plan.target, &plan.root) {
        return err(format!(
            "{} is outside the section or is a symlink; refusing to write it",
            plan.display
        ));
    }
    write_atomic(&plan.target, plan.text.as_bytes(), 0o644)?;
    let mut ledger = Ledger::load(&plan.ledger)?;
    ledger.posts.insert(
        plan.note_id.clone(),
        Entry {
            path: plan.target.clone(),
            sha1: digest(plan.text.as_bytes()),
        },
    );
    let json = serde_json::to_string_pretty(&ledger).map_err(|e| ExportError(e.to_string()))?;
    write_atomic(&plan.ledger, format!("{json}\n").as_bytes(), 0o600)?;

    let mut report = format!(
        "{} ({})",
        plan.display,
        if plan.draft { "draft" } else { "published" }
    );
    if let Some(dir) = &plan.staging {
        let names: Vec<String> = plan
            .media
            .iter()
            .filter_map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        report.push_str(&format!(
            ". Upload {} from {}: {}",
            if names.len() == 1 {
                "1 file".to_string()
            } else {
                format!("{} files", names.len())
            },
            tilde(dir),
            names.join(", ")
        ));
    } else if !plan.media.is_empty() {
        report.push_str(&format!(", {} images beside it", plan.media.len()));
    }
    Ok(report)
}

fn tilde(path: &Path) -> String {
    let home = home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if home != Path::new("/") => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_new_post_sh() {
        for (title, slug) in [
            ("My Cool Hack", "my-cool-hack"),
            (
                "The Internet Changed. So Did This Blog.",
                "the-internet-changed-so-did-this-blog",
            ),
            (
                "Pixel-Perfect: Turning AI Pixel Art Into Real Pixel Art",
                "pixel-perfect-turning-ai-pixel-art-into-real-pixel-art",
            ),
            ("  --Hello__World!!  ", "hello-world"),
            ("C++ & Rust: 2026?", "c-rust-2026"),
            ("a/b\\c", "a-b-c"),
            ("Café Tips — ÉCOLE", "caf-tips-cole"),
            ("émoji 🎉 time", "moji-time"),
            ("!!!", ""),
        ] {
            assert_eq!(slugify(title), slug, "{title}");
        }
    }

    /// The same titles through the script's own pipeline, when the tools are
    /// there. The C locale is how the script handles non-ASCII at all; under
    /// UTF-8 macOS sed stops with "illegal byte sequence".
    #[test]
    fn slugify_agrees_with_the_shell_pipeline() {
        if !Path::new("/usr/bin/sed").exists() || crate::util::which("tr").is_none() {
            return;
        }
        for title in [
            "My Cool Hack",
            "Pixel-Perfect: Turning AI Pixel Art Into Real Pixel Art",
            "What's new in v2.0 (beta)?",
            "  --Hello__World!!  ",
            "Café Tips — ÉCOLE",
            "Ünïcödé only 42",
        ] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(r#"printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | /usr/bin/sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g'"#)
                .arg("sh")
                .arg(title)
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end(),
                slugify(title),
                "{title}"
            );
        }
    }

    #[test]
    fn tags_map_under_the_prefix_and_an_empty_prefix_publishes_none() {
        let tags: Vec<String> = [
            "blog",
            "blog/rust",
            "blog/image processing",
            "blog/lang",
            "blog/lang/go",
            "blog/published",
            "work/secret",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            post_tags(&tags, "blog", "blog/published"),
            ["rust", "image-processing", "lang-go"]
        );
        assert_eq!(
            post_tags(&tags, "#blog", "#blog/published"),
            post_tags(&tags, "blog", "blog/published")
        );
        assert!(post_tags(&tags, "", "").is_empty(), "decision D");
        assert!(post_tags(&tags, "home", "").is_empty());
    }

    #[test]
    fn inline_tags_go_only_as_the_notes_own_at_a_word_start() {
        let known = vec!["private".to_string(), "multi word".to_string()];
        assert_eq!(strip_inline_tags("see #private here", &known), "see here");
        assert_eq!(strip_inline_tags("end #private", &known), "end");
        assert_eq!(strip_inline_tags("it is #private.", &known), "it is.");
        assert_eq!(strip_inline_tags("a (#private) b", &known), "a b");
        assert_eq!(strip_inline_tags("x #multi word# y", &known), "x y");
        assert_eq!(
            strip_inline_tags("https://x.test/#private and #other", &known),
            "https://x.test/#private and #other"
        );
        assert_eq!(strip_inline_tags("#privateer", &known), "#privateer");
        assert_eq!(strip_inline_tags("#private/sub", &known), "#private/sub");
    }

    #[test]
    fn prose_cleaning_flattens_wiki_and_bear_links_and_refuses_file_links() {
        let known: Vec<String> = Vec::new();
        assert_eq!(
            clean_prose("see [[Other Note]] and [[Other|that one]]", &known).unwrap(),
            "see Other Note and that one"
        );
        assert_eq!(
            clean_prose(
                "[this note](bear://x-callback-url/open-note?id=ABC) and <bear://x>",
                &known
            )
            .unwrap(),
            "this note and "
        );
        assert!(clean_prose("[f](file:///Users/me/secret.txt)", &known).is_err());
    }

    #[test]
    fn inline_code_and_fences_are_left_alone() {
        let out = outside_code("a `[[x]]` b [[y]]\n```\n[[z]]\n```", |t| {
            Ok(WIKI_RE.replace_all(t, "W").into_owned())
        })
        .unwrap();
        assert_eq!(out, "a `[[x]]` b W\n```\n[[z]]\n```");
        let out = outside_code("``a ` [[x]]`` [[y]]", |t| {
            Ok(WIKI_RE.replace_all(t, "W").into_owned())
        })
        .unwrap();
        assert_eq!(out, "``a ` [[x]]`` W");
    }

    #[test]
    fn divider_goes_after_a_plain_lead_paragraph_only() {
        assert_eq!(
            with_divider("Lead one\nstill lead\n\n## Next\ntext"),
            "Lead one\nstill lead\n\n<!--more-->\n\n## Next\ntext"
        );
        assert_eq!(with_divider("## Heading first"), "## Heading first");
        assert_eq!(
            with_divider("a\n\n<!--more-->\n\nb"),
            "a\n\n<!--more-->\n\nb"
        );
        assert_eq!(with_divider("Only"), "Only\n\n<!--more-->");
        assert_eq!(with_divider("Setext\n======\n\nx"), "Setext\n======\n\nx");
        assert_eq!(with_divider("Setext\n---\n\nx"), "Setext\n---\n\nx");
        assert_eq!(
            with_divider("    indented code\n\nx"),
            "    indented code\n\nx"
        );
        assert_eq!(
            with_divider("Lead\n```\ncode\n\nmore\n```"),
            "Lead\n```\ncode\n\nmore\n```"
        );
        assert_eq!(with_divider("1) item"), "1) item");
    }

    #[test]
    fn dates_parse_or_are_refused() {
        let local = |s: &str| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .unwrap()
                .and_local_timezone(Local)
                .earliest()
                .unwrap()
                .fixed_offset()
        };
        assert_eq!(
            parse_date("2026-09-15T06:15:00-04:00")
                .map(|d| stamp(&d))
                .as_deref(),
            Some("2026-09-15T06:15:00-04:00")
        );
        assert_eq!(
            parse_date("2026-09-15 06:15:00 -04:00")
                .map(|d| stamp(&d))
                .as_deref(),
            Some("2026-09-15T06:15:00-04:00")
        );
        assert_eq!(
            parse_date("2026-09-15T06:15:00"),
            Some(local("2026-09-15 06:15:00"))
        );
        assert_eq!(
            parse_date("2026-09-15 06:15:00"),
            Some(local("2026-09-15 06:15:00"))
        );
        assert_eq!(parse_date("2026-09-15"), Some(local("2026-09-15 00:00:00")));
        for bad in ["yesterday", "15/09/2026", "2026-13-01", ""] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn front_matter_is_read_with_a_parser() {
        let (front, rest) = read_front("\u{feff}---\ntitle: \"T\"\nDraft: false # live\n---\nbody")
            .unwrap()
            .unwrap();
        assert_eq!(
            front.get("draft").and_then(as_bool),
            Some(false),
            "comment and case"
        );
        assert_eq!(rest, "body");
        assert!(read_front("+++\ntitle = 'x'\n+++\n").is_err());
        assert!(read_front("{\"title\": 1}\n").is_err());
        assert!(
            read_front("---\ntitle: a\nTitle: b\n---\n")
                .unwrap_err()
                .contains("twice")
        );
        assert!(read_front("---\ntitle: a\ntitle: b\n---\n").is_err());
        assert!(read_front("---\nEdit: the fix is out: see v2.\n---\n").is_err());
        assert!(read_front("---\n- a\n- b\n---\n").is_err());
        assert!(read_front("---\ntitle: a\n").is_err(), "no closing fence");
        assert!(read_front("no front matter").unwrap().is_none());
        let (front, _) = read_front("---\ndescription: >\n  folded\n  text\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(
            front.get("description").and_then(as_text).as_deref(),
            Some("folded text\n")
        );
    }

    #[test]
    fn upsert_keeps_comments_and_order() {
        let (front, _) = read_front(
            "---\ntitle: \"Old\"\n# URL slug comment\nslug: \"s\"\ncover:\n  image: a\n# tail\n---\nbody",
        )
        .unwrap()
        .unwrap();
        let mut blocks = front.blocks;
        upsert(&mut blocks, Block::new("slug", vec!["slug: \"t\"".into()]));
        upsert(
            &mut blocks,
            Block::new("date", vec!["date: 2026-01-01T00:00:00Z".into()]),
        );
        upsert(&mut blocks, Block::new("custom", vec!["custom: 1".into()]));
        assert_eq!(
            render_blocks(&blocks),
            "---\ntitle: \"Old\"\n# URL slug comment\nslug: \"t\"\ndate: 2026-01-01T00:00:00Z\ncover:\n  image: a\ncustom: 1\n# tail\n---\n"
        );
    }

    #[test]
    fn the_final_check_rejects_what_does_not_read_back() {
        let good = "---\ntitle: \"T\"\ndate: 2026-09-19T10:00:00-04:00\ndraft: true\n---\n\nBody\n";
        assert!(validated(good, true).is_ok());
        assert!(validated(good, false).unwrap_err().contains("draft"));
        for bad in [
            "---\ntitle: [unclosed\ndate: 2026-09-19\ndraft: true\n---\n",
            "---\ntitle: a: b\ndate: 2026-09-19\ndraft: true\n---\n",
            "---\ntitle: x\nTitle: y\ndate: 2026-09-19\ndraft: true\n---\n",
            "---\ntitle: x\ndate: soon\ndraft: true\n---\n",
            "---\ntitle: x\ndate: 2026-09-19\ndraft: true\n",
            "no front matter",
        ] {
            assert!(validated(bad, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn dialog_text_is_cleaned_and_capped() {
        let evil = "Safe\u{202e}txt.exe\u{200b} and a very long title that keeps going and going well past the cap";
        let clean = clean_display(evil, DIALOG_TITLE_CHARS);
        assert!(!clean.contains('\u{202e}') && !clean.contains('\u{200b}'));
        assert!(clean.chars().count() <= DIALOG_TITLE_CHARS && clean.ends_with('…'));
    }

    #[test]
    fn media_names_never_take_a_bundles_page_name() {
        let mut taken = Vec::new();
        assert_eq!(media_name("index.png", "", &mut taken), "index-2.png");
        assert_eq!(media_name("_index.PNG", "", &mut taken), "index-3.png");
        assert_eq!(media_name("Front bed.png", "", &mut taken), "front-bed.png");
        assert_eq!(
            media_name("front-bed.png", "", &mut taken),
            "front-bed-2.png"
        );
    }
}
