//! A local stand-in for bearcli, used by the test suite and `bjorn --demo`.
//!
//! Implements the subset Bjorn drives with the real tool's contract: `--format
//! json` puts one JSON document on stdout for reads and `{"error": {...}}` for
//! their failures; writes print nothing on success and plain text on stderr
//! (exit 1) when they fail. State lives in a JSON file at
//! $BJORN_FAKE_BEAR_STATE (default ~/.cache/bjorn/demo-bear.json), seeded with
//! sample notes on first run. Every `app open` call is appended to
//! `<state>.opened` so tests can assert on it.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::LazyLock;

use base64::Engine;
use clap::{Args, Parser, Subcommand};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::Digest;

use crate::util::{home_dir, now_iso};

const META_FIELDS: [&str; 12] = [
    "id",
    "title",
    "locked",
    "tags",
    "length",
    "created",
    "modified",
    "pins",
    "location",
    "todos",
    "done",
    "attachments",
];
const DEFAULT_LIST_FIELDS: [&str; 4] = ["id", "title", "tags", "length"];

/// A 1x1 transparent PNG, base64: the seeded attachment.
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

pub fn state_path() -> PathBuf {
    match std::env::var_os("BJORN_FAKE_BEAR_STATE").filter(|v| !v.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => home_dir().join(".cache/bjorn/demo-bear.json"),
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct FakeNote {
    id: String,
    title: String,
    tags: Vec<String>,
    locked: bool,
    pins: Vec<String>,
    location: String,
    created: String,
    modified: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_data: Option<BTreeMap<String, String>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct State {
    notes: Vec<FakeNote>,
    next_id: u64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[allow(clippy::too_many_arguments)]
fn note(
    id: &str,
    title: &str,
    tags: &[&str],
    pins: &[&str],
    location: &str,
    created: &str,
    modified: &str,
    content: String,
) -> FakeNote {
    FakeNote {
        id: id.into(),
        title: title.into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        locked: false,
        pins: pins.iter().map(|p| p.to_string()).collect(),
        location: location.into(),
        created: created.into(),
        modified: modified.into(),
        content,
        attachments: Vec::new(),
        attachment_data: None,
        extra: Map::new(),
    }
}

fn seed_state() -> State {
    let today = now_iso();
    let mut garden = note(
        "NOTE-GARDEN",
        "Garden Plan",
        &["home", "home/garden"],
        &["#home"],
        "notes",
        "2026-07-12T09:00:00Z",
        "2026-08-28T10:00:00Z",
        "# Garden Plan\n#home/garden\n\n- [ ] order bulbs for the front bed\n- [x] mulch the roses\n\n\
         ## Next spring\n- [ ] move the hydrangea\n```\n- [ ] this is inside a code block\n```\n\n![](Front%20bed.png)\n"
            .into(),
    );
    garden.attachments = vec!["Front bed.png".into()];
    garden.attachment_data = Some(BTreeMap::from([(
        "Front bed.png".to_string(),
        ONE_PIXEL_PNG.to_string(),
    )]));
    let books: Vec<String> = (1..=120).map(|i| format!("- Book {i}")).collect();
    let notes = vec![
        note(
            "NOTE-PLANNING",
            "Sprint Planning",
            &["work", "work/sprint"],
            &["global"],
            "notes",
            "2026-08-01T09:00:00Z",
            &today,
            "# Sprint Planning\n#work/sprint\n\n## Tasks\n- [x] book the retro room\n- [ ] write the release notes\n\
             - [ ] ask Priya about the API deprecation\n  - [ ] confirm the sunset date\n\n## Notes\nVelocity is ==holding== steady.\n"
                .into(),
        ),
        garden,
        note(
            "NOTE-READING",
            "Reading Queue",
            &["home"],
            &[],
            "notes",
            "2026-06-01T09:00:00Z",
            "2026-08-15T12:00:00Z",
            format!("# Reading Queue\n#home\n\nNo tasks here, just titles.\n\n{}\n", books.join("\n")),
        ),
        note(
            "NOTE-DESIGN",
            "CAD and Design",
            &["work", "work/CAD and Design"],
            &[],
            "notes",
            "2026-05-01T09:00:00Z",
            "2026-08-01T12:00:00Z",
            "# CAD and Design\n#work/CAD and Design#\n\nMulti-word tag note.\n".into(),
        ),
        note("NOTE-UNTAGGED", "Loose Thought", &[], &[], "notes", "2026-04-01T09:00:00Z", "2026-07-01T12:00:00Z", "# Loose Thought\n\nNo tags on this one.\n".into()),
        note("NOTE-TRASHED", "Old Draft", &["work"], &[], "trash", "2026-03-01T09:00:00Z", "2026-06-01T12:00:00Z", "# Old Draft\n#work\n\nThrown away.\n".into()),
        note("NOTE-ARCHIVED", "Finished Project", &["work"], &[], "archive", "2026-02-01T09:00:00Z", "2026-05-01T12:00:00Z", "# Finished Project\n#work\n\nDone and dusted.\n".into()),
    ];
    State {
        notes,
        next_id: 1,
        extra: Map::new(),
    }
}

fn load_state() -> State {
    let path = state_path();
    if path.exists() {
        let text = std::fs::read_to_string(&path).expect("readable state file");
        return serde_json::from_str(&text).expect("valid state file");
    }
    let state = seed_state();
    save_state(&state);
    state
}

/// Atomic: `probe` runs two of us at once, and on first use both seed the
/// file; a reader must never see a half-written one.
fn save_state(state: &State) {
    let text = serde_json::to_string_pretty(state).expect("serializable state");
    super::write_atomic(&state_path(), &text).expect("writable state file");
}

// -- output ----------------------------------------------------------------

/// How a command ends: success, or a failure already explained on the right
/// stream, carrying the exit code.
type CmdResult = Result<(), i32>;

fn emit_json(payload: &Value) {
    println!("{payload}");
}

fn fail(fmt: &str, code: &str, message: &str) -> i32 {
    if fmt == "json" {
        emit_json(&json!({"error": {"code": code, "message": message}}));
    } else {
        eprintln!("Error: {message}");
    }
    1
}

fn fail_text(message: &str, exit_code: i32) -> i32 {
    eprintln!("Error: {message}");
    exit_code
}

fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(py_str).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

fn tsv_row<'a>(values: impl Iterator<Item = &'a Value>) -> String {
    values
        .map(|v| {
            py_str(v)
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\t', "\\t")
        })
        .collect::<Vec<_>>()
        .join("\t")
}

// -- helpers ---------------------------------------------------------------

fn content_hash(content: &str) -> String {
    let digest = sha1::Sha1::digest(content.as_bytes());
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..7]
        .to_string()
}

fn display_tag(tag: &str) -> String {
    if tag.contains(' ') {
        format!("#{tag}#")
    } else {
        format!("#{tag}")
    }
}

fn display_tags(note: &FakeNote) -> Vec<String> {
    note.tags.iter().map(|t| display_tag(t)).collect()
}

static TODO_OPEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*+] \[ \]").unwrap());
static TODO_DONE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*+] \[[xX]\]").unwrap());
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6}) ").unwrap());
static TAG_RE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?m)^#(?![# ])([^\n#]*?)#(?=\s|$)|(?<!\S)#(?![# ])([^\s#]+)").unwrap()
});

