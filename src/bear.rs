//! Async wrapper around bearcli, and the note models it produces.
//!
//! bearcli reads and writes Bear's SQLite database directly, so everything here
//! works with Bear closed except `open_in_app`. Reads use `--format json`, where
//! every command emits one JSON document on stdout, errors included as
//! `{"error": {...}}`. Writes print nothing on success and a plain-text line on
//! stderr when they fail. Both shapes become `BearError`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, Utc};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::render::{PREVIEW_LIMIT, preview};
use crate::util::{first_line, home_dir, is_executable, which};

pub const ENV_COMMAND: &str = "BJORN_BEARCLI";
pub const DEFAULT_COMMAND: &str = "bearcli";

/// Where Bear ships bearcli. Used when nothing on PATH is called `bearcli`.
pub fn app_bundle_commands() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Bear.app/Contents/MacOS/bearcli"),
        home_dir().join("Applications/Bear.app/Contents/MacOS/bearcli"),
    ]
}

/// Every metadata field `list` can return. Content is fetched separately.
pub const LIST_FIELDS: &str =
    "id,title,locked,tags,length,created,modified,pins,location,todos,done,attachments";
/// When more notes than this need a fresh preview, one `list` with content is
/// cheaper than a `cat` apiece.
pub const PREVIEW_CAT_LIMIT: usize = 24;
/// Bumped when the on-disk preview cache's shape changes.
pub const PREVIEW_CACHE_VERSION: i64 = 1;
/// How many `cat` processes run at once while previews are refreshed.
pub const PREVIEW_CAT_CONCURRENCY: usize = 6;
/// bearcli stamps `modified` to the second, so a note edited twice within one
/// second keeps its stamp. Anything modified this recently is never trusted
/// from a stamp-keyed cache.
pub const RECENT_SECONDS: f64 = 2.0;
/// A bearcli call that runs longer than this is killed and reported. The
/// Python Bjorn has no timeout; a hung bearcli hangs it forever.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The stderr line bearcli prints when `--base` no longer matches.
const STALE_TEXT: &str = "has changed since last read";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BearError {
    pub message: String,
    pub code: String,
    pub exit_code: i32,
}

impl BearError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: String::new(),
            exit_code: 1,
        }
    }

    pub fn with_code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            exit_code: 1,
        }
    }

    pub fn is_conflict(&self) -> bool {
        self.code == "conflict"
    }
}

pub type Result<T> = std::result::Result<T, BearError>;

/// The bearcli to run: `$BJORN_BEARCLI`, else the config value, else PATH, else inside Bear.app.
pub fn resolve_bearcli(configured: &str) -> String {
    resolve_bearcli_with(
        configured,
        std::env::var(ENV_COMMAND).ok().as_deref(),
        &app_bundle_commands(),
        |name| which(name).is_some(),
    )
}

