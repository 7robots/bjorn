//! Craft as a Bjorn library: Craft documents as notes, folders as tags.
//!
//! `BearClient` hands every call to a `CraftStore` when `[craft] enabled` is
//! set, so the app works the same against either. The mapping:
//!
//! - A document is a note. Its text is `# Title` and its blocks as markdown
//!   (`doc`); an edit writes back only the blocks that changed.
//! - A document's folder is its tag, with its ancestors, so the sidebar is the
//!   folder tree. Unsorted documents have no tag. A new note goes in the
//!   folder named by its first tag, made when missing.
//! - Craft's trash is the Trash. The Archive is a folder named in the config
//!   (`archive_folder`), off when unset; Craft has none of its own.
//! - Pins live in Bjorn (`craft-pins.json`); Craft has no pins.
//!
//! Craft allows about 100 requests a minute, and reading a document is one
//! request, so nothing here reads the whole library at once. Titles, dates and
//! folders come from a few listing calls every `refresh_seconds`; bodies are
//! read when a note is opened and, in the background, a few a minute until
//! every one is cached. The cache is kept on disk, so only the first launch is
//! slow to fill in previews, todo counts and full-text search.

pub mod api;
pub mod doc;
pub mod fake;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Digest;

use crate::bear::{
    BearError, Location, Note, NoteContent, Probe, Result, Snapshot, display_tag, normalize_tag,
    parse_time_str, recently_modified,
};
use crate::render::{PREVIEW_LIMIT, preview};
use api::Transport;
use doc::{Links, Op, Rendered};

/// Bumped when the cache file's shape changes; an older one is ignored.
const CACHE_VERSION: u32 = 1;
/// Bumped when `doc::render` draws a document differently: cached bodies from
/// another version are read again, the rest of the cache is kept.
const RENDER_VERSION: u32 = 2;
/// Folder listings are re-read in full this often, to catch a move that left
/// every folder's count where it was.
const FULL_WALK_EVERY: Duration = Duration::from_secs(30 * 60);
/// Bodies read per background cycle at most; the throttle sets the pace.
const BODIES_PER_CYCLE: usize = 40;

/// `[craft]` from the config file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftConfig {
    pub enabled: bool,
    /// The API link (`https://connect.craft.do/links/…/api/v1`), or an
    /// `op://` reference to it.
    pub api_url: String,
    /// The link's API key, or an `op://` reference to it; empty for a link
    /// that needs none.
    pub api_key: String,
    /// The 1Password account `op read` uses for `op://` values.
    pub op_account: String,
    /// The folder that stands for Bear's Archive; empty for none.
    pub archive_folder: String,
    pub refresh_seconds: u64,
}