fn todo_counts(content: &str) -> (i64, i64) {
    let (mut open, mut done) = (0, 0);
    for line in content.lines() {
        if TODO_OPEN_RE.is_match(line) {
            open += 1;
        } else if TODO_DONE_RE.is_match(line) {
            done += 1;
        }
    }
    (open, done)
}

fn row_for(note: &FakeNote, fields: &[String]) -> Value {
    let (todos, done) = todo_counts(&note.content);
    let mut full = Map::new();
    full.insert("id".into(), json!(note.id));
    full.insert("title".into(), json!(note.title));
    full.insert(
        "locked".into(),
        json!(if note.locked { "yes" } else { "no" }),
    );
    full.insert("tags".into(), json!(display_tags(note)));
    full.insert("length".into(), json!(note.content.chars().count()));
    full.insert("created".into(), json!(note.created));
    full.insert("modified".into(), json!(note.modified));
    full.insert("pins".into(), json!(note.pins));
    full.insert(
        "location".into(),
        json!(if note.location.is_empty() {
            "notes"
        } else {
            &note.location
        }),
    );
    full.insert("todos".into(), json!(todos));
    full.insert("done".into(), json!(done));
    full.insert("attachments".into(), json!(note.attachments));
    full.insert("content".into(), json!(note.content));
    full.insert("matches".into(), json!(0));
    let mut out = Map::new();
    for f in fields {
        if let Some(v) = full.get(f) {
            out.insert(f.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn parse_fields(spec: Option<&str>, default: &[&str], extra: &[&str]) -> Vec<String> {
    let Some(spec) = spec.filter(|s| !s.is_empty()) else {
        return default.iter().map(|s| s.to_string()).collect();
    };
    let mut out = Vec::new();
    for f in spec.split(',') {
        let f = f.trim();
        if f == "all" {
            out.extend(META_FIELDS.iter().map(|s| s.to_string()));
            out.extend(extra.iter().map(|s| s.to_string()));
        } else if !f.is_empty() {
            out.push(f.to_string());
        }
    }
    out
}

fn find_note(state: &State, note_id: Option<&str>, title: Option<&str>) -> Option<usize> {
    state.notes.iter().position(|n| {
        note_id.is_some_and(|id| n.id == id)
            || title.is_some_and(|t| n.title.to_lowercase() == t.to_lowercase())
    })
}

fn strip_tag(tag: &str) -> String {
    tag.trim().trim_matches('#').trim().to_string()
}

fn note_has_tag(note: &FakeNote, tag: &str) -> bool {
    let tag = strip_tag(tag);
    if let Some(tail) = tag.strip_prefix("*/") {
        let tail = tail.to_lowercase();
        return note.tags.iter().any(|t| {
            let parts: Vec<&str> = t.split('/').collect();
            (1..parts.len()).any(|i| {
                let suffix = parts[i..].join("/").to_lowercase();
                suffix == tail || suffix.starts_with(&format!("{tail}/"))
            })
        });
    }
    note.tags
        .iter()
        .any(|t| *t == tag || t.starts_with(&format!("{tag}/")))
}

fn sort_key(note: &FakeNote, field: &str) -> String {
    match field {
        "pinned" => if note.pins.is_empty() { "0" } else { "1" }.to_string(),
        "title" => note.title.to_lowercase(),
        "id" => note.id.clone(),
        "created" => note.created.clone(),
        "modified" => note.modified.clone(),
        "location" => note.location.clone(),
        "content" => note.content.clone(),
        _ => String::new(),
    }
}

/// Python's `list.sort(key=..., reverse=...)` per term, last term first, so
/// the first term wins; both directions keep equal keys in their original order.
fn sort_notes(notes: &mut [FakeNote], spec: &str) {
    let terms: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    for term in terms.iter().rev() {
        let (field, direction) = term.split_once(':').unwrap_or((term, ""));
        let reverse =
            direction == "desc" || (direction.is_empty() && matches!(field, "pinned" | "modified"));
        if reverse {
            notes.sort_by_cached_key(|n| std::cmp::Reverse(sort_key(n, field)));
        } else {
            notes.sort_by_cached_key(|n| sort_key(n, field));
        }
    }
}

fn emit_rows(rows: &[Value], fields: &[String], fmt: &str) {
    match fmt {
        "json" => emit_json(&Value::Array(rows.to_vec())),
        "csv" => {
            println!("{}", fields.join(","));
            for r in rows {
                println!(
                    "{}",
                    fields
                        .iter()
                        .map(|f| r.get(f).cloned().unwrap_or(Value::Null).to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
        }
        _ => {
            if rows.is_empty() {
                eprintln!("No notes found.");
            }
            for r in rows {
                let values: Vec<Value> = fields
                    .iter()
                    .map(|f| r.get(f).cloned().unwrap_or(Value::Null))
                    .collect();
                println!("{}", tsv_row(values.iter()));
            }
        }
    }
}

fn in_location(note: &FakeNote, location: &str) -> bool {
    let own = if note.location.is_empty() {
        "notes"
    } else {
        note.location.as_str()
    };
    location == "all" || own == location
}

fn unescape(text: &str) -> String {
    text.replace("\\n", "\n")
}

fn read_stdin() -> String {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf);
    buf
}

fn with_ancestors(tags: &mut Vec<String>, tag: &str) {
    let parts: Vec<&str> = tag.split('/').collect();
    for i in 0..parts.len() {
        let anc = parts[..=i].join("/");
        if !tags.contains(&anc) {
            tags.push(anc);
        }
    }
}

// -- argv --------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "bearcli",
    version = "fake",
    disable_version_flag = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),
    #[arg(long, global = true, default_value = "tsv", value_parser = ["tsv", "csv", "json"])]
    format: String,
    #[arg(long, global = true)]
    fields: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Args)]
struct Listing {
    #[arg(short = 's', long, default_value = "pinned,modified")]
    sort: String,
    #[arg(short = 'n', long)]
    limit: Option<i64>,
    #[arg(short = 'o', long, default_value_t = 0)]
    offset: i64,
    #[arg(short = 'l', long, default_value = "notes", value_parser = ["notes", "trash", "archive", "all"])]
    location: String,
    #[arg(long)]
    count: bool,
}

#[derive(Args)]
struct Target {
    #[arg(allow_hyphen_values = true)]
    note_id: Option<String>,
    #[arg(short = 't', long, allow_hyphen_values = true)]
    title: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    List {
        #[command(flatten)]
        listing: Listing,
        #[arg(long)]
        tag: Option<String>,
    },
    Search {
        #[arg(allow_hyphen_values = true)]
        query_arg: Option<String>,
        #[arg(short = 'q', long, allow_hyphen_values = true)]
        query: Option<String>,
        #[command(flatten)]
        listing: Listing,
    },
    Cat {
        #[command(flatten)]
        target: Target,
        #[arg(long, allow_hyphen_values = true)]
        section: Option<String>,
    },
    Show {
        #[command(flatten)]
        target: Target,
    },
    Tags {
        #[command(subcommand)]
        cmd: Option<TagsCmd>,
    },
    Create {
        #[arg(allow_hyphen_values = true)]
        title: Option<String>,
        #[arg(short = 'c', long, allow_hyphen_values = true)]
        content: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        tags: Option<String>,
        #[arg(long)]
        if_not_exists: bool,
    },
    Overwrite {
        #[command(flatten)]
        target: Target,
        #[arg(short = 'c', long, allow_hyphen_values = true)]
        content: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        section: Option<String>,
        #[arg(long)]
        no_update_modified: bool,
        #[arg(long)]
        force: bool,
    },
    Edit {
        #[command(flatten)]
        target: Target,
        #[arg(long, allow_hyphen_values = true)]
        section: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        find: String,
        #[arg(long, allow_hyphen_values = true)]
        replace: Option<String>,
        #[arg(long)]
        delete: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        no_update_modified: bool,
    },
    Trash {
        #[command(flatten)]
        target: Target,
    },
    Archive {
        #[command(flatten)]
        target: Target,
    },
    Restore {
        #[command(flatten)]
        target: Target,
    },
    Pin {
        #[command(subcommand)]
        cmd: Option<PinCmd>,
    },
    Attachments {
        #[command(subcommand)]
        cmd: Option<AttCmd>,
    },
    App {
        #[command(subcommand)]
        cmd: AppCmd,
    },
}

#[derive(Subcommand)]
enum TagsCmd {
    List {
        note_id: Option<String>,
        tag_names: Vec<String>,
    },
    Add {
        note_id: Option<String>,
        tag_names: Vec<String>,
    },
    Remove {
        note_id: Option<String>,
        tag_names: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PinCmd {
    List {
        note_id: Option<String>,
        targets: Vec<String>,
    },
    Add {
        note_id: Option<String>,
        targets: Vec<String>,
    },
    Remove {
        note_id: Option<String>,
        targets: Vec<String>,
    },
}

#[derive(Subcommand)]
enum AttCmd {
    List {
        #[command(flatten)]
        target: Target,
    },
    Save {
        #[command(flatten)]
        target: Target,
        #[arg(short = 'f', long, allow_hyphen_values = true)]
        filename: String,
    },
}

#[derive(Subcommand)]
enum AppCmd {
    Open {
        #[command(flatten)]
        target: Target,
        #[arg(long, allow_hyphen_values = true)]
        header: Option<String>,
        #[arg(long)]
        edit: bool,
        #[arg(long)]
        new_window: bool,
    },
}

// -- commands ----------------------------------------------------------------

struct Ctx<'a> {
    fmt: &'a str,
    fields: Option<&'a str>,
}

fn cmd_list(ctx: &Ctx, state: &State, listing: &Listing, tag: Option<&str>) -> CmdResult {
    let mut notes: Vec<FakeNote> = state
        .notes
        .iter()
        .filter(|n| in_location(n, &listing.location))
        .cloned()
        .collect();
    if let Some(tag) = tag {
        notes.retain(|n| note_has_tag(n, tag));
    }
    sort_notes(&mut notes, &listing.sort);
    let total = notes.len();
    if listing.count || listing.limit == Some(0) {
        if ctx.fmt == "json" {
            emit_json(&json!({"count": total}));
        } else {
            println!("{total}");
        }
        return Ok(());
    }
    let page = paginate(&notes, listing.offset, listing.limit);
    let fields = parse_fields(ctx.fields, &DEFAULT_LIST_FIELDS, &[]);
    let rows: Vec<Value> = page.iter().map(|n| row_for(n, &fields)).collect();
    emit_rows(&rows, &fields, ctx.fmt);
    Ok(())
}

/// Python slicing: `notes[offset:][:limit]`, negative values included.
fn paginate(notes: &[FakeNote], offset: i64, limit: Option<i64>) -> Vec<FakeNote> {
    let len = notes.len() as i64;
    let start = if offset < 0 {
        (len + offset).max(0)
    } else {
        offset.min(len)
    } as usize;
    let rest = &notes[start..];
    match limit {
        None => rest.to_vec(),
        Some(limit) => {
            let n = rest.len() as i64;
            let end = if limit < 0 {
                (n + limit).max(0)
            } else {
                limit.min(n)
            } as usize;
            rest[..end].to_vec()
        }
    }
}

fn matches_query(note: &FakeNote, query: &str) -> bool {
    let (todos, _) = todo_counts(&note.content);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    for term in query.split_whitespace() {
        let negate = term.starts_with('-');
        let term = if negate { &term[1..] } else { term };
        let ok = if term.starts_with('#') {
            note_has_tag(note, term)
        } else if term == "@todo" {
            todos > 0
        } else if term == "@untagged" {
            note.tags.is_empty()
        } else if term == "@tagged" {
            !note.tags.is_empty()
        } else if term == "@today" {
            note.modified.starts_with(&today)
        } else if term == "@pinned" {
            note.pins.iter().any(|p| p == "global")
        } else if term.starts_with('@') {
            true
        } else {
            note.content
                .to_lowercase()
                .contains(&term.trim_matches('"').to_lowercase())
        };
        if ok == negate {
            return false;
        }
    }
    true
}

fn cmd_search(
    ctx: &Ctx,
    state: &State,
    query_arg: Option<&str>,
    query: Option<&str>,
    listing: &Listing,
) -> CmdResult {
    let query = query.or(query_arg).unwrap_or("");
    let mut notes: Vec<FakeNote> = state
        .notes
        .iter()
        .filter(|n| in_location(n, &listing.location) && matches_query(n, query))
        .cloned()
        .collect();
    sort_notes(&mut notes, &listing.sort);
    if listing.count {
        if ctx.fmt == "json" {
            emit_json(&json!({"count": notes.len()}));
        } else {
            println!("{}", notes.len());
        }
        return Ok(());
    }
    let fields = parse_fields(
        ctx.fields,
        &["id", "title", "tags", "length", "matches"],
        &["matches"],
    );
    let page = paginate(&notes, listing.offset, listing.limit);
    let rows: Vec<Value> = page.iter().map(|n| row_for(n, &fields)).collect();
    emit_rows(&rows, &fields, ctx.fmt);
    Ok(())
}

fn cmd_cat(ctx: &Ctx, state: &State, target: &Target) -> CmdResult {
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail(ctx.fmt, "not_found", "Note not found"));
    };
    let note = &state.notes[i];
    if note.locked {
        return Err(fail(
            ctx.fmt,
            "locked",
            "Note is locked; content unavailable",
        ));
    }
    if ctx.fmt == "json" {
        emit_json(&json!({"content": note.content, "hash": content_hash(&note.content)}));
    } else {
        print!("{}", note.content);
    }
    Ok(())
}

fn cmd_show(ctx: &Ctx, state: &State, target: &Target) -> CmdResult {
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail(ctx.fmt, "not_found", "Note not found"));
    };
    let fields = parse_fields(ctx.fields, &["id", "title", "tags"], &[]);
    let row = row_for(&state.notes[i], &fields);
    if ctx.fmt == "json" {
        emit_json(&row);
    } else {
        let values: Vec<Value> = fields
            .iter()
            .map(|f| row.get(f).cloned().unwrap_or(Value::Null))
            .collect();
        println!("{}", tsv_row(values.iter()));
    }
    Ok(())
}