pub fn resolve_bearcli_with(
    configured: &str,
    env_value: Option<&str>,
    bundles: &[PathBuf],
    on_path: impl Fn(&str) -> bool,
) -> String {
    let explicit = env_value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(configured.trim());
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    if on_path(DEFAULT_COMMAND) {
        return DEFAULT_COMMAND.to_string();
    }
    for candidate in bundles {
        if is_executable(candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    DEFAULT_COMMAND.to_string()
}

pub fn bearcli_found(configured: &str) -> bool {
    which(&resolve_bearcli(configured)).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Location {
    #[default]
    Notes,
    Archive,
    Trash,
}

impl Location {
    pub fn as_str(self) -> &'static str {
        match self {
            Location::Notes => "notes",
            Location::Archive => "archive",
            Location::Trash => "trash",
        }
    }

    /// Unknown values are active notes, as in the Python client.
    pub fn parse(text: &str) -> Location {
        match text {
            "archive" => Location::Archive,
            "trash" => Location::Trash,
            _ => Location::Notes,
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `#work/CAD and Design#` -> `work/CAD and Design`.
///
/// Bear writes multi-word tags with a closing `#`; bearcli accepts either form
/// on input. Internally tags are stored bare.
pub fn normalize_tag(tag: &str) -> String {
    let mut tag = tag.trim();
    if let Some(rest) = tag.strip_prefix('#') {
        tag = rest;
    }
    if let Some(rest) = tag.strip_suffix('#') {
        tag = rest;
    }
    tag.trim().to_string()
}

/// Bare tag back to the form Bear shows, closing `#` for multi-word tags.
pub fn display_tag(tag: &str) -> String {
    let tag = normalize_tag(tag);
    if tag.contains(' ') {
        format!("#{tag}#")
    } else {
        format!("#{tag}")
    }
}

/// bearcli's ISO-8601 stamps; naive values are taken as UTC.
pub fn parse_time_str(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let normalized = text
        .strip_suffix('Z')
        .map(|t| format!("{t}+00:00"))
        .unwrap_or_else(|| text.to_string());
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(naive.and_utc());
        }
    }
    NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
}

fn parse_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value {
        Some(Value::String(s)) => parse_time_str(s),
        Some(Value::Number(_)) | Some(Value::Bool(_)) => {
            parse_time_str(&value.unwrap().to_string())
        }
        _ => None,
    }
}

/// Was `when` within RECENT_SECONDS of now? The stamp may still move.
pub fn recently_modified(when: Option<DateTime<Utc>>, now: Option<DateTime<Utc>>) -> bool {
    let Some(when) = when else { return false };
    let now = now.unwrap_or_else(Utc::now);
    let elapsed = (now - when).num_milliseconds() as f64 / 1000.0;
    elapsed < RECENT_SECONDS
}

pub fn recently_modified_str(stamp: &str) -> bool {
    recently_modified(parse_time_str(stamp), None)
}

/// bearcli's JSON writes `locked` as the strings "yes"/"no".
fn is_yes(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => matches!(s.trim().to_lowercase().as_str(), "yes" | "true" | "1"),
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Null) | None => false,
    }
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn int_of(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

/// One row of `bearcli list --fields all`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub length: i64,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
    pub pins: Vec<String>,
    pub location: Location,
    pub todos: i64,
    pub done: i64,
    pub attachments: i64,
    pub locked: bool,
    pub preview: String,
}

impl Note {
    /// A note from a `list` row; the preview comes from the row's content
    /// unless `preview_text` supplies one computed earlier.
    pub fn from_row(row: &Value, preview_text: Option<&str>) -> Note {
        let tags = row
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|t| text_of(Some(t)))
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| normalize_tag(&t))
                    .collect()
            })
            .unwrap_or_default();
        let pins = row
            .get("pins")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(|p| text_of(Some(p))).collect())
            .unwrap_or_default();
        let attachments = match row.get("attachments") {
            Some(Value::Array(items)) => items.len() as i64,
            other => int_of(other),
        };
        let title = text_of(row.get("title")).trim().to_string();
        Note {
            id: text_of(row.get("id")),
            title: if title.is_empty() {
                "Untitled".to_string()
            } else {
                title
            },
            tags,
            length: int_of(row.get("length")),
            created: parse_time(row.get("created")),
            modified: parse_time(row.get("modified")),
            pins,
            location: Location::parse(&text_of(row.get("location"))),
            todos: int_of(row.get("todos")),
            done: int_of(row.get("done")),
            attachments,
            locked: is_yes(row.get("locked")),
            preview: match preview_text {
                Some(text) => text.to_string(),
                None => preview(&text_of(row.get("content")), PREVIEW_LIMIT),
            },
        }
    }

    /// Any pin context, global or within a tag (ruling 2026-09-08).
    pub fn pinned(&self) -> bool {
        !self.pins.is_empty()
    }

    pub fn pinned_globally(&self) -> bool {
        self.pins.iter().any(|p| p == "global")
    }

    /// True for the tag itself or any nested child; bearcli lists ancestors too.
    pub fn has_tag(&self, tag: &str) -> bool {
        let tag = normalize_tag(tag);
        tag.is_empty() || self.tags.contains(&tag)
    }

    pub fn modified_local_date(&self) -> Option<NaiveDate> {
        self.modified.map(|m| m.with_timezone(&Local).date_naive())
    }
}

/// `bearcli cat --format json`: the body and the receipt for `overwrite --base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteContent {
    pub id: String,
    pub content: String,
    pub hash: String,
}

/// The cheap change signal: active-note count plus the newest modification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub count: i64,
    pub latest_id: String,
    pub latest_modified: String,
}

/// Everything `list --location all` returned, in one consistent read.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub notes: Vec<Note>,
    pub taken_at: Option<DateTime<Utc>>,
    /// Bodies this snapshot already had in hand, by note id. The listing that
    /// computes previews reads every note's content, so on a cold start the
    /// whole library comes back for free and the reader never has to `cat`
    /// again. Empty on the warm path, which lists metadata only. The stamp to
    /// key them by is the note's own, so take it from `notes`.
    pub bodies: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl Snapshot {
    pub fn new(notes: Vec<Note>) -> Snapshot {
        Snapshot::with_bodies(notes, Vec::new())
    }

    pub fn with_bodies(notes: Vec<Note>, bodies: Vec<(String, String)>) -> Snapshot {
        let index = notes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        Snapshot {
            notes,
            taken_at: Some(Utc::now()),
            bodies,
            index,
        }
    }

    pub fn by_id(&self, note_id: &str) -> Option<&Note> {
        self.index.get(note_id).map(|&i| &self.notes[i])
    }

    pub fn in_location(&self, location: Location) -> Vec<&Note> {
        self.notes
            .iter()
            .filter(|n| n.location == location)
            .collect()
    }
}

/// What one bearcli process produced.
#[derive(Debug, Clone, Default)]
pub struct RawOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Runs one bearcli invocation. The process runner is the real thing; tests
/// substitute a recorder that answers with canned rows.
pub trait Runner: Send + Sync {
    fn run<'a>(
        &'a self,
        args: &'a [String],
        stdin: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RawOutput>>;
    fn describe(&self) -> String;
}

/// Spawns the configured command with the given arguments.
pub struct ProcessRunner {
    pub command: Vec<String>,
    pub envs: Vec<(String, String)>,
}