impl Default for CraftConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: String::new(),
            api_key: String::new(),
            op_account: String::new(),
            archive_folder: String::new(),
            refresh_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Doc {
    title: String,
    created: String,
    modified: String,
    trashed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Folder {
    id: String,
    path: String,
    parent: Option<String>,
    depth: usize,
    count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Body {
    /// The document's `lastModifiedAt` when this was read.
    stamp: String,
    content: String,
}

/// What is known about the space, kept on disk between runs.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    version: u32,
    #[serde(default)]
    render_version: u32,
    docs: HashMap<String, Doc>,
    folders: Vec<Folder>,
    /// Document id -> folder id. Missing: unsorted, or not walked yet.
    folder_of: HashMap<String, String>,
    /// Folder counts as of the last listing of each folder.
    walked: HashMap<String, i64>,
    /// Where a trashed or archived document lived, for `restore`.
    home: HashMap<String, String>,
    bodies: HashMap<String, Body>,
    /// Templates are not notes.
    #[serde(default)]
    templates: HashSet<String>,
    /// Read back just after Bjorn wrote them: current under whatever stamp
    /// the next listing gives them.
    #[serde(skip)]
    adopt: HashSet<String>,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    fresh: Vec<String>,
    #[serde(skip)]
    listed: bool,
    #[serde(skip)]
    last_full_walk: Option<Instant>,
    #[serde(skip)]
    saved_generation: u64,
}

impl State {
    fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    fn path_of_doc(&self, id: &str) -> String {
        self.folder_of
            .get(id)
            .and_then(|f| self.folder(f))
            .map(|f| f.path.clone())
            .unwrap_or_default()
    }

    /// Is `inner` the folder `outer` or inside it?
    fn within(&self, inner: &str, outer: &str) -> bool {
        let mut at = Some(inner.to_string());
        while let Some(id) = at {
            if id == outer {
                return true;
            }
            at = self.folder(&id).and_then(|f| f.parent.clone());
        }
        false
    }

    fn body(&self, id: &str) -> Option<&str> {
        let doc = self.docs.get(id)?;
        self.bodies
            .get(id)
            .filter(|b| b.stamp == doc.modified)
            .map(|b| b.content.as_str())
    }

    fn touch(&mut self) {
        self.generation += 1;
    }
}

fn hash(content: &str) -> String {
    sha1::Sha1::digest(content.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn items(payload: &Value) -> Vec<Value> {
    payload
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn conflict() -> BearError {
    BearError::with_code(
        "Note has changed since last read. Read it again before writing.",
        "conflict",
    )
}

/// Built-in locations in the folder list that are not folders.
const BUILT_IN: [&str; 3] = ["unsorted", "trash", "templates"];

/// The Craft space, as the library Bjorn shows.
pub struct CraftStore {
    api: Arc<dyn Transport>,
    background: Arc<dyn Transport>,
    state: Mutex<State>,
    pins: Mutex<HashMap<String, Vec<String>>>,
    cache_path: Option<PathBuf>,
    pins_path: Option<PathBuf>,
    archive: String,
    refresh_every: Duration,
    started: AtomicBool,
    write_lock: tokio::sync::Mutex<()>,
    /// One listing refresh at a time; a snapshot that finds one running waits.
    refresh_lock: tokio::sync::Mutex<()>,
    space_id: Mutex<Option<String>>,
}

impl CraftStore {
    pub fn new(
        api: Arc<dyn Transport>,
        background: Arc<dyn Transport>,
        config: &CraftConfig,
    ) -> CraftStore {
        CraftStore {
            api,
            background,
            state: Mutex::new(State {
                version: CACHE_VERSION,
                render_version: RENDER_VERSION,
                ..State::default()
            }),
            pins: Mutex::new(HashMap::new()),
            cache_path: None,
            pins_path: None,
            archive: normalize_tag(&config.archive_folder),
            refresh_every: Duration::from_secs(config.refresh_seconds.max(5)),
            started: AtomicBool::new(false),
            write_lock: tokio::sync::Mutex::new(()),
            refresh_lock: tokio::sync::Mutex::new(()),
            space_id: Mutex::new(None),
        }
    }

    /// The real API, from the config: secrets resolved, transports built.
    pub fn connect(config: &CraftConfig) -> std::result::Result<CraftStore, String> {
        let url = std::env::var("BJORN_CRAFT_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| config.api_url.clone());
        let key = std::env::var("BJORN_CRAFT_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| config.api_key.clone());
        if url.trim().is_empty() {
            return Err("[craft] is enabled but api_url is not set".into());
        }
        let url = api::resolve_secret(&url, &config.op_account)?;
        let key = api::resolve_secret(&key, &config.op_account)?;
        let http = Arc::new(api::Http::new(&url, &key).map_err(|e| e.to_string())?);
        let background = Arc::new(api::Background(http.clone()));
        // One cache per link, named by a hash so the link itself (a secret)
        // never lands in a file name.
        let tag = &hash(url.trim())[..12];
        let dir = crate::config::cache_dir();
        Ok(CraftStore::new(http, background, config).with_files(
            dir.join(format!("craft-{tag}.json")),
            crate::config::config_dir().join("craft-pins.json"),
        ))
    }

    /// Keep the cache and the pins in these files, and read what is there.
    pub fn with_files(mut self, cache: PathBuf, pins: PathBuf) -> CraftStore {
        if let Ok(raw) = std::fs::read(&cache)
            && let Ok(state) = serde_json::from_slice::<State>(&raw)
            && state.version == CACHE_VERSION
        {
            let mut state = state;
            if state.render_version != RENDER_VERSION {
                state.bodies.clear();
                state.render_version = RENDER_VERSION;
            }
            *self.state.lock().unwrap() = state;
        }
        if let Ok(raw) = std::fs::read(&pins)
            && let Ok(map) = serde_json::from_slice(&raw)
        {
            *self.pins.lock().unwrap() = map;
        }
        self.cache_path = Some(cache);
        self.pins_path = Some(pins);
        self
    }

    pub fn describe(&self) -> String {
        "craft".into()
    }

    // -- background ----------------------------------------------------------

    /// Start the refresh loop, once. Needs a Tokio runtime, hence not in `new`.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let store = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(this) = store.upgrade() else { return };
                let began = Instant::now();
                let bg = this.background.clone();
                let _ = this.refresh(bg.as_ref()).await;
                let _ = this.walk(bg.as_ref()).await;
                this.fill_bodies(bg.as_ref(), began + this.refresh_every)
                    .await;
                let rest = (began + this.refresh_every).saturating_duration_since(Instant::now());
                drop(this);
                tokio::time::sleep(rest.max(Duration::from_secs(1))).await;
            }
        });
    }

    /// Re-read the folder tree and every document's title and dates.
    async fn refresh(&self, t: &dyn Transport) -> Result<()> {
        let _one = self.refresh_lock.lock().await;
        let meta = [("fetchMetadata", "true".to_string())];
        let folders = t.request("GET", "/folders", &[], None).await?;
        let all = t.request("GET", "/documents", &meta, None).await?;
        let trash = t
            .request("GET", "/documents", &[("location", "trash".into())], None)
            .await?;
        let trashed: HashSet<String> = items(&trash).iter().map(|d| text(d.get("id"))).collect();
        let mut flat = Vec::new();
        flatten_folders(&items(&folders), None, "", 0, &mut flat);
        let mut state = self.state.lock().unwrap();
        let mut docs = HashMap::new();
        for row in items(&all) {
            let id = text(row.get("id"));
            if id.is_empty() {
                continue;
            }
            let modified = text(row.get("lastModifiedAt"));
            // A body read back after a write here is current: it takes the
            // stamp the write produced.
            if state.adopt.contains(&id)
                && let Some(body) = state.bodies.get_mut(&id)
            {
                body.stamp = modified.clone();
            }
            docs.insert(
                id.clone(),
                Doc {
                    title: text(row.get("title")),
                    created: text(row.get("createdAt")),
                    modified,
                    trashed: trashed.contains(&id),
                },
            );
        }
        let changed = docs.len() != state.docs.len()
            || docs.iter().any(|(id, d)| {
                state.docs.get(id).is_none_or(|o| {
                    o.modified != d.modified || o.title != d.title || o.trashed != d.trashed
                })
            })
            || flat.len() != state.folders.len()
            || flat
                .iter()
                .zip(&state.folders)
                .any(|(a, b)| a.id != b.id || a.path != b.path);
        state.adopt.clear();
        state.bodies.retain(|id, _| docs.contains_key(id));
        state.folder_of.retain(|id, _| docs.contains_key(id));
        state.docs = docs;
        state.folders = flat;
        state.listed = true;
        if changed {
            state.touch();
        }
        Ok(())
    }

    /// Learn which folder each document is in. Craft lists a folder's
    /// documents with its subfolders' included, so a document belongs to the
    /// deepest folder that lists it. Only folders whose count moved since they
    /// were last listed are listed again, deepest first, except every
    /// `FULL_WALK_EVERY`.
    async fn walk(&self, t: &dyn Transport) -> Result<()> {
        let (todo, full) = {
            let mut state = self.state.lock().unwrap();
            let full = state
                .last_full_walk
                .is_none_or(|at| at.elapsed() >= FULL_WALK_EVERY);
            if full {
                state.last_full_walk = Some(Instant::now());
            }
            let mut todo: Vec<Folder> = state
                .folders
                .iter()
                .filter(|f| f.id != "trash")
                .filter(|f| (full && f.count > 0) || state.walked.get(&f.id) != Some(&f.count))
                .cloned()
                .collect();
            todo.sort_by_key(|f| std::cmp::Reverse(f.depth));
            (todo, full)
        };
        if todo.is_empty() {
            return Ok(());
        }
        let listed: HashSet<String> = todo.iter().map(|f| f.id.clone()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        for folder in &todo {
            let query: Vec<(&str, String)> = if BUILT_IN.contains(&folder.id.as_str()) {
                vec![("location", folder.id.clone())]
            } else {
                vec![("folderId", folder.id.clone())]
            };
            let ids: Vec<String> = if folder.count == 0 && !BUILT_IN.contains(&folder.id.as_str()) {
                Vec::new()
            } else {
                items(&t.request("GET", "/documents", &query, None).await?)
                    .iter()
                    .map(|d| text(d.get("id")))
                    .collect()
            };
            let mut state = self.state.lock().unwrap();
            for id in ids {
                if !seen.insert(id.clone()) {
                    continue;
                }
                match folder.id.as_str() {
                    "templates" => {
                        state.templates.insert(id);
                    }
                    "unsorted" => {
                        state.folder_of.remove(&id);
                    }
                    _ => {
                        // Already placed deeper, in a folder this pass did not
                        // need to list: that is still where it is.
                        let deeper = state.folder_of.get(&id).is_some_and(|cur| {
                            cur != &folder.id
                                && !listed.contains(cur)
                                && state.within(cur, &folder.id)
                        });
                        if !deeper {
                            state.folder_of.insert(id, folder.id.clone());
                        }
                    }
                }
            }
            state.walked.insert(folder.id.clone(), folder.count);
            state.touch();
        }
        let mut state = self.state.lock().unwrap();
        // Placed in a folder that was listed without it: it moved somewhere
        // this pass did not see, which leaves Unsorted.
        let gone: Vec<String> = state
            .folder_of
            .iter()
            .filter(|(id, f)| listed.contains(*f) && !seen.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            state.folder_of.remove(&id);
        }
        if full {
            let templates: HashSet<String> = state.templates.intersection(&seen).cloned().collect();
            state.templates = templates;
        }
        let live: HashSet<String> = state.folders.iter().map(|f| f.id.clone()).collect();
        state.walked.retain(|id, _| live.contains(id));
        state.touch();
        Ok(())
    }

    /// Read bodies that are missing or out of date, newest first, until the
    /// next refresh is due.
    async fn fill_bodies(&self, t: &dyn Transport, until: Instant) {
        let wanted: Vec<String> = {
            let state = self.state.lock().unwrap();
            let mut ids: Vec<(&String, &Doc)> = state
                .docs
                .iter()
                .filter(|(id, d)| {
                    !d.trashed && state.body(id).is_none() && !state.templates.contains(*id)
                })
                .collect();
            ids.sort_by(|a, b| b.1.modified.cmp(&a.1.modified));
            ids.into_iter()
                .take(BODIES_PER_CYCLE)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in wanted {
            if Instant::now() >= until {
                return;
            }
            let _ = self.read(t, &id).await;
        }
    }

    // -- reading -------------------------------------------------------------

    fn links(&self) -> Links {
        let state = self.state.lock().unwrap();
        Links::new(
            state
                .docs
                .iter()
                .filter(|(_, d)| !d.trashed)
                .map(|(id, d)| (id.as_str(), d.title.as_str())),
        )
    }

    /// Fetch and render a document, and cache its body.
    async fn read(&self, t: &dyn Transport, id: &str) -> Result<Rendered> {
        let root = t
            .request("GET", "/blocks", &[("id", id.to_string())], None)
            .await?;
        if !root.is_object() {
            return Err(BearError::with_code("Note not found", "not_found"));
        }
        let rendered = doc::render(&root, &self.links());
        let mut state = self.state.lock().unwrap();
        if let Some(doc) = state.docs.get_mut(id) {
            doc.title = rendered.title.clone();
        }
        let stamp = state
            .docs
            .get(id)
            .map(|d| d.modified.clone())
            .unwrap_or_default();
        let changed = state
            .bodies
            .get(id)
            .is_none_or(|b| b.content != rendered.content);
        state.bodies.insert(
            id.to_string(),
            Body {
                stamp,
                content: rendered.content.clone(),
            },
        );
        if changed {
            state.fresh.push(id.to_string());
            state.touch();
        }
        Ok(rendered)
    }

    /// The first time this session, list the library before answering.
    async fn ensure_listed(&self) -> Result<()> {
        if self.state.lock().unwrap().listed {
            return Ok(());
        }
        self.refresh(self.api.as_ref()).await
    }

    pub async fn snapshot(self: &Arc<Self>) -> Result<Snapshot> {
        self.start();
        let cold = self.state.lock().unwrap().docs.is_empty();
        if cold {
            self.ensure_listed().await?;
        }
        let pins = self.pins.lock().unwrap().clone();
        let mut state = self.state.lock().unwrap();
        let mut notes: Vec<Note> = state
            .docs
            .iter()
            .filter(|(id, _)| !state.templates.contains(*id))
            .map(|(id, d)| self.note(&state, id, d, pins.get(id)))
            .collect();
        notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
        let fresh = std::mem::take(&mut state.fresh);
        let bodies = fresh
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|id| state.body(&id).map(|b| (id.clone(), b.to_string())))
            .collect();
        Ok(Snapshot::with_bodies(notes, bodies))
    }

    fn location(&self, state: &State, id: &str, d: &Doc) -> Location {
        if d.trashed {
            return Location::Trash;
        }
        let path = state.path_of_doc(id);
        if !self.archive.is_empty()
            && (path.eq_ignore_ascii_case(&self.archive)
                || path
                    .to_lowercase()
                    .starts_with(&format!("{}/", self.archive.to_lowercase())))
        {
            Location::Archive
        } else {
            Location::Notes
        }
    }

    fn tags(&self, state: &State, id: &str) -> Vec<String> {
        let path = state.path_of_doc(id);
        let mut tags = Vec::new();
        let mut at = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            tags.push(at.clone());
        }
        tags
    }

    fn note(&self, state: &State, id: &str, d: &Doc, pins: Option<&Vec<String>>) -> Note {
        let body = state.body(id).unwrap_or("");
        let (todos, done) = doc::todo_counts(body);
        Note {
            id: id.to_string(),
            title: if d.title.trim().is_empty() {
                "Untitled".into()
            } else {
                d.title.trim().to_string()
            },
            tags: self.tags(state, id),
            length: body.chars().count() as i64,
            created: parse_time_str(&d.created),
            modified: parse_time_str(&d.modified),
            pins: pins.cloned().unwrap_or_default(),
            location: self.location(state, id, d),
            todos,
            done,
            attachments: 0,
            locked: false,
            preview: preview(body, PREVIEW_LIMIT),
        }
    }

    pub async fn probe(self: &Arc<Self>) -> Result<Probe> {
        self.start();
        let state = self.state.lock().unwrap();
        let latest = state
            .docs
            .iter()
            .filter(|(_, d)| !d.trashed)
            .max_by(|a, b| a.1.modified.cmp(&b.1.modified));
        Ok(Probe {
            count: state.docs.values().filter(|d| !d.trashed).count() as i64,
            // Any change the background work makes (a folder learned, a body
            // read) is a reason to take a new snapshot.
            latest_id: format!(
                "{}#{}",
                latest.map(|(id, _)| id.as_str()).unwrap_or(""),
                state.generation
            ),
            latest_modified: latest.map(|(_, d)| d.modified.clone()).unwrap_or_default(),
        })
    }

    pub async fn cat(&self, id: &str) -> Result<NoteContent> {
        let cached = {
            let state = self.state.lock().unwrap();
            let recent = state
                .docs
                .get(id)
                .is_some_and(|d| recently_modified(parse_time_str(&d.modified), None));
            state.body(id).filter(|_| !recent).map(str::to_string)
        };
        let content = match cached {
            Some(content) => content,
            None => self.read(self.api.as_ref(), id).await?.content,
        };
        Ok(NoteContent {
            id: id.to_string(),
            hash: hash(&content),
            content,
        })
    }

    pub async fn title_and_location(&self, id: &str) -> Result<(String, String)> {
        let root = self
            .api
            .request(
                "GET",
                "/blocks",
                &[("id", id.to_string()), ("maxDepth", "0".into())],
                None,
            )
            .await?;
        let state = self.state.lock().unwrap();
        let location = state
            .docs
            .get(id)
            .map(|d| self.location(&state, id, d))
            .unwrap_or_default();
        Ok((text(root.get("markdown")), location.as_str().to_string()))
    }

    pub fn tag_list(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .folders
            .iter()
            .filter(|f| !BUILT_IN.contains(&f.id.as_str()))
            .map(|f| f.path.clone())
            .collect()
    }

    // -- search --------------------------------------------------------------

    /// Notes matching a Bear search query, as `bearcli search` rows (`id`,
    /// `title`, `tags`, `location`, `locked`, `content`), newest first. Only
    /// cached bodies are searched for text; while the cache fills, a text
    /// query also asks Craft's own search, which finds the top matches.
    pub async fn search_rows(
        &self,
        query: &str,
        location: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Value>> {
        let terms = query_terms(query);
        let words: Vec<String> = terms
            .iter()
            .filter(|t| !t.starts_with('-') && !t.starts_with('#') && !t.starts_with('@'))
            .map(|t| unescape_phrase(t))
            .filter(|t| t.chars().any(char::is_alphanumeric))
            .collect();
        let uncached = {
            let state = self.state.lock().unwrap();
            state
                .docs
                .iter()
                .any(|(id, d)| !d.trashed && state.body(id).is_none())
        };
        let mut remote: HashSet<String> = HashSet::new();
        if uncached && !words.is_empty() {
            let query: Vec<(&str, String)> = words.iter().map(|w| ("include", w.clone())).collect();
            if let Ok(found) = self
                .api
                .request("GET", "/documents/search", &query, None)
                .await
            {
                remote = items(&found)
                    .iter()
                    .map(|r| text(r.get("documentId")))
                    .collect();
            }
        }
        let pins = self.pins.lock().unwrap().clone();
        let state = self.state.lock().unwrap();
        let mut hits: Vec<(&String, &Doc)> = state
            .docs
            .iter()
            .filter(|(id, _)| !state.templates.contains(*id))
            .filter(|(id, d)| match location {
                "all" => true,
                other => self.location(&state, id, d).as_str() == other,
            })
            .filter(|(id, d)| {
                let note = self.note(&state, id, d, pins.get(*id));
                let body = state.body(id);
                matches_query(&note, body, &terms, remote.contains(*id))
            })
            .collect();
        hits.sort_by(|a, b| b.1.modified.cmp(&a.1.modified));
        Ok(hits
            .into_iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(|(id, d)| {
                json!({
                    "id": id,
                    "title": d.title,
                    "tags": self.tags(&state, id).iter().map(|t| display_tag(t)).collect::<Vec<_>>(),
                    "location": self.location(&state, id, d).as_str(),
                    "locked": "no",
                    "content": state.body(id).unwrap_or(""),
                })
            })
            .collect())
    }

    // -- writing -------------------------------------------------------------

    /// Write `content` over a document, guarded by `base` (the hash `cat`
    /// gave) unless it is empty.
    pub async fn overwrite(&self, id: &str, content: &str, base: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let current = self.read(self.api.as_ref(), id).await?;
        if !base.is_empty() && hash(&current.content) != base {
            return Err(conflict());
        }
        self.apply(id, &current, content).await
    }

    /// `edit`: one exact find and replace, inside a section when one is given.
    pub async fn edit(&self, id: &str, find: &str, replace: &str, section: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let current = self.read(self.api.as_ref(), id).await?;
        let (start, end) = section_range(&current.content, section)?;
        let region = &current.content[start..end];
        match region.matches(find).count() {
            0 => return Err(BearError::new("Find text not found")),
            1 => {}
            _ => return Err(BearError::new("Find text matches more than once")),
        }
        let updated = format!(
            "{}{}{}",
            &current.content[..start],
            region.replacen(find, replace, 1),
            &current.content[end..]
        );
        self.apply(id, &current, &updated).await
    }

    /// `append`: add text at the end of the note, or of one section.
    pub async fn append(&self, id: &str, added: &str, section: Option<&str>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let current = self.read(self.api.as_ref(), id).await?;
        let added = added.trim_end_matches('\n');
        let content = &current.content;
        let (start, end) = match section.filter(|s| !s.trim().is_empty()) {
            Some(section) => section_range(content, section)?,
            None => (0, content.len()),
        };
        let at = start + content[start..end].trim_end().len();
        // A list grows by a line; anything else is a block of its own.
        let last = content[..at].rsplit('\n').next().unwrap_or("");
        let lists = doc::is_list_item(last) && doc::is_list_item(added);
        let gap = if lists || at == 0 { "\n" } else { "\n\n" };
        let updated = format!("{}{gap}{added}{}", &content[..at], &content[at..]);
        self.apply(id, &current, &updated).await
    }

    /// Turn `current` into `content` with as few block writes as it takes.
    async fn apply(&self, id: &str, current: &Rendered, content: &str) -> Result<()> {
        let ops = doc::plan(current, content, &self.links());
        if ops.is_empty() {
            return Ok(());
        }
        let mut puts: Vec<Value> = Vec::new();
        for op in &ops {
            match op {
                Op::Title(title) => puts.push(json!({"id": id, "markdown": title})),
                Op::Update { id, markdown } => puts.push(json!({"id": id, "markdown": markdown})),
                _ => {}
            }
        }
        if !puts.is_empty() {
            self.api
                .request("PUT", "/blocks", &[], Some(&json!({ "blocks": puts })))
                .await?;
        }
        // Each insert follows a block by id, or the last block an earlier
        // insert made (`@n`).
        let mut made: HashMap<usize, String> = HashMap::new();
        for (n, op) in ops.iter().enumerate() {
            match op {
                Op::Insert { after, markdown } => {
                    let after = match after.as_deref() {
                        Some(marker) if marker.starts_with('@') => marker[1..]
                            .parse::<usize>()
                            .ok()
                            .and_then(|k| made.get(&k).cloned()),
                        other => other.map(str::to_string),
                    };
                    let position = match after {
                        Some(sibling) => json!({"siblingId": sibling, "position": "after"}),
                        None => json!({"pageId": id, "position": "start"}),
                    };
                    let res = self
                        .api
                        .request(
                            "POST",
                            "/blocks",
                            &[],
                            Some(&json!({"markdown": markdown, "position": position})),
                        )
                        .await?;
                    if let Some(last) = items(&res).last() {
                        made.insert(n, text(last.get("id")));
                    }
                }
                Op::Delete(ids) => {
                    self.api
                        .request("DELETE", "/blocks", &[], Some(&json!({ "blockIds": ids })))
                        .await?;
                }
                _ => {}
            }
        }
        self.wrote(id).await
    }

    /// After a write: read the document back, so the cache holds what Craft
    /// made of it, and mark it changed now.
    async fn wrote(&self, id: &str) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if let Some(doc) = state.docs.get_mut(id) {
                doc.modified = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            }
        }
        self.read(self.api.as_ref(), id).await?;
        let mut state = self.state.lock().unwrap();
        state.adopt.insert(id.to_string());
        state.fresh.push(id.to_string());
        state.touch();
        Ok(())
    }

    /// The folder for a tag path, made (with any missing parents) if needed.
    async fn ensure_folder(&self, path: &str) -> Result<Option<String>> {
        let path = normalize_tag(path);
        if path.is_empty() {
            return Ok(None);
        }
        let mut parent: Option<String> = None;
        let mut at = String::new();
        for (depth, name) in path.split('/').filter(|p| !p.trim().is_empty()).enumerate() {
            let name = name.trim();
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(name);
            let existing = {
                let state = self.state.lock().unwrap();
                state
                    .folders
                    .iter()
                    .find(|f| f.parent == parent && f.path.eq_ignore_ascii_case(&at))
                    .map(|f| f.id.clone())
            };
            let id = match existing {
                Some(id) => id,
                None => {
                    let mut entry = json!({ "name": name });
                    if let Some(p) = &parent {
                        entry["parentFolderId"] = json!(p);
                    }
                    let res = self
                        .api
                        .request(
                            "POST",
                            "/folders",
                            &[],
                            Some(&json!({ "folders": [entry] })),
                        )
                        .await?;
                    let id = items(&res)
                        .first()
                        .map(|f| text(f.get("id")))
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| BearError::new("Craft made no folder"))?;
                    let mut state = self.state.lock().unwrap();
                    state.folders.push(Folder {
                        id: id.clone(),
                        path: at.clone(),
                        parent: parent.clone(),
                        depth,
                        count: 0,
                    });
                    state.walked.insert(id.clone(), 0);
                    id
                }
            };
            parent = Some(id);
        }
        Ok(parent)
    }

    /// `create`: a new document from a title (or the content's first line),
    /// in the folder of the first tag. With `if_not_exists`, a document with
    /// that title (any case) is answered instead, wherever it is.
    pub async fn create(
        &self,
        title: Option<&str>,
        tags: &[String],
        content: &str,
        if_not_exists: bool,
    ) -> Result<(String, String, Location)> {
        self.ensure_listed().await?;
        let _guard = self.write_lock.lock().await;
        let (first, rest) = doc::split_title(content);
        let (title, body) = match title {
            Some(t) if !first.eq_ignore_ascii_case(t.trim()) => {
                (t.trim().to_string(), content.trim().to_string())
            }
            Some(t) => (t.trim().to_string(), rest),
            None => (first, rest),
        };
        let title = if title.is_empty() {
            "Untitled".to_string()
        } else {
            title
        };
        if if_not_exists {
            let state = self.state.lock().unwrap();
            if let Some((id, d)) = state
                .docs
                .iter()
                .filter(|(id, _)| !state.templates.contains(*id))
                .find(|(_, d)| d.title.trim().eq_ignore_ascii_case(&title))
            {
                return Ok((id.clone(), d.title.clone(), self.location(&state, id, d)));
            }
        }
        let folder = match tags
            .iter()
            .map(|t| normalize_tag(t))
            .find(|t| !t.is_empty())
        {
            Some(tag) => self.ensure_folder(&tag).await?,
            None => None,
        };
        let destination = match &folder {
            Some(f) => json!({ "folderId": f }),
            None => json!({ "destination": "unsorted" }),
        };
        let res = self
            .api
            .request(
                "POST",
                "/documents",
                &[],
                Some(&json!({"documents": [{"title": title}], "destination": destination})),
            )
            .await?;
        let id = items(&res)
            .first()
            .map(|d| text(d.get("id")))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| BearError::new("Craft create returned no id"))?;
        {
            let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let mut state = self.state.lock().unwrap();
            state.docs.insert(
                id.clone(),
                Doc {
                    title: title.clone(),
                    created: now.clone(),
                    modified: now,
                    trashed: false,
                },
            );
            if let Some(f) = &folder {
                state.folder_of.insert(id.clone(), f.clone());
            }
            state.touch();
        }
        if !body.trim().is_empty() {
            let markdown = doc::from_wiki(&body, &self.links());
            self.api
                .request(
                    "POST",
                    "/blocks",
                    &[],
                    Some(&json!({"markdown": markdown, "position": {"pageId": id, "position": "end"}})),
                )
                .await?;
        }
        self.wrote(&id).await?;
        Ok((id, title, Location::Notes))
    }

    async fn move_to(&self, id: &str, folder: Option<String>) -> Result<()> {
        let destination = match &folder {
            Some(f) => json!({ "folderId": f }),
            None => json!({ "destination": "unsorted" }),
        };
        self.api
            .request(
                "PUT",
                "/documents/move",
                &[],
                Some(&json!({"documentIds": [id], "destination": destination})),
            )
            .await?;
        let mut state = self.state.lock().unwrap();
        match folder {
            Some(f) => state.folder_of.insert(id.to_string(), f),
            None => state.folder_of.remove(id),
        };
        if let Some(doc) = state.docs.get_mut(id) {
            doc.trashed = false;
        }
        state.touch();
        Ok(())
    }

    pub async fn trash(&self, id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.api
            .request(
                "DELETE",
                "/documents",
                &[],
                Some(&json!({ "documentIds": [id] })),
            )
            .await?;
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folder_of.remove(id) {
            state.home.insert(id.to_string(), f);
        }
        if let Some(doc) = state.docs.get_mut(id) {
            doc.trashed = true;
        }
        state.touch();
        Ok(())
    }

    /// Back to where it was before the trash or the archive, or Unsorted.
    pub async fn restore(&self, id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let home = {
            let state = self.state.lock().unwrap();
            state
                .home
                .get(id)
                .filter(|f| state.folder(f).is_some())
                .cloned()
        };
        self.move_to(id, home).await?;
        self.state.lock().unwrap().home.remove(id);
        Ok(())
    }

    pub async fn archive(&self, id: &str) -> Result<()> {
        if self.archive.is_empty() {
            return Err(BearError::new(
                "Craft has no archive; set [craft] archive_folder to use one",
            ));
        }
        let _guard = self.write_lock.lock().await;
        let folder = self.ensure_folder(&self.archive).await?;
        let previous = self.state.lock().unwrap().folder_of.get(id).cloned();
        self.move_to(id, folder).await?;
        if let Some(f) = previous {
            self.state.lock().unwrap().home.insert(id.to_string(), f);
        }
        Ok(())
    }

    pub fn pin(&self, id: &str, target: &str, on: bool) -> Result<()> {
        let mut pins = self.pins.lock().unwrap();
        let entry = pins.entry(id.to_string()).or_default();
        entry.retain(|p| p != target);
        if on {
            entry.push(target.to_string());
        }
        if entry.is_empty() {
            pins.remove(id);
        }
        if let Some(path) = &self.pins_path {
            let body = serde_json::to_vec_pretty(&*pins).unwrap_or_default();
            crate::fake::write_atomic(path, &String::from_utf8_lossy(&body))
                .map_err(|e| BearError::new(format!("could not save pins: {e}")))?;
        }
        self.state.lock().unwrap().touch();
        Ok(())
    }

    /// Open the document in Craft, at the heading `header` when one is given.
    pub async fn open_in_app(&self, id: &str, header: &str) -> Result<()> {
        let space = self.space_id().await?;
        let mut block = id.to_string();
        if !header.trim().is_empty() {
            let rendered = self.read(self.api.as_ref(), id).await?;
            if let Some(seg) = rendered
                .segments
                .iter()
                .find(|s| s.text.trim() == header.trim())
            {
                block = seg.id.clone();
            }
        }
        let url = format!("craftdocs://open?spaceId={space}&blockId={block}");
        let status = tokio::process::Command::new("open")
            .arg(&url)
            .status()
            .await
            .map_err(|e| BearError::new(format!("could not open Craft: {e}")))?;
        if !status.success() {
            return Err(BearError::new("could not open Craft"));
        }
        Ok(())
    }

    async fn space_id(&self) -> Result<String> {
        if let Some(id) = self.space_id.lock().unwrap().clone() {
            return Ok(id);
        }
        let conn = self.api.request("GET", "/connection", &[], None).await?;
        let id = text(conn.get("space").and_then(|s| s.get("id")));
        if id.is_empty() {
            return Err(BearError::new("Craft did not say which space this is"));
        }
        *self.space_id.lock().unwrap() = Some(id.clone());
        Ok(id)
    }

    // -- cache ---------------------------------------------------------------

    pub fn dirty(&self) -> bool {
        let state = self.state.lock().unwrap();
        self.cache_path.is_some() && state.saved_generation != state.generation
    }

    /// Write the cache out. A failure only costs the next launch a cold start.
    pub fn save(&self) {
        let Some(path) = &self.cache_path else { return };
        let (body, generation) = {
            let state = self.state.lock().unwrap();
            (serde_json::to_vec(&*state), state.generation)
        };
        if let Ok(body) = body
            && crate::bear::write_private(path, &body).is_ok()
        {
            self.state.lock().unwrap().saved_generation = generation;
        }
    }
}