fn cmd_tags(ctx: &Ctx, state: &mut State, cmd: Option<TagsCmd>) -> CmdResult {
    let (action, note_id, tag_names) = match cmd {
        None => ("list", None, Vec::new()),
        Some(TagsCmd::List { note_id, tag_names }) => ("list", note_id, tag_names),
        Some(TagsCmd::Add { note_id, tag_names }) => ("add", note_id, tag_names),
        Some(TagsCmd::Remove { note_id, tag_names }) => ("remove", note_id, tag_names),
    };
    if action == "list" {
        let tags: Vec<String> = match note_id.as_deref() {
            Some(id) => {
                let Some(i) = find_note(state, Some(id), None) else {
                    return Err(fail(ctx.fmt, "not_found", "Note not found"));
                };
                display_tags(&state.notes[i])
            }
            None => {
                let mut tags: Vec<String> = state
                    .notes
                    .iter()
                    .filter(|n| in_location(n, "notes"))
                    .flat_map(display_tags)
                    .collect();
                tags.sort();
                tags.dedup();
                tags.sort_by_cached_key(|t| t.to_lowercase());
                tags
            }
        };
        if ctx.fmt == "json" {
            let rows: Vec<Value> = tags.iter().map(|t| json!({"tag": t})).collect();
            emit_json(&Value::Array(rows));
        } else {
            println!("{}", tags.join("\n"));
        }
        return Ok(());
    }
    let Some(i) = find_note(state, note_id.as_deref(), None) else {
        return Err(fail_text("Note not found", 1));
    };
    let wanted: Vec<String> = tag_names.iter().map(|t| strip_tag(t)).collect();
    let note = &mut state.notes[i];
    if action == "add" {
        for t in &wanted {
            with_ancestors(&mut note.tags, t);
        }
        let line = wanted
            .iter()
            .map(|t| display_tag(t))
            .collect::<Vec<_>>()
            .join(" ");
        let mut lines: Vec<String> = note.content.lines().map(str::to_string).collect();
        let at = if lines.first().is_some_and(|l| l.starts_with("# ")) {
            1
        } else {
            0
        };
        lines.insert(at.min(lines.len()), line);
        note.content = format!("{}\n", lines.join("\n"));
    } else {
        note.tags.retain(|t| {
            !wanted
                .iter()
                .any(|w| t == w || t.starts_with(&format!("{w}/")))
        });
    }
    note.modified = now_iso();
    save_state(state);
    Ok(())
}