impl Runner for ProcessRunner {
    fn run<'a>(
        &'a self,
        args: &'a [String],
        stdin: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RawOutput>> {
        Box::pin(async move {
            let (program, prefix) = self.command.split_first().expect("a command to run");
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(prefix)
                .args(args)
                .envs(self.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(if stdin.is_some() {
                    std::process::Stdio::piped()
                } else {
                    std::process::Stdio::null()
                })
                .kill_on_drop(true);
            let mut child = cmd.spawn().map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    BearError::with_code(
                        format!("bearcli not found ({program}). It ships inside Bear.app; see the README."),
                        "not_found",
                    )
                } else {
                    BearError::new(format!("could not run {program}: {err}"))
                }
            })?;
            let mut stdin_handle = child.stdin.take();
            let feed = async {
                if let (Some(mut handle), Some(body)) = (stdin_handle.take(), stdin) {
                    let _ = handle.write_all(body.as_bytes()).await;
                    let _ = handle.shutdown().await;
                }
            };
            let (_, waited) = tokio::join!(
                feed,
                tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
            );
            let output = match waited {
                Ok(Ok(output)) => output,
                Ok(Err(err)) => {
                    return Err(BearError::new(format!("bearcli failed to run: {err}")));
                }
                Err(_) => {
                    return Err(BearError::with_code(
                        format!(
                            "bearcli did not answer within {} s",
                            COMMAND_TIMEOUT.as_secs()
                        ),
                        "timeout",
                    ));
                }
            };
            Ok(RawOutput {
                status: output.status.code().unwrap_or(1),
                stdout: output.stdout,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        })
    }

    fn describe(&self) -> String {
        self.command.join(" ")
    }
}

/// Shells out to bearcli. One instance per app; writes are serialized.
pub struct BearClient {
    runner: Box<dyn Runner>,
    write_lock: tokio::sync::Mutex<()>,
    /// note id -> (modification stamp, preview). Bodies make up nine tenths of
    /// a `list` with content, so a snapshot lists metadata only and fetches
    /// bodies just for the notes whose stamp moved.
    previews: Mutex<HashMap<String, (String, String)>>,
    /// Where the previews are kept between runs, when the caller asked for it.
    /// Without this every launch is a cold start: the preview listing has to
    /// read every body, which is most of the second a cold start costs.
    preview_cache: Option<PathBuf>,
    /// What was last written, so an unchanged library is not rewritten on
    /// every poll.
    preview_cache_written: Mutex<Option<u64>>,
}

impl BearClient {
    pub fn new(command: Vec<String>) -> BearClient {
        Self::from_runner(Box::new(ProcessRunner {
            command,
            envs: Vec::new(),
        }))
    }

    /// A client whose bearcli runs with extra environment variables (the fakes
    /// take their state file from one).
    pub fn with_env(command: Vec<String>, envs: Vec<(String, String)>) -> BearClient {
        Self::from_runner(Box::new(ProcessRunner { command, envs }))
    }

    pub fn from_runner(runner: Box<dyn Runner>) -> BearClient {
        BearClient {
            runner,
            write_lock: tokio::sync::Mutex::new(()),
            previews: Mutex::new(HashMap::new()),
            preview_cache: None,
            preview_cache_written: Mutex::new(None),
        }
    }