fn flatten_folders(
    items: &[Value],
    parent: Option<&str>,
    prefix: &str,
    depth: usize,
    out: &mut Vec<Folder>,
) {
    for item in items {
        let id = text(item.get("id"));
        if id.is_empty() {
            continue;
        }
        let name = text(item.get("name")).replace('/', "-");
        let built_in = parent.is_none() && BUILT_IN.contains(&id.as_str());
        let path = if built_in {
            String::new()
        } else if prefix.is_empty() {
            name.trim().to_string()
        } else {
            format!("{prefix}/{}", name.trim())
        };
        out.push(Folder {
            id: id.clone(),
            path: path.clone(),
            parent: parent.map(str::to_string),
            depth,
            count: item
                .get("documentCount")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        });
        if let Some(children) = item.get("folders").and_then(Value::as_array) {
            flatten_folders(children, Some(&id), &path, depth + 1, out);
        }
    }
}

/// The byte range of the section a heading line addresses, from the heading
/// to the next heading of the same or a higher level; the whole note for none.
fn section_range(content: &str, section: &str) -> Result<(usize, usize)> {
    let heading = section.trim();
    if heading.is_empty() {
        return Ok((0, content.len()));
    }
    let level = heading.len() - heading.trim_start_matches('#').len();
    if !(1..=6).contains(&level) {
        return Err(BearError::new("Section not found"));
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].trim() == heading)
        .collect();
    let [first] = starts[..] else {
        return Err(BearError::new(if starts.is_empty() {
            "Section not found"
        } else {
            "Section address is ambiguous"
        }));
    };
    let mut last = lines.len();
    for (j, line) in lines.iter().enumerate().skip(first + 1) {
        let hashes = line.len() - line.trim_start_matches('#').len();
        if (1..=level).contains(&hashes) && line[hashes..].starts_with(' ') {
            last = j;
            break;
        }
    }
    let start = lines[..first].join("\n").len() + usize::from(first > 0);
    Ok((start, lines[..last].join("\n").len()))
}