fn cmd_create(
    ctx: &Ctx,
    state: &mut State,
    title: Option<&str>,
    content: Option<&str>,
    tags: Option<&str>,
    if_not_exists: bool,
) -> CmdResult {
    let content = match content {
        Some(c) => unescape(c),
        None => read_stdin(),
    };
    let title = match title {
        Some(t) => t.to_string(),
        None => content
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("Untitled")
            .trim_start_matches(['#', ' '])
            .trim()
            .to_string(),
    };
    if if_not_exists && let Some(i) = find_note(state, None, Some(&title)) {
        let fields = parse_fields(ctx.fields, &["id", "title", "tags"], &[]);
        emit_rows(&[row_for(&state.notes[i], &fields)], &fields, ctx.fmt);
        return Ok(());
    }
    let mut all_tags: Vec<String> = Vec::new();
    for t in tags.unwrap_or("").split(',') {
        let t = strip_tag(t);
        if !t.is_empty() {
            with_ancestors(&mut all_tags, &t);
        }
    }
    let heading = format!("# {title}");
    let body = match content.strip_prefix(heading.as_str()) {
        Some(rest) => rest.trim_start_matches('\n').to_string(),
        None => content.clone(),
    };
    let leaf_tags: Vec<&String> = all_tags
        .iter()
        .filter(|t| {
            !all_tags
                .iter()
                .any(|o| o != *t && o.starts_with(&format!("{t}/")))
        })
        .collect();
    let tag_line = leaf_tags
        .iter()
        .map(|t| display_tag(t))
        .collect::<Vec<_>>()
        .join(" ");
    let mut full = format!("{heading}\n");
    if tag_line.is_empty() {
        full.push('\n');
    } else {
        full.push_str(&format!("{tag_line}\n\n"));
    }
    full.push_str(&body);
    if !full.ends_with('\n') {
        full.push('\n');
    }
    let next = if state.next_id == 0 { 1 } else { state.next_id };
    let nid = format!("NEW-{next:04}");
    state.next_id = next + 1;
    let stamp = now_iso();
    let new_note = FakeNote {
        id: nid,
        title,
        tags: all_tags,
        locked: false,
        pins: Vec::new(),
        location: "notes".into(),
        created: stamp.clone(),
        modified: stamp,
        content: full,
        attachments: Vec::new(),
        attachment_data: None,
        extra: Map::new(),
    };
    state.notes.insert(0, new_note);
    save_state(state);
    let fields = parse_fields(ctx.fields, &["id", "title", "tags"], &[]);
    let row = row_for(&state.notes[0], &fields);
    if ctx.fmt == "json" {
        emit_json(&row);
    } else {
        let values: Vec<Value> = fields
            .iter()
            .map(|f| row.get(f).cloned().unwrap_or(Value::Null))
            .collect();
        println!("{}", tsv_row(values.iter()));
    }
    Ok(())
}