    /// Keep previews in `path` between runs, and read whatever is there now.
    /// The file is a cache: anything unreadable, or from another bearcli, is
    /// ignored and overwritten.
    pub fn with_preview_cache(mut self, path: PathBuf) -> BearClient {
        self.preview_cache = Some(path.clone());
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return self;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
            return self;
        };
        if doc.get("version").and_then(Value::as_i64) != Some(PREVIEW_CACHE_VERSION)
            || text_of(doc.get("bearcli")) != self.describe()
        {
            return self;
        }
        if let Some(entries) = doc.get("previews").and_then(Value::as_object) {
            {
                let mut previews = self.previews.lock().unwrap();
                previews.reserve(entries.len());
                for (id, pair) in entries {
                    if let Some(pair) = pair.as_array()
                        && pair.len() == 2
                    {
                        previews.insert(
                            id.clone(),
                            (text_of(Some(&pair[0])), text_of(Some(&pair[1]))),
                        );
                    }
                }
            }
            *self.preview_cache_written.lock().unwrap() = Some(self.preview_signature());
        }
        self
    }

    /// A cheap signature of which notes are covered and at which stamp.
    fn preview_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        // Order-independent: the map's iteration order is not stable.
        let mut total: u64 = 0;
        for (id, (stamp, _)) in self.previews.lock().unwrap().iter() {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            id.hash(&mut hasher);
            stamp.hash(&mut hasher);
            total = total.wrapping_add(hasher.finish());
        }
        total
    }

    /// Is there anything new to write? Polling takes a snapshot every few
    /// seconds and an unchanged library must not rewrite the file each time.
    pub fn preview_cache_dirty(&self) -> bool {
        self.preview_cache.is_some()
            && *self.preview_cache_written.lock().unwrap() != Some(self.preview_signature())
    }

    /// Write the previews out, if a cache path was given. Called after a
    /// snapshot; a failure is not worth reporting, the next run just runs cold.
    pub fn save_preview_cache(&self) {
        let Some(path) = self.preview_cache.as_ref() else {
            return;
        };
        let signature = self.preview_signature();
        let entries: serde_json::Map<String, Value> = self
            .previews
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (stamp, text))| {
                (
                    id.clone(),
                    Value::Array(vec![
                        Value::String(stamp.clone()),
                        Value::String(text.clone()),
                    ]),
                )
            })
            .collect();
        let doc = serde_json::json!({
            "version": PREVIEW_CACHE_VERSION,
            "bearcli": self.describe(),
            "previews": Value::Object(entries),
        });
        let Ok(body) = serde_json::to_vec(&doc) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Written beside the target and renamed, so a killed process never
        // leaves half a cache behind.
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, &body).is_ok() && std::fs::rename(&temp, path).is_ok() {
            *self.preview_cache_written.lock().unwrap() = Some(signature);
        }
    }

    pub fn describe(&self) -> String {
        self.runner.describe()
    }

    /// Cached previews, for tests and diagnostics.
    pub fn preview_ids(&self) -> Vec<String> {
        self.previews.lock().unwrap().keys().cloned().collect()
    }

    async fn spawn(&self, args: &[&str], stdin: Option<&str>) -> Result<RawOutput> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.runner.run(&owned, stdin).await
    }

    /// Run bearcli once and return its JSON (`Value::Null` when it printed
    /// nothing or `parse` is off), raising `BearError` for either error shape.
    async fn run(&self, args: &[&str], parse: bool, stdin: Option<&str>) -> Result<Value> {
        let output = self.spawn(args, stdin).await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut payload = Value::Null;
        if parse && !stdout.trim().is_empty() {
            payload = serde_json::from_str(&stdout)
                .map_err(|err| BearError::new(format!("bearcli returned invalid JSON: {err}")))?;
            if let Some(err) = payload.as_object().and_then(|o| o.get("error")) {
                let message = text_of(err.get("message"));
                return Err(BearError {
                    message: if message.is_empty() {
                        "bearcli error".into()
                    } else {
                        message
                    },
                    code: text_of(err.get("code")),
                    exit_code: if output.status == 0 { 1 } else { output.status },
                });
            }
        }
        if output.status != 0 {
            let line = first_line(&output.stderr);
            return Err(BearError {
                message: if line.is_empty() {
                    "bearcli failed with no error output".into()
                } else {
                    line
                },
                code: if output.stderr.contains(STALE_TEXT) {
                    "conflict".into()
                } else {
                    String::new()
                },
                exit_code: output.status,
            });
        }
        Ok(payload)
    }

    fn rows(payload: Value) -> Vec<Value> {
        match payload {
            Value::Array(rows) => rows,
            _ => Vec::new(),
        }
    }

    // -- reads ---------------------------------------------------------------

    /// Every note's metadata, with a body preview each.
    ///
    /// The first call lists content too and remembers every preview. Later
    /// calls list metadata alone and read the body only of notes whose
    /// modification stamp changed: a `cat` each for a few, one content-bearing
    /// `list` for many.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let mut rows: Vec<Value> = Vec::new();
        let mut stale: Vec<Value> = Vec::new();
        let mut previews: HashMap<String, String> = HashMap::new();
        let mut bodies: Vec<(String, String)> = Vec::new();
        let warm = !self.previews.lock().unwrap().is_empty();
        if warm {
            rows = Self::rows(
                self.run(
                    &[
                        "list",
                        "--location",
                        "all",
                        "--format",
                        "json",
                        "--fields",
                        LIST_FIELDS,
                    ],
                    true,
                    None,
                )
                .await?,
            );
            for row in &rows {
                match self.preview_of(row) {
                    Some(known) => {
                        previews.insert(text_of(row.get("id")), known);
                    }
                    None => stale.push(row.clone()),
                }
            }
        }
        if !warm || stale.len() > PREVIEW_CAT_LIMIT {
            let fields = format!("{LIST_FIELDS},content");
            rows = Self::rows(
                self.run(
                    &[
                        "list",
                        "--location",
                        "all",
                        "--format",
                        "json",
                        "--fields",
                        &fields,
                    ],
                    true,
                    None,
                )
                .await?,
            );
            for row in &rows {
                let body = text_of(row.get("content"));
                let text = preview(&body, PREVIEW_LIMIT);
                let id = text_of(row.get("id"));
                if !recently_modified_str(&Self::stamp(row)) {
                    bodies.push((id.clone(), body));
                }
                previews.insert(id, self.remember_preview(row, text));
            }
        } else if !stale.is_empty() {
            let semaphore = Arc::new(Semaphore::new(PREVIEW_CAT_CONCURRENCY));
            let reads = stale.iter().map(|row| {
                let semaphore = semaphore.clone();
                async move {
                    let _permit = semaphore.acquire().await;
                    let id = text_of(row.get("id"));
                    let body = match self.cat(&id).await {
                        Ok(content) => content.content,
                        Err(_) => String::new(), // locked, or gone since the list
                    };
                    (row, preview(&body, PREVIEW_LIMIT), body)
                }
            });
            for (row, text, body) in futures::future::join_all(reads).await {
                let id = text_of(row.get("id"));
                if !body.is_empty() && !recently_modified_str(&Self::stamp(row)) {
                    bodies.push((id.clone(), body));
                }
                previews.insert(id, self.remember_preview(row, text));
            }
        }
        let notes: Vec<Note> = rows
            .iter()
            .map(|row| {
                let id = text_of(row.get("id"));
                Note::from_row(
                    row,
                    Some(previews.get(&id).map(String::as_str).unwrap_or("")),
                )
            })
            .collect();
        let live: std::collections::HashSet<&str> = notes.iter().map(|n| n.id.as_str()).collect();
        self.previews
            .lock()
            .unwrap()
            .retain(|id, _| live.contains(id.as_str()));
        Ok(Snapshot::with_bodies(notes, bodies))
    }

    fn stamp(row: &Value) -> String {
        text_of(row.get("modified"))
    }

    fn preview_of(&self, row: &Value) -> Option<String> {
        let stamp = Self::stamp(row);
        if recently_modified_str(&stamp) {
            return None;
        }
        let previews = self.previews.lock().unwrap();
        previews
            .get(&text_of(row.get("id")))
            .filter(|(known, _)| *known == stamp)
            .map(|(_, text)| text.clone())
    }

    fn remember_preview(&self, row: &Value, text: String) -> String {
        self.previews
            .lock()
            .unwrap()
            .insert(text_of(row.get("id")), (Self::stamp(row), text.clone()));
        text
    }

    /// Two ~20 ms bearcli calls, run together: enough to know whether a full
    /// `snapshot` is worth taking.
    pub async fn probe(&self) -> Result<Probe> {
        let (count_rows, latest) = tokio::join!(
            self.run(
                &["list", "--location", "all", "--count", "--format", "json"],
                true,
                None
            ),
            self.run(
                &[
                    "list",
                    "--location",
                    "all",
                    "--sort",
                    "modified:desc",
                    "-n",
                    "1",
                    "--format",
                    "json",
                    "--fields",
                    "id,modified"
                ],
                true,
                None,
            ),
        );
        let count = count_from(&count_rows?);
        let latest = latest?;
        let row = latest
            .as_array()
            .and_then(|rows| rows.first())
            .cloned()
            .unwrap_or(Value::Null);
        Ok(Probe {
            count,
            latest_id: text_of(row.get("id")),
            latest_modified: text_of(row.get("modified")),
        })
    }

    /// Ids matching a Bear search query, in bearcli's order.
    pub async fn search_ids(&self, query: &str, location: &str) -> Result<Vec<String>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self
            .run(
                &[
                    "search",
                    "--query",
                    query,
                    "--location",
                    location,
                    "--format",
                    "json",
                    "--fields",
                    "id",
                ],
                true,
                None,
            )
            .await?;
        Ok(Self::rows(rows)
            .iter()
            .map(|r| text_of(r.get("id")))
            .filter(|id| !id.is_empty())
            .collect())
    }

    pub async fn cat(&self, note_id: &str) -> Result<NoteContent> {
        let payload = self
            .run(&["cat", note_id, "--format", "json"], true, None)
            .await?;
        if !payload.is_object() {
            return Err(BearError::new("bearcli cat returned no content"));
        }
        Ok(NoteContent {
            id: note_id.to_string(),
            content: text_of(payload.get("content")),
            hash: text_of(payload.get("hash")),
        })
    }

    /// Raw rows for every active note with an open todo, optionally only under
    /// a tag. Content included; `todos::scan_rows` turns them into items.
    pub async fn todo_rows(&self, workspace: &str) -> Result<Vec<Value>> {
        let ws = normalize_tag(workspace);
        let query = if ws.is_empty() {
            "@todo".to_string()
        } else {
            format!("@todo #{ws}")
        };
        let rows = self
            .run(
                &[
                    "search",
                    "--query",
                    &query,
                    "--location",
                    "notes",
                    "--format",
                    "json",
                    "--fields",
                    "id,title,tags,locked,content",
                ],
                true,
                None,
            )
            .await?;
        Ok(Self::rows(rows))
    }

    /// The note's attachment filenames, as they appear in its markdown links
    /// (before percent-encoding).
    pub async fn attachments(&self, note_id: &str) -> Result<Vec<String>> {
        let rows = self
            .run(
                &[
                    "attachments",
                    "list",
                    note_id,
                    "--format",
                    "json",
                    "--fields",
                    "filename",
                ],
                true,
                None,
            )
            .await?;
        Ok(Self::rows(rows)
            .iter()
            .filter(|r| r.is_object())
            .map(|r| text_of(r.get("filename")))
            .filter(|name| !name.is_empty())
            .collect())
    }

    /// One attachment's bytes. bearcli refuses a TTY for this; stdout is a pipe
    /// here, so it always answers.
    pub async fn attachment(&self, note_id: &str, filename: &str) -> Result<Vec<u8>> {
        let output = self
            .spawn(
                &["attachments", "save", note_id, "--filename", filename],
                None,
            )
            .await?;
        if output.status != 0 {
            let line = first_line(&output.stderr);
            return Err(BearError {
                message: if line.is_empty() {
                    format!("bearcli could not save {filename}")
                } else {
                    line
                },
                code: String::new(),
                exit_code: output.status,
            });
        }
        Ok(output.stdout)
    }

    pub async fn tags(&self) -> Result<Vec<String>> {
        let rows = self
            .run(&["tags", "list", "--format", "json"], true, None)
            .await?;
        Ok(Self::rows(rows)
            .iter()
            .map(|r| text_of(r.get("tag")))
            .filter(|t| !t.is_empty())
            .map(|t| normalize_tag(&t))
            .collect())
    }

    // -- writes --------------------------------------------------------------

    /// Create a note and return its id.
    pub async fn create(&self, title: &str, tags: &[String], content: &str) -> Result<String> {
        let mut args: Vec<String> = vec![
            "create".into(),
            title.into(),
            "--format".into(),
            "json".into(),
            "--fields".into(),
            "id".into(),
        ];
        let tag_list = tags
            .iter()
            .map(|t| normalize_tag(t))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        if !tag_list.is_empty() {
            args.push("--tags".into());
            args.push(tag_list);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let payload = {
            let _guard = self.write_lock.lock().await;
            self.run(&refs, true, Some(content)).await?
        };
        let payload = match payload {
            Value::Array(rows) => rows.into_iter().next().unwrap_or(Value::Null),
            other => other,
        };
        let id = text_of(payload.get("id"));
        if !payload.is_object() || id.is_empty() {
            return Err(BearError::new("bearcli create returned no id"));
        }
        Ok(id)
    }

    /// Replace a note's whole content, guarded by the hash from `cat`.
    pub async fn overwrite(&self, note_id: &str, content: &str, base: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(
            &["overwrite", note_id, "--base", base],
            false,
            Some(content),
        )
        .await
        .map(|_| ())
    }

    pub async fn trash(&self, note_id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(&["trash", note_id], false, None).await.map(|_| ())
    }

    pub async fn restore(&self, note_id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(&["restore", note_id], false, None)
            .await
            .map(|_| ())
    }

    pub async fn archive(&self, note_id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(&["archive", note_id], false, None)
            .await
            .map(|_| ())
    }

    pub async fn pin(&self, note_id: &str, target: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(&["pin", "add", note_id, target], false, None)
            .await
            .map(|_| ())
    }

    pub async fn unpin(&self, note_id: &str, target: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.run(&["pin", "remove", note_id, target], false, None)
            .await
            .map(|_| ())
    }

    /// One exact find/replace, the primitive todo triage uses.
    pub async fn edit(
        &self,
        note_id: &str,
        find: &str,
        replace: &str,
        section: &str,
    ) -> Result<()> {
        let mut args: Vec<String> = vec!["edit".into(), note_id.into()];
        if !section.is_empty() {
            args.push("--section".into());
            args.push(escape_flag(section));
        }
        args.push("--find".into());
        args.push(escape_flag(find));
        args.push("--replace".into());
        args.push(escape_flag(replace));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let _guard = self.write_lock.lock().await;
        self.run(&refs, false, None).await.map(|_| ())
    }

    /// Flip one `[ ]` to `[x]`, matching the whole line.
    ///
    /// `--find` is a substring match, so a bare line would also hit a longer
    /// line that starts the same way. The note is read first: the line must
    /// still be present as a complete line, and the newline that ends it pins
    /// the match (the last line of a note has none).
    pub async fn tick_todo(
        &self,
        note_id: &str,
        line: &str,
        done_line: &str,
        section: &str,
    ) -> Result<()> {
        let current = self.cat(note_id).await?;
        let lines: Vec<&str> = current.content.split('\n').collect();
        if !lines.contains(&line) {
            let shown: String = line.trim().chars().take(60).collect();
            return Err(BearError::with_code(
                format!("The line is no longer in the note: {shown}"),
                "conflict",
            ));
        }
        let occurrences = lines.iter().filter(|l| **l == line).count();
        if lines.last() == Some(&line) && occurrences == 1 {
            self.edit(note_id, line, done_line, section).await
        } else {
            self.edit(
                note_id,
                &format!("{line}\n"),
                &format!("{done_line}\n"),
                section,
            )
            .await
        }
    }

    // -- app -----------------------------------------------------------------

    pub async fn open_in_app(&self, note_id: &str, header: &str) -> Result<()> {
        let mut args = vec!["app", "open", note_id];
        if !header.is_empty() {
            args.push("--header");
            args.push(header);
        }
        self.run(&args, false, None).await.map(|_| ())
    }
}