/// A Bear query's terms; a quoted phrase is one term, quotes kept.
fn query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for ch in query.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                current.push(ch);
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

/// A term's text: quotes off, Bear's backslash escapes undone.
fn unescape_phrase(term: &str) -> String {
    let inner = term.trim_matches('"');
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Bear's search, as far as Bjorn uses it: words and phrases, `#tag` (with
/// its children), `@todo`, `@tagged`, `@untagged`, `@today`, `@pinned`,
/// `@wikilinks`, and `-` to negate. `remote` says Craft's search found the
/// note, which stands in for the text of a body not cached yet.
fn matches_query(note: &Note, body: Option<&str>, terms: &[String], remote: bool) -> bool {
    let today = chrono::Local::now().date_naive();
    let haystack = body
        .map(str::to_lowercase)
        .unwrap_or_else(|| note.title.to_lowercase());
    for term in terms {
        let (negate, term) = match term.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, term.as_str()),
        };
        let ok = if term.starts_with('#') {
            let tag = normalize_tag(term).to_lowercase();
            note.tags.iter().any(|t| t.to_lowercase() == tag)
        } else if term == "@todo" {
            note.todos > 0
        } else if term == "@untagged" {
            note.tags.is_empty()
        } else if term == "@tagged" {
            !note.tags.is_empty()
        } else if term == "@today" {
            note.modified_local_date() == Some(today)
        } else if term == "@pinned" {
            note.pinned_globally()
        } else if term == "@wikilinks" {
            haystack.contains("[[")
        } else if term.starts_with('@') {
            true
        } else {
            let needle = unescape_phrase(term).to_lowercase();
            needle.chars().any(char::is_alphanumeric)
                && (haystack.contains(&needle) || (body.is_none() && remote))
        };
        if ok == negate {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;