fn cmd_overwrite(
    state: &mut State,
    target: &Target,
    content: Option<&str>,
    base: Option<&str>,
    no_update_modified: bool,
) -> CmdResult {
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail_text("Note not found", 1));
    };
    let content = match content {
        Some(c) => unescape(c),
        None => read_stdin(),
    };
    let note = &mut state.notes[i];
    if let Some(base) = base.filter(|b| !b.is_empty())
        && base != content_hash(&note.content)
    {
        return Err(fail_text(
            "Note has changed since last read. Read it again before writing.",
            1,
        ));
    }
    note.content = content.clone();
    if let Some(first) = content.lines().find(|l| l.starts_with("# ")) {
        note.title = first[2..].trim().to_string();
    }
    let mut tags: Vec<String> = Vec::new();
    for caps in TAG_RE.captures_iter(&content).flatten() {
        let t = caps
            .get(1)
            .or_else(|| caps.get(2))
            .map(|m| m.as_str().trim())
            .unwrap_or("");
        if !t.is_empty() {
            with_ancestors(&mut tags, t);
        }
    }
    note.tags = tags;
    if !no_update_modified {
        note.modified = now_iso();
    }
    save_state(state);
    Ok(())
}

fn cmd_edit(
    state: &mut State,
    target: &Target,
    section: Option<&str>,
    find: &str,
    replace: Option<&str>,
    delete: bool,
    all: bool,
) -> CmdResult {
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail_text("Note not found", 1));
    };
    let find = unescape(find);
    let content = state.notes[i].content.clone();
    let (mut start, mut end) = (0usize, content.len());
    if let Some(section) = section {
        let heading = unescape(section).trim().to_string();
        let lines: Vec<&str> = content.split('\n').collect();
        let level = heading.len() - heading.trim_start_matches('#').len();
        let starts: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim() == heading)
            .map(|(i, _)| i)
            .collect();
        if starts.len() != 1 {
            return Err(fail_text(
                if starts.is_empty() {
                    "Section not found"
                } else {
                    "Section address is ambiguous"
                },
                1,
            ));
        }
        let first = starts[0];
        let mut last = lines.len();
        for (j, line) in lines.iter().enumerate().skip(first + 1) {
            if let Some(m) = HEADING_RE.captures(line)
                && m[1].len() <= level
            {
                last = j;
                break;
            }
        }
        start = lines[..first].join("\n").len() + usize::from(first > 0);
        end = lines[..last].join("\n").len();
    }
    let region = &content[start..end];
    let hits = region.matches(find.as_str()).count();
    if hits == 0 {
        return Err(fail_text("Find text not found", 1));
    }
    if hits > 1 && !all {
        return Err(fail_text("Find text matches more than once", 1));
    }
    let repl = match (replace, delete) {
        (Some(r), _) => unescape(r),
        (None, true) => String::new(),
        (None, false) => return Err(fail_text("edit needs --replace or --delete", 1)),
    };
    let region = if all {
        region.replace(find.as_str(), &repl)
    } else {
        region.replacen(find.as_str(), &repl, 1)
    };
    let note = &mut state.notes[i];
    note.content = format!("{}{}{}", &content[..start], region, &content[end..]);
    note.modified = now_iso();
    save_state(state);
    Ok(())
}