/// Text a bearcli text flag interprets: `\n`, `\t`, `\r`, `\\`.
pub fn escape_flag(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// `list --count --format json` returns `{"count": N}`; accept older shapes too.
pub fn count_from(payload: &Value) -> i64 {
    match payload {
        Value::Object(map) => ["count", "total"]
            .iter()
            .find_map(|k| map.get(*k))
            .map(|v| int_of(Some(v)))
            .unwrap_or(0),
        Value::Array(items) => items.len() as i64,
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s)
            if s.trim().chars().all(|c| c.is_ascii_digit()) && !s.trim().is_empty() =>
        {
            s.trim().parse().unwrap_or(0)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_and_display_tag() {
        assert_eq!(
            normalize_tag("#work/CAD and Design#"),
            "work/CAD and Design"
        );
        assert_eq!(normalize_tag(" #tech "), "tech");
        assert_eq!(display_tag("work/CAD and Design"), "#work/CAD and Design#");
        assert_eq!(display_tag("tech/dev"), "#tech/dev");
    }

    #[test]
    fn note_from_row_parses_bearcli_shapes() {
        let row = json!({
            "id": "X", "title": "T", "locked": "no", "tags": ["#a", "#a/b c#"], "length": 12,
            "created": "2026-09-01T13:10:12Z", "modified": "2026-09-08T13:33:55Z", "pins": ["#meetings"],
            "location": "notes", "todos": 1, "done": 0, "attachments": [],
        });
        let note = Note::from_row(&row, None);
        assert_eq!(note.tags, vec!["a", "a/b c"]);
        assert!(note.pinned() && !note.pinned_globally());
        assert_eq!(note.modified.unwrap().format("%Y").to_string(), "2026");
        assert!(note.has_tag("a") && note.has_tag("#a/b c#") && !note.has_tag("z"));
        assert_eq!(note.location, Location::Notes);
        assert!(!note.locked);
        assert_eq!(
            Note::from_row(&json!({"id": "Y", "title": "  "}), None).title,
            "Untitled"
        );
        assert_eq!(
            Note::from_row(&json!({"id": "Y", "attachments": 3, "locked": "yes"}), None)
                .attachments,
            3
        );
    }

    #[test]
    fn times_and_recency() {
        assert!(parse_time_str("2026-09-01T13:10:12Z").is_some());
        assert!(parse_time_str("2026-09-01T13:10:12").is_some());
        assert!(parse_time_str("2026-09-01").is_some());
        assert!(parse_time_str("nonsense").is_none());
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(recently_modified_str(&now));
        assert!(!recently_modified_str("2026-09-01T00:00:00Z"));
        assert!(!recently_modified(None, None));
    }

    #[test]
    fn escape_and_count() {
        assert_eq!(escape_flag("a\\b\nc\td\r"), "a\\\\b\\nc\\td\\r");
        assert_eq!(count_from(&json!({"count": 7})), 7);
        assert_eq!(count_from(&json!({"total": "3"})), 3);
        assert_eq!(count_from(&json!([1, 2])), 2);
        assert_eq!(count_from(&json!("12")), 12);
        assert_eq!(count_from(&json!("x")), 0);
    }

    #[test]
    fn resolve_bearcli_order() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("Bear.app/Contents/MacOS/bearcli");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            &bundled,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let bundles = vec![dir.path().join("missing"), bundled.clone()];
        let nothing = |_: &str| false;
        assert_eq!(
            resolve_bearcli_with("", None, &bundles, nothing),
            bundled.to_string_lossy()
        );
        assert_eq!(
            resolve_bearcli_with("/opt/bearcli", None, &bundles, nothing),
            "/opt/bearcli"
        );
        assert_eq!(
            resolve_bearcli_with("/opt/bearcli", Some("/env/bearcli"), &bundles, nothing),
            "/env/bearcli"
        );
        assert_eq!(resolve_bearcli_with("", None, &[], nothing), "bearcli");
        assert_eq!(resolve_bearcli_with("", None, &[], |_| true), "bearcli");
    }

    /// A bearcli made of canned rows; every call is logged.
    struct Recorder {
        rows: Mutex<Vec<Value>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl Recorder {
        fn new(rows: Vec<Value>) -> Arc<Recorder> {
            Arc::new(Recorder {
                rows: Mutex::new(rows),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn kinds(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| {
                    if call[0] == "cat" {
                        "cat".to_string()
                    } else {
                        let fields = &call[call.iter().position(|a| a == "--fields").unwrap() + 1];
                        if fields.contains("content") {
                            "list+content".into()
                        } else {
                            "list".into()
                        }
                    }
                })
                .collect()
        }

        fn last_call(&self) -> Vec<String> {
            self.calls.lock().unwrap().last().cloned().unwrap()
        }

        fn clear(&self) {
            self.calls.lock().unwrap().clear();
        }
    }

    struct RecorderHandle(Arc<Recorder>);

    impl Runner for RecorderHandle {
        fn run<'a>(
            &'a self,
            args: &'a [String],
            _stdin: Option<&'a str>,
        ) -> BoxFuture<'a, Result<RawOutput>> {
            Box::pin(async move {
                self.0.calls.lock().unwrap().push(args.to_vec());
                let rows = self.0.rows.lock().unwrap().clone();
                let payload = if args[0] == "cat" {
                    let row = rows.iter().find(|r| r["id"] == args[1]).unwrap();
                    json!({"content": row["content"], "hash": "h"})
                } else if args.iter().any(|a| a == "--count") {
                    json!({"count": rows.len()})
                } else {
                    let fields: Vec<&str> = args
                        [args.iter().position(|a| a == "--fields").unwrap() + 1]
                        .split(',')
                        .collect();
                    Value::Array(
                        rows.iter()
                            .map(|r| {
                                Value::Object(
                                    r.as_object()
                                        .unwrap()
                                        .iter()
                                        .filter(|(k, _)| fields.contains(&k.as_str()))
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect(),
                                )
                            })
                            .collect(),
                    )
                };
                Ok(RawOutput {
                    status: 0,
                    stdout: payload.to_string().into_bytes(),
                    stderr: String::new(),
                })
            })
        }

        fn describe(&self) -> String {
            "recorder".into()
        }
    }

    fn row(i: usize, stamp: &str) -> Value {
        json!({"id": format!("N{i}"), "title": format!("Note {i}"), "modified": stamp, "location": "notes",
               "content": format!("# Note {i}\n\nbody {i} at {stamp}\n")})
    }

    fn client(rec: &Arc<Recorder>) -> BearClient {
        BearClient::from_runner(Box::new(RecorderHandle(rec.clone())))
    }

    #[tokio::test]
    async fn snapshot_reads_bodies_only_for_notes_whose_stamp_moved() {
        let rec = Recorder::new((0..5).map(|i| row(i, "2026-09-01T00:00:00Z")).collect());
        let client = client(&rec);
        let first = client.snapshot().await.unwrap();
        assert_eq!(
            rec.kinds(),
            vec!["list+content"],
            "a cold snapshot lists content in one call"
        );
        assert_eq!(
            first.by_id("N3").unwrap().preview,
            "body 3 at 2026-09-01T00:00:00Z"
        );

        rec.clear();
        let second = client.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list"], "nothing changed: metadata only");
        assert_eq!(
            second.by_id("N3").unwrap().preview,
            first.by_id("N3").unwrap().preview
        );

        rec.rows.lock().unwrap()[3] = row(3, "2026-09-02T00:00:00Z");
        rec.clear();
        let third = client.snapshot().await.unwrap();
        let mut kinds = rec.kinds();
        kinds.sort();
        assert_eq!(kinds, vec!["cat", "list"], "one stamp moved: one cat");
        assert_eq!(rec.last_call()[1], "N3");
        assert_eq!(
            third.by_id("N3").unwrap().preview,
            "body 3 at 2026-09-02T00:00:00Z"
        );
        assert_eq!(
            third.by_id("N1").unwrap().preview,
            first.by_id("N1").unwrap().preview
        );
    }

    #[tokio::test]
    async fn snapshot_falls_back_to_one_content_list_when_many_notes_changed() {
        let rec = Recorder::new(
            (0..PREVIEW_CAT_LIMIT + 5)
                .map(|i| row(i, "2026-09-01T00:00:00Z"))
                .collect(),
        );
        let client = client(&rec);
        client.snapshot().await.unwrap();
        for i in 0..PREVIEW_CAT_LIMIT + 1 {
            rec.rows.lock().unwrap()[i] = row(i, "2026-09-03T00:00:00Z");
        }
        rec.clear();
        let snap = client.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list", "list+content"]);
        assert!(
            snap.by_id("N0")
                .unwrap()
                .preview
                .ends_with("2026-09-03T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn snapshot_forgets_previews_of_notes_that_are_gone() {
        let rec = Recorder::new((0..3).map(|i| row(i, "2026-09-01T00:00:00Z")).collect());
        let client = client(&rec);
        client.snapshot().await.unwrap();
        rec.rows.lock().unwrap().remove(0);
        client.snapshot().await.unwrap();
        let mut ids = client.preview_ids();
        ids.sort();
        assert_eq!(ids, vec!["N1", "N2"]);
    }

    #[tokio::test]
    async fn previews_survive_a_restart_through_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("previews.json");
        let rows: Vec<Value> = (0..3).map(|i| row(i, "2026-09-01T00:00:00Z")).collect();

        let rec = Recorder::new(rows.clone());
        let first = client(&rec).with_preview_cache(path.clone());
        assert!(first.preview_cache_dirty(), "nothing written yet");
        first.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list+content"], "a cold start reads bodies");
        first.save_preview_cache();
        assert!(!first.preview_cache_dirty(), "the file is up to date");

        // A second run, same library: the cache means metadata only.
        let rec = Recorder::new(rows);
        let restarted = client(&rec).with_preview_cache(path.clone());
        let snap = restarted.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list"]);
        assert!(snap.by_id("N0").unwrap().preview.contains("2026-09-01"));
        assert!(snap.bodies.is_empty(), "no bodies were read to get there");

        // Another bearcli's cache is not this one's.
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "version": PREVIEW_CACHE_VERSION,
                "bearcli": "somewhere/else/bearcli",
                "previews": {"N0": ["2026-09-01T00:00:00Z", "stale"]},
            }))
            .unwrap(),
        )
        .unwrap();
        let rec = Recorder::new((0..3).map(|i| row(i, "2026-09-01T00:00:00Z")).collect());
        let elsewhere = client(&rec).with_preview_cache(path.clone());
        elsewhere.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list+content"]);

        // So is a corrupt one.
        std::fs::write(&path, "{not json").unwrap();
        let rec = Recorder::new((0..3).map(|i| row(i, "2026-09-01T00:00:00Z")).collect());
        let broken = client(&rec).with_preview_cache(path);
        broken.snapshot().await.unwrap();
        assert_eq!(rec.kinds(), vec!["list+content"]);
    }

    #[tokio::test]
    async fn a_cold_snapshot_hands_back_the_bodies_it_read() {
        let rec = Recorder::new((0..3).map(|i| row(i, "2026-09-01T00:00:00Z")).collect());
        let snap = client(&rec).snapshot().await.unwrap();
        let mut ids: Vec<&str> = snap.bodies.iter().map(|(id, _)| id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["N0", "N1", "N2"]);
        assert!(snap.bodies.iter().all(|(_, body)| !body.is_empty()));
    }

    #[tokio::test]
    async fn probe_runs_its_two_commands() {
        let rec = Recorder::new(vec![row(0, "2026-09-01T00:00:00Z")]);
        let client = client(&rec);
        let probe = client.probe().await.unwrap();
        assert_eq!(probe.count, 1);
        assert_eq!(probe.latest_id, "N0");
        assert_eq!(rec.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_note_stamped_this_second_is_read_again_every_snapshot() {
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let rec = Recorder::new(vec![row(0, "2026-09-01T00:00:00Z"), row(1, &now)]);
        let client = client(&rec);
        client.snapshot().await.unwrap();
        rec.clear();
        client.snapshot().await.unwrap();
        let mut kinds = rec.kinds();
        kinds.sort();
        assert_eq!(kinds, vec!["cat", "list"]);
        assert_eq!(rec.last_call()[1], "N1");
    }
}