fn cmd_move(state: &mut State, target: &Target, location: &str) -> CmdResult {
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail_text("Note not found", 1));
    };
    state.notes[i].location = location.to_string();
    save_state(state);
    Ok(())
}

fn cmd_pin(ctx: &Ctx, state: &mut State, cmd: Option<PinCmd>) -> CmdResult {
    let (action, note_id, targets) = match cmd {
        None => ("list", None, Vec::new()),
        Some(PinCmd::List { note_id, targets }) => ("list", note_id, targets),
        Some(PinCmd::Add { note_id, targets }) => ("add", note_id, targets),
        Some(PinCmd::Remove { note_id, targets }) => ("remove", note_id, targets),
    };
    if action == "list" {
        let pins: Vec<String> = match note_id.as_deref() {
            Some(id) => {
                let Some(i) = find_note(state, Some(id), None) else {
                    return Err(fail(ctx.fmt, "not_found", "Note not found"));
                };
                state.notes[i].pins.clone()
            }
            None => {
                let mut pins: Vec<String> = state
                    .notes
                    .iter()
                    .flat_map(|n| n.pins.iter().cloned())
                    .collect();
                pins.sort();
                pins.dedup();
                pins
            }
        };
        if ctx.fmt == "json" {
            let rows: Vec<Value> = pins.iter().map(|p| json!({"pin": p})).collect();
            emit_json(&Value::Array(rows));
        } else {
            println!("{}", pins.join("\n"));
        }
        return Ok(());
    }
    let Some(i) = find_note(state, note_id.as_deref(), None) else {
        return Err(fail_text("Note not found", 1));
    };
    let note = &mut state.notes[i];
    for target in &targets {
        let label = if target == "global" {
            "global".to_string()
        } else {
            format!("#{}", target.trim_matches('#'))
        };
        if action == "add" && !note.pins.contains(&label) {
            note.pins.push(label.clone());
        }
        if action == "remove" {
            note.pins.retain(|p| *p != label);
        }
    }
    save_state(state);
    Ok(())
}

fn cmd_attachments(ctx: &Ctx, state: &State, cmd: Option<AttCmd>) -> CmdResult {
    let (target, filename) = match &cmd {
        None => return Err(fail(ctx.fmt, "not_found", "Note not found")),
        Some(AttCmd::List { target }) => (target, None),
        Some(AttCmd::Save { target, filename }) => (target, Some(filename.as_str())),
    };
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail(ctx.fmt, "not_found", "Note not found"));
    };
    let note = &state.notes[i];
    let data = note.attachment_data.clone().unwrap_or_default();
    let engine = base64::engine::general_purpose::STANDARD;
    match filename {
        None => {
            let fields = parse_fields(ctx.fields, &["filename", "size"], &[]);
            let rows: Vec<Value> = note
                .attachments
                .iter()
                .map(|name| {
                    let size = data
                        .get(name)
                        .and_then(|b| engine.decode(b).ok())
                        .map(|b| b.len())
                        .unwrap_or(0);
                    let full = json!({"filename": name, "size": size});
                    let mut out = Map::new();
                    for f in &fields {
                        if let Some(v) = full.get(f) {
                            out.insert(f.clone(), v.clone());
                        }
                    }
                    Value::Object(out)
                })
                .collect();
            emit_rows(&rows, &fields, ctx.fmt);
            Ok(())
        }
        Some(name) => {
            if std::io::stdout().is_terminal() {
                return Err(fail_text(
                    "Refusing to write binary data to a terminal; redirect stdout.",
                    1,
                ));
            }
            let Some(encoded) = data
                .get(name)
                .filter(|_| note.attachments.iter().any(|a| a == name))
            else {
                return Err(fail_text("Attachment not found", 1));
            };
            let bytes = engine.decode(encoded).unwrap_or_default();
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&bytes);
            let _ = out.flush();
            Ok(())
        }
    }
}

fn cmd_app(state: &State, cmd: AppCmd) -> CmdResult {
    let AppCmd::Open { target, header, .. } = cmd;
    let Some(i) = find_note(state, target.note_id.as_deref(), target.title.as_deref()) else {
        return Err(fail_text("Note not found", 1));
    };
    let log = PathBuf::from(format!("{}.opened", state_path().display()));
    let line = json!({"id": state.notes[i].id, "header": header});
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(log)
        .expect("writable .opened log");
    let _ = writeln!(file, "{line}");
    Ok(())
}

/// Run the fake with `argv` (program name first) and return the exit code.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => {
            let code = if err.use_stderr() { 64 } else { 0 };
            let _ = err.print();
            return code;
        }
    };
    let Some(cmd) = cli.cmd else {
        let _ = <Cli as clap::CommandFactory>::command().print_help();
        return 64;
    };
    let ctx = Ctx {
        fmt: &cli.format,
        fields: cli.fields.as_deref(),
    };
    let mut state = load_state();
    let outcome = match cmd {
        Cmd::List { listing, tag } => cmd_list(&ctx, &state, &listing, tag.as_deref()),
        Cmd::Search {
            query_arg,
            query,
            listing,
        } => cmd_search(
            &ctx,
            &state,
            query_arg.as_deref(),
            query.as_deref(),
            &listing,
        ),
        Cmd::Cat { target, .. } => cmd_cat(&ctx, &state, &target),
        Cmd::Show { target } => cmd_show(&ctx, &state, &target),
        Cmd::Tags { cmd } => cmd_tags(&ctx, &mut state, cmd),
        Cmd::Create {
            title,
            content,
            tags,
            if_not_exists,
        } => cmd_create(
            &ctx,
            &mut state,
            title.as_deref(),
            content.as_deref(),
            tags.as_deref(),
            if_not_exists,
        ),
        Cmd::Overwrite {
            target,
            content,
            base,
            no_update_modified,
            ..
        } => cmd_overwrite(
            &mut state,
            &target,
            content.as_deref(),
            base.as_deref(),
            no_update_modified,
        ),
        Cmd::Edit {
            target,
            section,
            find,
            replace,
            delete,
            all,
            ..
        } => cmd_edit(
            &mut state,
            &target,
            section.as_deref(),
            &find,
            replace.as_deref(),
            delete,
            all,
        ),
        Cmd::Trash { target } => cmd_move(&mut state, &target, "trash"),
        Cmd::Archive { target } => cmd_move(&mut state, &target, "archive"),
        Cmd::Restore { target } => cmd_move(&mut state, &target, "notes"),
        Cmd::Pin { cmd } => cmd_pin(&ctx, &mut state, cmd),
        Cmd::Attachments { cmd } => cmd_attachments(&ctx, &state, cmd),
        Cmd::App { cmd } => cmd_app(&state, cmd),
    };
    match outcome {
        Ok(()) => 0,
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_engine_matches_python() {
        let n = note(
            "X",
            "T",
            &["work", "work/sprint"],
            &["global"],
            "notes",
            "",
            "2026-09-11T10:00:00Z",
            "# T\n#work/sprint\n\n- [ ] open\nHello World\n".into(),
        );
        assert!(matches_query(&n, "@todo"));
        assert!(matches_query(&n, "hello"));
        assert!(matches_query(&n, "\"hello\" -goodbye"));
        assert!(!matches_query(&n, "-hello"));
        assert!(matches_query(&n, "#work"));
        assert!(matches_query(&n, "#*/sprint"));
        assert!(!matches_query(&n, "#*/work"));
        assert!(matches_query(&n, "@pinned @whatever"));
        assert!(!matches_query(&n, "@untagged"));
    }

    #[test]
    fn hashes_and_todos() {
        assert_eq!(content_hash("abc"), "a9993e3");
        assert_eq!(todo_counts("- [ ] a\n- [x] b\n  - [ ] c\n"), (2, 1));
    }

    #[test]
    fn sorting_pinned_then_modified_desc() {
        let mut notes = vec![
            note(
                "a",
                "A",
                &[],
                &[],
                "notes",
                "",
                "2026-01-01T00:00:00Z",
                String::new(),
            ),
            note(
                "b",
                "B",
                &[],
                &["global"],
                "notes",
                "",
                "2025-01-01T00:00:00Z",
                String::new(),
            ),
            note(
                "c",
                "C",
                &[],
                &[],
                "notes",
                "",
                "2026-06-01T00:00:00Z",
                String::new(),
            ),
        ];
        sort_notes(&mut notes, "pinned,modified");
        assert_eq!(
            notes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        sort_notes(&mut notes, "title");
        assert_eq!(
            notes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn overwrite_tag_regex_finds_tags_not_headings() {
        let content = "# Heading\n#work/sprint #multi word#\nbody with #inline and a # lone hash\n";
        let found: Vec<String> = TAG_RE
            .captures_iter(content)
            .flatten()
            .map(|c| c.get(1).or_else(|| c.get(2)).unwrap().as_str().to_string())
            .collect();
        // The Python regex stops a multi-word tag at the first space; the fake matches it.
        assert_eq!(found, vec!["work/sprint", "multi", "inline"]);
    }
}
