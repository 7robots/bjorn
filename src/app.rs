//! The Bjorn application: three panes over one bearcli snapshot.
//!
//! One `App` owns every piece of state and is driven by three inputs: terminal
//! events, `Msg` values sent back by the tokio tasks that talk to bearcli, and
//! `tick` for timers. Drawing reads the state; nothing here touches the
//! terminal. Results are tagged with a generation number and a stale one is
//! dropped, never cancelled mid-flight.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::bear::{
    BearClient, BearError, Note, NoteContent, Probe, Snapshot, display_tag, normalize_tag,
    recently_modified,
};
use crate::config::{Config, editor_available, resolve_editor};
use crate::editor::{self, EditorJob};
use crate::export::{
    FORMATS, Format, default_export_path, export_note, extension_for, format_by_id,
};
use crate::icons::IconSet;
use crate::model::{Selection, View, duplicate_titles, select_notes, today};
use crate::pty::PtySession;
use crate::reminders::{
    RemctlClient, Status, join as join_reminders, remctl_found, resolve_remctl,
};
use crate::search::{query_pattern, rewrite_subtags};
use crate::todos::{TodoScan, scan_rows};
use crate::ui::modals::{Field, Overlay, Pending, Severity, TextPurpose, Toast};
use crate::ui::note_list::{NoteList, ROW_HEIGHT};
use crate::ui::note_view::Reader;
use crate::ui::sidebar::{Row, Sidebar};
use crate::ui::triage::{Triage, TriageRow};

/// Delay between the list cursor moving and the note being fetched and rendered.
/// Only a note whose body has to be read from Bear waits: a cached one is drawn
/// on the next frame, so holding `j` down stays instant.
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(120);
/// Note bodies kept in memory, most recently read last. Each is keyed by the
/// note's modification stamp, so a changed note is fetched again on its own.
pub const CONTENT_CACHE_SIZE: usize = 512;
/// Bytes of note bodies to keep. A thousand notes of a few KB each fit; one
/// 170 KB monster does not evict the rest of the library on its own.
pub const CONTENT_CACHE_BYTES: usize = 24 * 1024 * 1024;
/// How far either side of the cursor to read ahead, so the next `j` or `k`
/// lands on a note that is already in hand.
pub const PREFETCH_RADIUS: usize = 3;
/// Read-aheads allowed in flight at once; bearcli is a process per call.
pub const PREFETCH_INFLIGHT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Notes,
    Reader,
    Search,
}

/// What the background tasks send back.
#[derive(Debug)]
pub enum Msg {
    Loaded {
        generation: u64,
        result: Result<Snapshot, BearError>,
        probe: Option<Probe>,
        keep_id: Option<String>,
        focus_id: Option<String>,
        force: bool,
    },
    Probed(Result<Probe, BearError>),
    /// A read-ahead finished: the body goes in the cache, nothing is drawn.
    Prefetched {
        epoch: u64,
        id: String,
        stamp: String,
        body: Option<String>,
    },
    Content {
        generation: u64,
        epoch: u64,
        result: Result<NoteContent, BearError>,
    },
    SearchDone {
        query: String,
        result: Result<Vec<String>, BearError>,
    },
    Created {
        result: Result<String, BearError>,
    },
    EditContent {
        note: Note,
        result: Result<NoteContent, BearError>,
    },
    Written {
        job: EditorJob,
        result: Result<(), BearError>,
    },
    /// The editor drew something; a redraw is due.
    EditorOutput,
    /// The editor process ended.
    EditorExited(std::io::Result<portable_pty::ExitStatus>),
    Trashed {
        note: Note,
        result: Result<(), BearError>,
    },
    Restored {
        note: Note,
        result: Result<(), BearError>,
    },
    Pinned {
        note_id: String,
        result: Result<(), BearError>,
    },
    Opened(Result<(), BearError>),
    Exported(Result<PathBuf, String>),
    TriageLoaded {
        scan: TodoScan,
        statuses: HashMap<String, (Status, i64)>,
        error: String,
    },
    TriageTicked {
        ticked: Vec<String>,
        failures: Vec<String>,
    },
    TriageAdded {
        added: usize,
        failures: Vec<String>,
        keys: Vec<String>,
    },
}

/// Where the last frame put each pane, for mouse events.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rects {
    pub sidebar_header: Rect,
    pub sidebar_rows: Rect,
    pub notes_header: Rect,
    pub search_box: Rect,
    pub notes_rows: Rect,
    pub note_bar: Rect,
    pub glyph: Rect,
    pub reader_body: Rect,
    /// Where the editor's screen is drawn while one is open.
    pub editor: Rect,
}

/// An editor open in the reader pane.
pub struct Editing {
    pub job: EditorJob,
    pub pty: PtySession,
}

pub struct App {
    pub config: Config,
    pub client: Arc<BearClient>,
    pub icons: IconSet,
    pub environ: HashMap<String, String>,
    pub snapshot: Snapshot,
    pub selection: Selection,
    pub search_query: String,
    pub loaded: bool,
    pub running: bool,
    pub columns: u8,
    pub focus: Pane,
    pub sidebar: Sidebar,
    pub notes: NoteList,
    pub reader: Reader,
    pub overlay: Option<Overlay>,
    pub toasts: Vec<Toast>,
    pub rects: Rects,
    tx: UnboundedSender<Msg>,
    /// Note id, modification stamp, body. The reader's cache only: the edit
    /// path always re-reads, because it needs a fresh hash to write against.
    content_cache: VecDeque<(String, String, String)>,
    content_cache_bytes: usize,
    /// Read-aheads running now, and the ids they are for.
    prefetching: std::collections::HashSet<String>,
    cache_epoch: u64,
    load_gen: u64,
    reload_gen: u64,
    preview_due: Option<(Instant, Note)>,
    last_probe: Option<Probe>,
    next_poll: Option<Instant>,
    poll_inflight: bool,
    reload_inflight: bool,
    /// Set while an editor is open; the poll waits.
    pub busy: bool,
    written_titles: Vec<String>,
    /// The editor running in the reader pane, if any. Every key goes to it.
    pub editing: Option<Editing>,
    /// A note just created, to open in the editor once the reload shows it.
    pending_edit: Option<String>,
    /// The triage screen while it is up; it covers the three columns.
    pub triage: Option<Triage>,
    pub remctl: Option<Arc<RemctlClient>>,
    reminders_notice_shown: bool,
    reader_width: usize,
    reader_height: usize,
}

impl App {
    pub fn new(
        config: Config,
        client: Arc<BearClient>,
        workspace: Option<&str>,
        environ: HashMap<String, String>,
    ) -> (App, UnboundedReceiver<Msg>) {
        let (tx, rx) = unbounded_channel();
        // The palette is process-wide, so every entry point (the binary, the
        // gate, the test harness) picks it up from the config it was given.
        crate::ui::theme::set(&config.theme);
        let icons = IconSet::new(
            &config.icon_style,
            &config.icons,
            environ.get("TERM_PROGRAM").map(String::as_str),
        );
        let ws = normalize_tag(workspace.unwrap_or(&config.workspace));
        let mut sidebar = Sidebar::new(icons.clone());
        sidebar.set_workspace(&ws);
        let remctl = if config.reminders.enabled && remctl_found(&config.reminders.remctl) {
            Some(Arc::new(RemctlClient::new(vec![resolve_remctl(
                &config.reminders.remctl,
            )])))
        } else {
            None
        };
        let mut app = App {
            selection: Selection {
                workspace: ws,
                ..Selection::default()
            },
            config,
            client,
            icons,
            environ,
            snapshot: Snapshot::default(),
            search_query: String::new(),
            loaded: false,
            running: true,
            columns: 3,
            focus: Pane::Notes,
            sidebar,
            notes: NoteList::default(),
            reader: Reader::default(),
            overlay: None,
            toasts: Vec::new(),
            rects: Rects::default(),
            tx,
            content_cache: VecDeque::new(),
            content_cache_bytes: 0,
            prefetching: std::collections::HashSet::new(),
            cache_epoch: 0,
            load_gen: 0,
            reload_gen: 0,
            preview_due: None,
            last_probe: None,
            next_poll: None,
            poll_inflight: false,
            reload_inflight: false,
            busy: false,
            written_titles: Vec::new(),
            editing: None,
            pending_edit: None,
            triage: None,
            remctl,
            reminders_notice_shown: false,
            reader_width: 80,
            reader_height: 24,
        };
        app.reader.clear("Loading\u{2026}");
        (app, rx)
    }

    /// Kick off the first load and the poll timer.
    pub fn start(&mut self) {
        self.start_reload(None, None, false);
        if self.config.poll_seconds > 0 {
            self.next_poll = Some(Instant::now() + Duration::from_secs(self.config.poll_seconds));
        }
    }

    fn env(&self, name: &str) -> Option<String> {
        self.environ.get(name).cloned()
    }

    // -- notifications -------------------------------------------------------------

    pub fn notify(&mut self, message: &str, timeout: Duration) {
        self.toasts
            .push(Toast::new("", message, Severity::Information, timeout));
    }

    pub fn notify_titled(
        &mut self,
        title: &str,
        message: &str,
        severity: Severity,
        timeout: Duration,
    ) {
        self.toasts
            .push(Toast::new(title, message, severity, timeout));
    }

    fn error(&mut self, title: &str, err: &BearError) {
        self.notify_titled(
            title,
            &err.message,
            Severity::Error,
            Duration::from_secs(10),
        );
    }

    /// Messages of the toasts currently showing, for tests.
    pub fn toast_messages(&self) -> Vec<String> {
        self.toasts.iter().map(|t| t.message.clone()).collect()
    }

    // -- timers --------------------------------------------------------------------

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let mut deadline: Option<Instant> = None;
        let mut consider = |t: Instant| deadline = Some(deadline.map_or(t, |d: Instant| d.min(t)));
        if let Some((due, _)) = &self.preview_due {
            consider(*due);
        }
        if let Some(poll) = self.next_poll {
            consider(poll);
        }
        for toast in &self.toasts {
            consider(toast.expires);
        }
        deadline
    }

    pub fn tick(&mut self, now: Instant) {
        self.toasts.retain(|t| t.expires > now);
        if let Some((due, note)) = self.preview_due.clone()
            && due <= now
        {
            self.preview_due = None;
            self.load_note(note);
        }
        if let Some(poll) = self.next_poll
            && poll <= now
        {
            self.next_poll = Some(now + Duration::from_secs(self.config.poll_seconds.max(1)));
            self.poll();
        }
    }

    // -- data ----------------------------------------------------------------------

    /// Take a fresh snapshot (and probe) in the background; `Msg::Loaded`
    /// redraws every pane around it. `force` re-renders the reader even when it
    /// already shows the current note (after `r`, which also drops the cache).
    pub fn start_reload(&mut self, keep_id: Option<String>, focus_id: Option<String>, force: bool) {
        self.reload_gen += 1;
        self.reload_inflight = true;
        let generation = self.reload_gen;
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let (snapshot, probe) = tokio::join!(client.snapshot(), client.probe());
            let _ = tx.send(Msg::Loaded {
                generation,
                result: snapshot,
                probe: probe.ok(),
                keep_id,
                focus_id,
                force,
            });
        });
    }

    fn on_loaded(
        &mut self,
        result: Result<Snapshot, BearError>,
        probe: Option<Probe>,
        keep_id: Option<String>,
        focus_id: Option<String>,
        force: bool,
    ) {
        self.reload_inflight = false;
        let mut snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.error("bearcli", &err);
                if !self.loaded {
                    self.reader.clear(&format!("Could not read Bear: {err}"));
                }
                return;
            }
        };
        // The cold listing reads every body to build the previews; keeping
        // them means the reader never waits for bearcli again this session.
        let bodies = std::mem::take(&mut snapshot.bodies);
        self.snapshot = snapshot;
        self.loaded = true;
        // The previews this snapshot settled on are worth the next launch not
        // having to read every body again. Off the frame's path, and only when
        // they moved: the poll takes a snapshot every few seconds.
        if self.client.preview_cache_dirty() {
            let client = self.client.clone();
            tokio::task::spawn_blocking(move || client.save_preview_cache());
        }
        // Oldest first, so that if the cache has to evict, the notes Bear
        // listed first — the most recently modified — are the last to go. The
        // stamp comes from the note, so it is spelled the way `cached_content`
        // will look it up.
        for (id, body) in bodies.into_iter().rev() {
            let Some(stamp) = self
                .snapshot
                .by_id(&id)
                .map(|n| n.modified.map(|m| m.to_rfc3339()).unwrap_or_default())
            else {
                continue;
            };
            self.remember_content(&id, &stamp, &body);
        }
        if let Some(probe) = probe {
            self.last_probe = Some(probe);
        }
        let current_id = self.notes.current().map(|n| n.id.clone());
        let keep_tag = self.selection.tag.clone();
        let keep_view = self.selection.view;
        self.sidebar.populate(
            &self.snapshot,
            &self.selection.workspace.clone(),
            &keep_tag,
            keep_view,
        );
        let keep = focus_id.clone().or(keep_id).or(current_id);
        self.apply_selection(keep.as_deref(), force);
        if let Some(id) = focus_id
            && self.notes.select_id(&id)
            && let Some(note) = self.notes.current().cloned()
        {
            self.schedule_preview(note, true, false);
        }
        self.warn_duplicates();
        if let Some(id) = self.pending_edit.take()
            && let Some(note) = self.snapshot.by_id(&id).cloned()
        {
            self.edit_note(note);
        }
    }

    /// Rebuild the notes list for the current selection and line the reader up
    /// with the note under the cursor. `force` redraws the reader even when it
    /// already shows that version of the note.
    pub fn apply_selection(&mut self, keep_id: Option<&str>, force: bool) {
        let notes = select_notes(&self.snapshot, &self.selection, today());
        let header = if self.search_query.is_empty() {
            self.selection.describe()
        } else {
            format!("“{}”", self.search_query)
        };
        self.notes.header = format!("{header} · {}", notes.len());
        let pattern = if self.search_query.is_empty() {
            None
        } else {
            query_pattern(&self.search_query)
        };
        self.reader.set_pattern(pattern.clone());
        self.notes.pattern = pattern;
        match self.notes.show_notes(notes, keep_id) {
            Some(note) => self.schedule_preview(note, force, force),
            None => {
                self.preview_due = None;
                self.load_gen += 1;
                self.reader.clear("No note selected");
            }
        }
    }

    fn warn_duplicates(&mut self) {
        if self.written_titles.is_empty() {
            return;
        }
        let dups = duplicate_titles(&self.snapshot, &self.written_titles);
        for (title, ids) in dups {
            self.notify_titled(
                "Duplicate note",
                &format!("“{title}” now exists {} times — iCloud may have resurrected the old version. Check in Bear before trashing either copy.", ids.len()),
                Severity::Warning,
                Duration::from_secs(15),
            );
        }
        self.written_titles.clear();
    }

    /// Remember a title just written, so the next reload can spot a duplicate.
    pub fn note_written(&mut self, title: &str) {
        self.written_titles.push(title.to_string());
    }

    fn poll(&mut self) {
        if self.busy || !self.loaded || self.poll_inflight || self.reload_inflight {
            return;
        }
        self.poll_inflight = true;
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Probed(client.probe().await));
        });
    }

    fn on_probed(&mut self, result: Result<Probe, BearError>) {
        self.poll_inflight = false;
        let Ok(probe) = result else { return };
        match &self.last_probe {
            Some(last) if *last != probe => {
                self.last_probe = Some(probe);
                self.start_reload(None, None, false);
            }
            _ => self.last_probe = Some(probe),
        }
    }

    // -- messages ------------------------------------------------------------------

    pub fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Prefetched {
                epoch,
                id,
                stamp,
                body,
            } => self.on_prefetched(epoch, id, stamp, body),
            Msg::Loaded {
                generation,
                result,
                probe,
                keep_id,
                focus_id,
                force,
            } => {
                if generation == self.reload_gen {
                    self.on_loaded(result, probe, keep_id, focus_id, force);
                }
            }
            Msg::Probed(result) => self.on_probed(result),
            Msg::Content {
                generation,
                epoch,
                result,
            } => self.on_content(generation, epoch, result),
            Msg::SearchDone { query, result } => self.on_search_done(query, result),
            Msg::Created { result } => match result {
                Ok(id) => {
                    self.pending_edit = Some(id.clone());
                    self.start_reload(None, Some(id), false);
                }
                Err(err) => self.error("Create failed", &err),
            },
            Msg::EditContent { note, result } => match result {
                Ok(before) => self.open_editor(note, before),
                Err(err) => self.error("Read failed", &err),
            },
            Msg::Written { job, result } => self.on_written(job, result),
            Msg::EditorOutput => {}
            Msg::EditorExited(status) => self.editor_exited(status),
            Msg::Trashed { note, result } => match result {
                Ok(()) => {
                    self.forget_content(Some(&note.id));
                    self.start_reload(None, None, false);
                    self.notify(
                        &format!(
                            "Trashed “{}” — restore it from the Trash view with u.",
                            note.title
                        ),
                        Duration::from_secs(4),
                    );
                }
                Err(err) => self.error("Trash failed", &err),
            },
            Msg::Restored { note, result } => match result {
                Ok(()) => {
                    self.start_reload(None, None, false);
                    self.notify(
                        &format!("Restored “{}”.", note.title),
                        Duration::from_secs(3),
                    );
                }
                Err(err) => self.error("Restore failed", &err),
            },
            Msg::Pinned { note_id, result } => match result {
                Ok(()) => self.start_reload(Some(note_id), None, false),
                Err(err) => self.error("Pin failed", &err),
            },
            Msg::Opened(result) => {
                if let Err(err) = result {
                    self.error("Open in Bear failed", &err);
                }
            }
            Msg::TriageLoaded {
                scan,
                statuses,
                error,
            } => {
                if let Some(triage) = self.triage.as_mut() {
                    triage.show(scan, &statuses, &error);
                }
            }
            Msg::TriageTicked { ticked, failures } => self.on_triage_ticked(ticked, failures),
            Msg::TriageAdded {
                added,
                failures,
                keys,
            } => self.on_triage_added(added, failures, keys),
            Msg::Exported(result) => match result {
                Ok(path) => self.notify(
                    &format!("Exported to {}", path.display()),
                    Duration::from_secs(5),
                ),
                Err(message) => self.notify_titled(
                    "Export failed",
                    &message,
                    Severity::Error,
                    Duration::from_secs(10),
                ),
            },
        }
    }

    // -- preview -------------------------------------------------------------------

    /// Render `note` after the debounce, unless the reader already shows this
    /// version of it: a list rebuild re-highlights the same note, and redrawing
    /// it would blank the page for nothing.
    pub fn schedule_preview(&mut self, note: Note, immediate: bool, force: bool) {
        if !force && !recently_modified(note.modified, None) && self.reader.shows(&note) {
            return;
        }
        self.load_gen += 1;
        // The debounce exists to keep bearcli off the critical path while the
        // cursor is moving. A body already in memory costs a render, so it is
        // drawn now: this is what makes holding `j` feel like scrolling.
        if immediate || self.has_cached_content(&note) {
            self.preview_due = None;
            self.load_note(note);
        } else {
            self.preview_due = Some((Instant::now() + PREVIEW_DEBOUNCE, note));
        }
    }

    /// Is this version of the note's body in the cache?
    fn has_cached_content(&self, note: &Note) -> bool {
        if note.locked || recently_modified(note.modified, None) {
            return false;
        }
        let stamp = note.modified.map(|m| m.to_rfc3339()).unwrap_or_default();
        self.content_cache
            .iter()
            .any(|(id, s, _)| *id == note.id && *s == stamp)
    }

    fn cached_content(&mut self, note: &Note) -> Option<String> {
        let stamp = note.modified.map(|m| m.to_rfc3339()).unwrap_or_default();
        if recently_modified(note.modified, None) {
            return None;
        }
        let pos = self
            .content_cache
            .iter()
            .position(|(id, s, _)| *id == note.id && *s == stamp)?;
        // Most recently read last, so the eviction below drops cold bodies.
        let entry = self.content_cache.remove(pos)?;
        let body = entry.2.clone();
        self.content_cache.push_back(entry);
        Some(body)
    }

    pub fn remember_content(&mut self, note_id: &str, stamp: &str, body: &str) {
        self.drop_cached(note_id);
        self.content_cache_bytes += body.len();
        self.content_cache
            .push_back((note_id.to_string(), stamp.to_string(), body.to_string()));
        while self.content_cache.len() > CONTENT_CACHE_SIZE
            || (self.content_cache_bytes > CONTENT_CACHE_BYTES && self.content_cache.len() > 1)
        {
            if let Some((_, _, body)) = self.content_cache.pop_front() {
                self.content_cache_bytes = self.content_cache_bytes.saturating_sub(body.len());
            }
        }
    }

    fn drop_cached(&mut self, note_id: &str) {
        let mut kept = VecDeque::with_capacity(self.content_cache.len());
        while let Some(entry) = self.content_cache.pop_front() {
            if entry.0 == note_id {
                self.content_cache_bytes = self.content_cache_bytes.saturating_sub(entry.2.len());
            } else {
                kept.push_back(entry);
            }
        }
        self.content_cache = kept;
    }

    /// Drop one note's body (or all of them) and outdate any read in flight.
    pub fn forget_content(&mut self, note_id: Option<&str>) {
        match note_id {
            None => {
                self.content_cache.clear();
                self.content_cache_bytes = 0;
            }
            Some(id) => self.drop_cached(id),
        }
        self.cache_epoch += 1;
    }

    /// Read the bodies either side of the cursor into the cache, so the next
    /// step of the cursor draws without waiting for bearcli. At most
    /// `PREFETCH_INFLIGHT` reads run at once and each note is asked for once.
    fn prefetch_around_cursor(&mut self) {
        if self.prefetching.len() >= PREFETCH_INFLIGHT {
            return;
        }
        let Some(cursor) = self.notes.cursor else {
            return;
        };
        let mut wanted: Vec<Note> = Vec::new();
        for step in 1..=PREFETCH_RADIUS {
            for index in [cursor.checked_sub(step), Some(cursor + step)]
                .into_iter()
                .flatten()
            {
                let Some(note) = self.notes.notes.get(index) else {
                    continue;
                };
                if note.locked
                    || self.prefetching.contains(&note.id)
                    || self.has_cached_content(note)
                    || recently_modified(note.modified, None)
                {
                    continue;
                }
                wanted.push(note.clone());
            }
        }
        for note in wanted
            .into_iter()
            .take(PREFETCH_INFLIGHT - self.prefetching.len())
        {
            let client = self.client.clone();
            let tx = self.tx.clone();
            let epoch = self.cache_epoch;
            let id = note.id.clone();
            let stamp = note.modified.map(|m| m.to_rfc3339()).unwrap_or_default();
            self.prefetching.insert(id.clone());
            tokio::spawn(async move {
                let body = client.cat(&id).await.map(|c| c.content).ok();
                let _ = tx.send(Msg::Prefetched {
                    epoch,
                    id,
                    stamp,
                    body,
                });
            });
        }
    }

    fn on_prefetched(&mut self, epoch: u64, id: String, stamp: String, body: Option<String>) {
        self.prefetching.remove(&id);
        if epoch == self.cache_epoch
            && let Some(body) = body
        {
            self.remember_content(&id, &stamp, &body);
        }
        // One finished, so the next one along can start.
        self.prefetch_around_cursor();
    }

    fn load_note(&mut self, note: Note) {
        let generation = self.load_gen;
        if note.locked {
            self.reader.show_error(
                &note,
                "This note is locked; Bear does not expose its content.",
            );
            return;
        }
        if let Some(body) = self.cached_content(&note) {
            self.reader.show(&note, &body);
            self.prefetch_around_cursor();
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        let epoch = self.cache_epoch;
        let id = note.id.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Content {
                generation,
                epoch,
                result: client.cat(&id).await,
            });
        });
    }

    fn on_content(&mut self, generation: u64, epoch: u64, result: Result<NoteContent, BearError>) {
        if generation != self.load_gen {
            return;
        }
        let Some(note) = self.notes.current().cloned() else {
            return;
        };
        match result {
            Ok(content) => {
                if content.id != note.id {
                    return;
                }
                let stamp = note.modified.map(|m| m.to_rfc3339()).unwrap_or_default();
                if epoch == self.cache_epoch && !recently_modified(note.modified, None) {
                    self.remember_content(&note.id, &stamp, &content.content);
                }
                self.reader.show(&note, &content.content);
                self.prefetch_around_cursor();
            }
            Err(err) => self
                .reader
                .show_error(&note, &format!("Could not read note: {err}")),
        }
    }

    pub fn current_note(&self) -> Option<&Note> {
        self.notes.current()
    }

    /// Tags for search-box completion: the workspace's subtree first, then the
    /// rest, each group sorted.
    pub fn query_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .snapshot
            .notes
            .iter()
            .flat_map(|n| n.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        let ws = &self.selection.workspace;
        if ws.is_empty() {
            return tags;
        }
        let prefix = format!("{ws}/");
        let (inside, outside): (Vec<String>, Vec<String>) = tags
            .into_iter()
            .partition(|t| t == ws || t.starts_with(&prefix));
        inside.into_iter().chain(outside).collect()
    }

    // -- selection -----------------------------------------------------------------

    fn select_view(&mut self, view: View) {
        self.drop_search();
        self.selection = Selection {
            view,
            workspace: self.selection.workspace.clone(),
            ..Selection::default()
        };
        self.apply_selection(None, false);
    }

    fn select_tag(&mut self, tag: &str) {
        self.drop_search();
        self.selection = Selection {
            view: View::All,
            tag: tag.to_string(),
            workspace: self.selection.workspace.clone(),
            search_ids: None,
        };
        self.apply_selection(None, false);
    }

    /// A new view or tag replaces the search: forget the query and close the box.
    fn drop_search(&mut self) {
        self.search_query.clear();
        if self.notes.search.open {
            self.notes.search.close();
            if self.focus == Pane::Search {
                self.focus = Pane::Notes;
            }
        }
    }

    /// The sidebar cursor moved (or the column got focus): apply what it points
    /// at. `on_focus` skips the rebuild when the selection is already showing.
    fn sidebar_announce(&mut self, on_focus: bool) {
        match self.sidebar.highlighted().cloned() {
            Some(Row::View(view)) => {
                if on_focus && self.selection.view == view && self.selection.tag.is_empty() {
                    return;
                }
                self.select_view(view);
            }
            Some(Row::Tag { path, .. }) => {
                if on_focus && self.selection.tag == path {
                    return;
                }
                self.select_tag(&path);
            }
            _ => {}
        }
    }

    pub fn action_view(&mut self, view: View) {
        self.sidebar.select_view(view);
        self.select_view(view);
    }

    // -- search --------------------------------------------------------------------

    pub fn open_search(&mut self) {
        let query = self.search_query.clone();
        self.notes.search.open_with(&query);
        let tags = self.query_tags();
        self.notes.search.refresh_suggestion(&tags);
        self.focus = Pane::Search;
    }

    /// `esc`: close the box and drop the search and its highlights.
    pub fn clear_search(&mut self) {
        if self.notes.search.open {
            self.notes.search.close();
            if self.focus == Pane::Search {
                self.focus = Pane::Notes;
            }
        }
        if !self.search_query.is_empty() || self.selection.search_ids.is_some() {
            self.search_query.clear();
            self.selection.search_ids = None;
            self.apply_selection(None, false);
        }
    }

    fn submit_search(&mut self) {
        let typed = self.notes.search.value.trim().to_string();
        if typed.is_empty() {
            self.clear_search();
            return;
        }
        let query = rewrite_subtags(&typed, &self.query_tags());
        if query != typed {
            self.notes.search.value = query.clone();
            self.notes.search.cursor = query.chars().count();
            self.notify(&format!("Sub-tag search: {query}"), Duration::from_secs(3));
        }
        self.notes.search.suggestion = None;
        self.search_query = query.clone();
        self.focus = Pane::Notes;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let location = self.selection.view.location().as_str().to_string();
        tokio::spawn(async move {
            let result = client.search_ids(&query, &location).await;
            let _ = tx.send(Msg::SearchDone { query, result });
        });
    }

    fn on_search_done(&mut self, query: String, result: Result<Vec<String>, BearError>) {
        if self.search_query != query {
            return;
        }
        match result {
            Ok(ids) => {
                self.selection.search_ids = Some(ids);
                self.apply_selection(None, false);
            }
            Err(err) => self.error("Search", &err),
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        let box_ = &mut self.notes.search;
        let mut edited = false;
        match key.code {
            KeyCode::Esc => {
                self.clear_search();
                return;
            }
            KeyCode::Enter => {
                self.submit_search();
                return;
            }
            KeyCode::Tab => {
                if box_.accept() {
                    edited = true;
                } else {
                    self.focus = Pane::Notes;
                    return;
                }
            }
            KeyCode::BackTab => {
                self.focus = Pane::Notes;
                return;
            }
            KeyCode::Right => edited = box_.right(),
            KeyCode::Left => box_.left(),
            KeyCode::Home => box_.home(),
            KeyCode::End => box_.end(),
            KeyCode::Backspace => {
                box_.backspace();
                edited = true;
            }
            KeyCode::Delete => {
                box_.delete();
                edited = true;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                box_.insert(c);
                edited = true;
            }
            _ => {}
        }
        if edited {
            let tags = self.query_tags();
            self.notes.search.refresh_suggestion(&tags);
        }
    }

    /// `]` / `[`: the next or previous matching block in the reader.
    fn jump_match(&mut self, delta: i64) {
        if self.reader.pattern.is_none() {
            return;
        }
        self.reader.jump(delta, self.reader_width);
        self.focus = Pane::Reader;
    }

    // -- columns and focus ----------------------------------------------------------

    pub fn set_columns(&mut self, count: u8) {
        self.columns = count.clamp(1, 3);
        let hidden = match self.focus {
            Pane::Sidebar => self.columns < 3,
            Pane::Notes | Pane::Search => self.columns < 2,
            Pane::Reader => false,
        };
        if hidden {
            self.set_focus(if self.columns >= 2 {
                Pane::Notes
            } else {
                Pane::Reader
            });
        }
    }

    pub fn cycle_columns(&mut self) {
        self.set_columns(if self.columns == 1 {
            3
        } else {
            self.columns - 1
        });
    }

    pub fn visible_panes(&self) -> Vec<Pane> {
        let mut panes = Vec::new();
        if self.columns == 3 {
            panes.push(Pane::Sidebar);
        }
        if self.columns >= 2 {
            panes.push(Pane::Notes);
        }
        panes.push(Pane::Reader);
        panes
    }

    pub fn set_focus(&mut self, pane: Pane) {
        let was = self.focus;
        self.focus = pane;
        if pane == Pane::Sidebar && was != Pane::Sidebar {
            self.sidebar_announce(true);
        }
    }

    fn focus_next(&mut self, delta: i32) {
        let panes = self.visible_panes();
        let i = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as i32;
        let next = (i + delta).rem_euclid(panes.len() as i32) as usize;
        self.set_focus(panes[next]);
    }

    // -- workspace -----------------------------------------------------------------

    pub fn set_workspace(&mut self, tag: &str) {
        let tag = normalize_tag(tag);
        self.drop_search();
        self.selection = Selection {
            view: View::All,
            workspace: tag.clone(),
            ..Selection::default()
        };
        self.sidebar.populate(&self.snapshot, &tag, "", View::All);
        self.apply_selection(None, false);
        let message = if tag.is_empty() {
            "Workspace cleared".to_string()
        } else {
            format!("Workspace: {}", display_tag(&tag))
        };
        self.notify(&message, Duration::from_secs(3));
    }

    /// `w` scopes to the highlighted tag; pressed again on the workspace itself
    /// (or with no other tag in hand) it clears the scope.
    fn toggle_workspace(&mut self) {
        let mut tag = String::new();
        if self.focus == Pane::Sidebar {
            tag = self.sidebar.highlighted_tag();
        }
        if tag.is_empty() {
            tag = self.selection.tag.clone();
        }
        let current = self.selection.workspace.clone();
        if !current.is_empty() && (tag.is_empty() || tag == current) {
            self.set_workspace("");
            return;
        }
        if tag.is_empty() {
            self.notify_titled(
                "Workspace",
                "Highlight a tag first (the workspace is a tag subtree).",
                Severity::Information,
                Duration::from_secs(5),
            );
            return;
        }
        self.set_workspace(&tag);
    }

    fn fold_tag(&mut self) {
        if self.sidebar.highlighted_tag().is_empty() && !self.selection.tag.is_empty() {
            let tag = self.selection.tag.clone();
            self.sidebar.move_to_tag(&tag);
        }
        self.sidebar.fold_at_cursor();
    }

    fn fold_all(&mut self) {
        let expanded = self.sidebar.toggle_all_folds();
        self.notify(
            if expanded {
                "All tags expanded"
            } else {
                "All tags folded"
            },
            Duration::from_millis(1500),
        );
    }

    // -- actions: navigation -------------------------------------------------------

    fn refresh(&mut self) {
        self.forget_content(None);
        self.start_reload(None, None, true);
    }

    fn confirm_quit(&mut self) {
        self.overlay = Some(Overlay::Confirm {
            message: "Quit Bjorn?".into(),
            confirm_label: "Quit".into(),
            action: Pending::Quit,
        });
    }

    fn run_pending(&mut self, action: Pending) {
        match action {
            Pending::Quit => self.running = false,
            Pending::Tick(rows) => self.tick_rows(rows),
            Pending::Trash(note) => {
                let client = self.client.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = client.trash(&note.id).await;
                    let _ = tx.send(Msg::Trashed { note, result });
                });
            }
        }
    }

    /// Cursor movement in the focused pane: `j`/`k`, the arrows.
    fn cursor(&mut self, delta: i32) {
        match self.focus {
            Pane::Sidebar => {
                if self.sidebar.move_cursor(delta) {
                    self.sidebar_announce(false);
                }
            }
            Pane::Notes => {
                if self.notes.move_cursor(delta)
                    && let Some(note) = self.notes.current().cloned()
                {
                    self.schedule_preview(note, false, false);
                }
            }
            Pane::Reader => {
                self.reader
                    .scroll_by(delta as i64, self.reader_width, self.reader_height)
            }
            Pane::Search => {}
        }
    }

    /// `enter` on a row: into the reader, at the first match when searching.
    fn open_current(&mut self) {
        if self.notes.current().is_none() {
            return;
        }
        if self.reader.pattern.is_some() {
            self.reader.reset_match_cursor();
            self.reader.jump(1, self.reader_width);
        }
        self.set_focus(Pane::Reader);
    }

    // -- actions: writes -----------------------------------------------------------

    fn new_note(&mut self) {
        let default_tags = if self.selection.tag.is_empty() {
            self.selection.workspace.clone()
        } else {
            self.selection.tag.clone()
        };
        self.overlay = Some(Overlay::NewNote {
            title: Field::new(""),
            tags: Field::new(&default_tags),
            field: 0,
        });
    }

    fn create_note(&mut self, title: String, tags: String) {
        let tag_list: Vec<String> = tags
            .split(',')
            .map(normalize_tag)
            .filter(|t| !t.is_empty())
            .collect();
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.create(&title, &tag_list, "").await;
            let _ = tx.send(Msg::Created { result });
        });
    }

    /// Round-trip the note through the editor, writing back hash-guarded.
    pub fn edit_note(&mut self, note: Note) {
        if note.locked {
            self.notify_titled(
                "",
                "Locked notes cannot be edited here.",
                Severity::Warning,
                Duration::from_secs(5),
            );
            return;
        }
        let editor = resolve_editor(&self.config, &|name| self.env(name));
        if !editor_available(&editor) {
            self.notify_titled(
                "",
                &format!("Editor “{editor}” not found. Set $EDITOR or `editor` in config."),
                Severity::Error,
                Duration::from_secs(10),
            );
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        let id = note.id.clone();
        tokio::spawn(async move {
            let result = client.cat(&id).await;
            let _ = tx.send(Msg::EditContent { note, result });
        });
    }

    fn open_editor(&mut self, note: Note, before: NoteContent) {
        let editor = resolve_editor(&self.config, &|name| self.env(name));
        let job = match editor::prepare(&note, &before, &editor) {
            Ok(job) => job,
            Err(err) => {
                self.notify_titled(
                    "Edit failed",
                    &format!("Could not write the temp file: {err}"),
                    Severity::Error,
                    Duration::from_secs(10),
                );
                return;
            }
        };
        let output = self.tx.clone();
        let exited = self.tx.clone();
        let sink = (
            move || {
                let _ = output.send(Msg::EditorOutput);
            },
            move |status| {
                let _ = exited.send(Msg::EditorExited(status));
            },
        );
        let (cols, rows) = self.editor_viewport();
        match PtySession::spawn(&job.command, rows, cols, sink) {
            Ok(pty) => {
                self.busy = true;
                self.editing = Some(Editing { job, pty });
            }
            Err(err) => {
                editor::cleanup(&job);
                self.notify_titled(
                    "Edit failed",
                    &format!("Could not run the editor: {err}"),
                    Severity::Error,
                    Duration::from_secs(10),
                );
            }
        }
    }

    /// The pane the editor draws into, as (cols, rows).
    fn editor_viewport(&self) -> (u16, u16) {
        let (width, height) = self.reader_viewport();
        (
            width.min(u16::MAX as usize) as u16,
            height.min(u16::MAX as usize) as u16,
        )
    }

    /// The editor process ended: read the file back and write it to Bear
    /// if it changed.
    fn editor_exited(&mut self, status: std::io::Result<portable_pty::ExitStatus>) {
        let Some(Editing { job, .. }) = self.editing.take() else {
            return;
        };
        self.busy = false;
        if let Err(err) = status {
            self.notify_titled(
                "Edit failed",
                &format!("Could not run the editor: {err}"),
                Severity::Error,
                Duration::from_secs(10),
            );
            return;
        }
        let after = match editor::result(&job) {
            Ok(text) => text,
            Err(err) => {
                self.notify_titled(
                    "",
                    &format!("Could not read the edited file: {err}"),
                    Severity::Error,
                    Duration::from_secs(10),
                );
                return;
            }
        };
        if after == job.before.content {
            editor::cleanup(&job);
            self.notify("No changes.", Duration::from_secs(2));
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client
                .overwrite(&job.note.id, &after, &job.before.hash)
                .await;
            let _ = tx.send(Msg::Written { job, result });
        });
    }

    /// Stop an editor that is still running, for shutdown.
    pub fn shutdown(&mut self) {
        if let Some(editing) = self.editing.as_mut() {
            editing.pty.kill();
        }
    }

    fn on_written(&mut self, job: EditorJob, result: Result<(), BearError>) {
        match result {
            Err(err) if err.is_conflict() => self.notify_titled(
                "Edit conflict",
                &format!("The note changed in Bear while you were editing. Nothing was written; your version is at {}", job.tmp.display()),
                Severity::Error,
                Duration::from_secs(30),
            ),
            Err(err) => self.notify_titled("Write failed", &format!("{err} — your version is at {}", job.tmp.display()), Severity::Error, Duration::from_secs(30)),
            Ok(()) => {
                editor::cleanup(&job);
                self.forget_content(Some(&job.note.id));
                self.note_written(&job.note.title);
                self.start_reload(Some(job.note.id.clone()), None, true);
                self.notify("Saved to Bear.", Duration::from_secs(2));
            }
        }
    }

    fn trash_note(&mut self) {
        let Some(note) = self.current_note().cloned() else {
            self.notify("No note selected.", Duration::from_secs(3));
            return;
        };
        if self.selection.view == View::Trash {
            self.notify(
                "Already in the trash. Bear empties the trash itself.",
                Duration::from_secs(4),
            );
            return;
        }
        self.overlay = Some(Overlay::Confirm {
            message: format!("Move “{}” to the trash?", note.title),
            confirm_label: "Trash".into(),
            action: Pending::Trash(note),
        });
    }

    fn restore_note(&mut self) {
        let Some(note) = self.current_note().cloned() else {
            return;
        };
        if !matches!(self.selection.view, View::Trash | View::Archive) {
            self.notify(
                "Restore works in the Trash and Archive views.",
                Duration::from_secs(3),
            );
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.restore(&note.id).await;
            let _ = tx.send(Msg::Restored { note, result });
        });
    }

    fn toggle_pin(&mut self) {
        let Some(current) = self.current_note().cloned() else {
            return;
        };
        // The list item may predate the last reload; decide from the snapshot.
        let note = self.snapshot.by_id(&current.id).cloned().unwrap_or(current);
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = if note.pinned_globally() {
                client.unpin(&note.id, "global").await
            } else {
                client.pin(&note.id, "global").await
            };
            let _ = tx.send(Msg::Pinned {
                note_id: note.id,
                result,
            });
        });
    }

    fn open_in_bear(&mut self) {
        let Some(note) = self.current_note().cloned() else {
            return;
        };
        self.open_note_in_bear(&note.id, "");
    }

    pub fn open_note_in_bear(&mut self, note_id: &str, header: &str) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        let (id, header) = (note_id.to_string(), header.to_string());
        tokio::spawn(async move {
            let _ = tx.send(Msg::Opened(client.open_in_app(&id, &header).await));
        });
    }

    fn export_note_action(&mut self) {
        let Some(note) = self.current_note().cloned() else {
            self.notify("No note selected.", Duration::from_secs(3));
            return;
        };
        if note.locked {
            self.notify_titled(
                "",
                "Locked notes cannot be exported here.",
                Severity::Warning,
                Duration::from_secs(5),
            );
            return;
        }
        let index = FORMATS
            .iter()
            .position(|f| f.id == self.config.export_format)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Format { index, note });
    }

    fn choose_format(&mut self, fmt: Format, note: Note) {
        let default = default_export_path(
            &self.config.export_dir,
            &note.title,
            extension_for(fmt, note.attachments > 0),
        );
        self.overlay = Some(Overlay::Text {
            title: format!("Export as {} to", fmt.label),
            field: Field::new(&default.to_string_lossy()),
            hint: "enter to write · esc to cancel".into(),
            purpose: TextPurpose::ExportPath {
                format_id: fmt.id,
                note,
            },
        });
    }

    fn export_to(&mut self, format_id: &str, note: Note, target: &Path) {
        let fmt = format_by_id(format_id);
        let client = self.client.clone();
        let tx = self.tx.clone();
        let target = target.to_path_buf();
        tokio::spawn(async move {
            let outcome: Result<PathBuf, String> = async {
                let content = client.cat(&note.id).await.map_err(|e| e.to_string())?;
                let mut images: HashMap<String, Vec<u8>> = HashMap::new();
                if fmt.needs_attachments && note.attachments > 0 {
                    for name in client
                        .attachments(&note.id)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        let bytes = client
                            .attachment(&note.id, &name)
                            .await
                            .map_err(|e| e.to_string())?;
                        images.insert(name, bytes);
                    }
                }
                let title = note.title.clone();
                tokio::task::spawn_blocking(move || {
                    export_note(fmt, &content.content, &title, &target, &images).map_err(|e| e.0)
                })
                .await
                .map_err(|e| e.to_string())?
            }
            .await;
            let _ = tx.send(Msg::Exported(outcome));
        });
    }

    // -- input ---------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if let Some(editing) = self.editing.as_mut() {
            editing.pty.send_key(key);
            return;
        }
        if let Some(overlay) = self.overlay.clone() {
            self.handle_overlay_key(overlay, key);
            return;
        }
        if self.triage.is_some() {
            self.triage_key(key);
            return;
        }
        if self.focus == Pane::Search {
            self.search_key(key);
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => self.confirm_quit(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help { scroll: 0 }),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Esc => self.clear_search(),
            KeyCode::Char('n') => self.new_note(),
            KeyCode::Char('e') => {
                if let Some(note) = self.current_note().cloned() {
                    self.edit_note(note);
                } else {
                    self.notify("No note selected.", Duration::from_secs(3));
                }
            }
            KeyCode::Char('d') => self.trash_note(),
            KeyCode::Char('u') => self.restore_note(),
            KeyCode::Char('p') => self.toggle_pin(),
            KeyCode::Char('x') => self.export_note_action(),
            KeyCode::Char('b') => self.open_in_bear(),
            KeyCode::Char('t') => self.open_triage(),
            KeyCode::Char('c') => self.cycle_columns(),
            KeyCode::Char('w') => self.toggle_workspace(),
            KeyCode::Char('W') => {
                if !self.selection.workspace.is_empty() {
                    self.set_workspace("");
                }
            }
            KeyCode::Char('f') => self.fold_tag(),
            KeyCode::Char('F') => self.fold_all(),
            KeyCode::Char(']') => self.jump_match(1),
            KeyCode::Char('[') => self.jump_match(-1),
            KeyCode::Char('j') | KeyCode::Down => self.cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.cursor(-1),
            KeyCode::PageDown => {
                if self.focus == Pane::Reader {
                    self.reader.scroll_by(
                        self.reader_height as i64,
                        self.reader_width,
                        self.reader_height,
                    );
                }
            }
            KeyCode::PageUp => {
                if self.focus == Pane::Reader {
                    self.reader.scroll_by(
                        -(self.reader_height as i64),
                        self.reader_width,
                        self.reader_height,
                    );
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if self.focus == Pane::Reader {
                    self.reader
                        .scroll_to_end(self.reader_width, self.reader_height);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if self.focus == Pane::Reader {
                    self.reader.scroll = 0;
                }
            }
            KeyCode::Tab => self.focus_next(1),
            KeyCode::BackTab => self.focus_next(-1),
            KeyCode::Enter => match self.focus {
                Pane::Notes => self.open_current(),
                Pane::Sidebar => self.sidebar_announce(false),
                Pane::Reader | Pane::Search => {}
            },
            KeyCode::Char(c) if c.is_ascii_digit() && !shift => {
                if let Some(view) = c.to_digit(10).and_then(|d| View::ALL.get(d as usize - 1)) {
                    self.action_view(*view);
                }
            }
            _ => {}
        }
    }

    fn field_key(field: &mut Field, key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => field.insert(c),
            KeyCode::Backspace => field.backspace(),
            KeyCode::Delete => field.delete(),
            KeyCode::Left => field.left(),
            KeyCode::Right => field.right(),
            KeyCode::Home => field.home(),
            KeyCode::End => field.end(),
            _ => return false,
        }
        true
    }

    fn handle_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) {
        match overlay {
            Overlay::Confirm { action, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') => self.overlay = None,
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.overlay = None;
                    self.run_pending(action);
                }
                _ => {}
            },
            Overlay::Help { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => self.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    self.overlay = Some(Overlay::Help { scroll: scroll + 1 })
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Help {
                        scroll: scroll.saturating_sub(1),
                    })
                }
                _ => {}
            },
            Overlay::Format { index, note } => {
                // A format's letter wins over h/l movement (h is HTML).
                if let KeyCode::Char(c) = key.code
                    && let Some(fmt) = FORMATS.iter().copied().find(|f| f.key == c)
                {
                    self.choose_format(fmt, note);
                    return;
                }
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Enter => self.choose_format(FORMATS[index], note),
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.overlay = Some(Overlay::Format {
                            index: (index + FORMATS.len() - 1) % FORMATS.len(),
                            note,
                        })
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        self.overlay = Some(Overlay::Format {
                            index: (index + 1) % FORMATS.len(),
                            note,
                        })
                    }
                    _ => {}
                }
            }
            Overlay::Text {
                title,
                mut field,
                hint,
                purpose,
            } => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Enter => {
                    self.overlay = None;
                    let value = field.value.trim().to_string();
                    if value.is_empty() {
                        return;
                    }
                    match purpose {
                        TextPurpose::ExportPath { format_id, note } => {
                            self.export_to(format_id, note, Path::new(&value))
                        }
                    }
                }
                _ => {
                    Self::field_key(&mut field, &key);
                    self.overlay = Some(Overlay::Text {
                        title,
                        field,
                        hint,
                        purpose,
                    });
                }
            },
            Overlay::NewNote {
                mut title,
                mut tags,
                mut field,
            } => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Tab | KeyCode::BackTab => {
                    self.overlay = Some(Overlay::NewNote {
                        title,
                        tags,
                        field: 1 - field,
                    })
                }
                KeyCode::Enter => {
                    let title_text = title.value.trim().to_string();
                    if field == 0 {
                        if !title_text.is_empty() {
                            field = 1;
                        }
                        self.overlay = Some(Overlay::NewNote { title, tags, field });
                    } else if title_text.is_empty() {
                        self.overlay = Some(Overlay::NewNote {
                            title,
                            tags,
                            field: 0,
                        });
                    } else {
                        self.overlay = None;
                        self.create_note(title_text, tags.value.trim().to_string());
                    }
                }
                _ => {
                    Self::field_key(if field == 0 { &mut title } else { &mut tags }, &key);
                    self.overlay = Some(Overlay::NewNote { title, tags, field });
                }
            },
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(editing) = self.editing.as_mut() {
            let pane = self.rects.editor;
            if mouse.column >= pane.x
                && mouse.column < pane.right()
                && mouse.row >= pane.y
                && mouse.row < pane.bottom()
            {
                editing
                    .pty
                    .send_mouse(mouse, mouse.column - pane.x, mouse.row - pane.y);
            }
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        let (x, y) = (mouse.column, mouse.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if inside(self.rects.glyph) {
                    self.cycle_columns();
                } else if self.columns == 3 && inside(self.rects.sidebar_rows) {
                    let index = self.sidebar.scroll + (y - self.rects.sidebar_rows.y) as usize;
                    self.focus = Pane::Sidebar;
                    if self.sidebar.click(index) {
                        self.sidebar_announce(false);
                    }
                } else if self.columns >= 2
                    && self.notes.search.open
                    && inside(self.rects.search_box)
                {
                    self.focus = Pane::Search;
                } else if self.columns >= 2 && inside(self.rects.notes_rows) {
                    let index =
                        self.notes.scroll + (y - self.rects.notes_rows.y) as usize / ROW_HEIGHT;
                    self.focus = Pane::Notes;
                    if self.notes.select_index(index)
                        && let Some(note) = self.notes.current().cloned()
                    {
                        self.schedule_preview(note, false, false);
                    }
                } else if inside(self.rects.reader_body) {
                    self.focus = Pane::Reader;
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if mouse.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if inside(self.rects.reader_body) {
                    self.reader
                        .scroll_by(delta, self.reader_width, self.reader_height);
                } else if inside(self.rects.notes_rows) {
                    let rows = self.notes.len();
                    self.notes.scroll = (self.notes.scroll as i64 + delta / 3)
                        .clamp(0, rows.saturating_sub(1) as i64)
                        as usize;
                } else if inside(self.rects.sidebar_rows) {
                    let rows = self.sidebar.rows.len();
                    self.sidebar.scroll = (self.sidebar.scroll as i64 + delta)
                        .clamp(0, rows.saturating_sub(1) as i64)
                        as usize;
                }
            }
            _ => {}
        }
    }

    /// The reader's viewport as last drawn; the draw code reports it here.
    pub fn set_reader_viewport(&mut self, width: usize, height: usize) {
        self.reader_width = width.max(1);
        self.reader_height = height.max(1);
    }

    pub fn reader_viewport(&self) -> (usize, usize) {
        (self.reader_width, self.reader_height)
    }
}

// -- triage ------------------------------------------------------------------------

impl App {
    pub fn reminders_enabled(&self) -> bool {
        self.config.reminders.enabled && self.remctl.is_some()
    }

    /// `t`: every open todo in the workspace (all notes when none is set).
    pub fn open_triage(&mut self) {
        if self.triage.is_some() {
            return;
        }
        let scope = if self.selection.workspace.is_empty() {
            "all notes".to_string()
        } else {
            display_tag(&self.selection.workspace)
        };
        if self.config.reminders.enabled && self.remctl.is_none() && !self.reminders_notice_shown {
            self.reminders_notice_shown = true;
            self.notify_titled(
                "Reminders",
                "Reminders mode is configured but remctl was not found; triage runs Bear-only.",
                Severity::Warning,
                Duration::from_secs(6),
            );
        }
        self.triage = Some(Triage::new(&scope, self.reminders_enabled()));
        self.triage_load();
    }

    fn triage_load(&mut self) {
        let client = self.client.clone();
        let remctl = if self.reminders_enabled() {
            self.remctl.clone()
        } else {
            None
        };
        let tx = self.tx.clone();
        let workspace = self.selection.workspace.clone();
        tokio::spawn(async move {
            let rows = match client.todo_rows(&workspace).await {
                Ok(rows) => rows,
                Err(err) => {
                    let _ = tx.send(Msg::TriageLoaded {
                        scan: TodoScan::default(),
                        statuses: HashMap::new(),
                        error: format!("Triage: {err}"),
                    });
                    return;
                }
            };
            let scan = scan_rows(&rows);
            let mut statuses = HashMap::new();
            let mut error = String::new();
            if let Some(remctl) = remctl {
                match remctl.linked_reminders().await {
                    Ok(reminders) => statuses = join_reminders(&scan.todos, &reminders),
                    Err(err) => error = format!("Reminders unavailable: {err}"),
                }
            }
            let _ = tx.send(Msg::TriageLoaded {
                scan,
                statuses,
                error,
            });
        });
    }

    fn close_triage(&mut self) {
        self.triage = None;
    }

    fn triage_key(&mut self, key: KeyEvent) {
        let Some(triage) = self.triage.as_mut() else {
            return;
        };
        if let Some(field) = triage.filter.as_mut() {
            match key.code {
                KeyCode::Esc => triage.close_filter(),
                KeyCode::Enter => triage.apply_filter(),
                _ => {
                    Self::field_key(field, &key);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if !triage.filter_text.is_empty() {
                    triage.clear_filter();
                } else {
                    self.close_triage();
                }
            }
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help { scroll: 0 }),
            KeyCode::Char('j') | KeyCode::Down => triage.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => triage.move_cursor(-1),
            KeyCode::Char(' ') => triage.toggle_mark(),
            KeyCode::Char('/') => triage.open_filter(),
            KeyCode::Char('r') => {
                triage.status = "reloading…".into();
                self.triage_load();
            }
            KeyCode::Char('x') => {
                let rows = triage.targets();
                if rows.is_empty() {
                    return;
                }
                if rows.len() > 1 {
                    self.overlay = Some(Overlay::Confirm {
                        message: format!("Tick {} todos in Bear?", rows.len()),
                        confirm_label: "Tick".into(),
                        action: Pending::Tick(rows),
                    });
                } else {
                    self.tick_rows(rows);
                }
            }
            KeyCode::Enter => {
                if let Some(row) = triage.current_row().cloned() {
                    self.triage_goto(&row.todo.note_id);
                }
            }
            KeyCode::Char('b') => {
                if let Some(row) = triage.current_row().cloned() {
                    let header = if row.todo.section.starts_with("# ") {
                        String::new()
                    } else {
                        row.todo.header()
                    };
                    self.open_note_in_bear(&row.todo.note_id, &header);
                }
            }
            KeyCode::Char('a') => self.triage_add(),
            _ => {}
        }
    }

    fn tick_rows(&mut self, rows: Vec<TriageRow>) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut ticked = Vec::new();
            let mut failures = Vec::new();
            for row in rows {
                let todo = row.todo;
                match client
                    .tick_todo(&todo.note_id, &todo.line, &todo.done_line(), &todo.section)
                    .await
                {
                    Ok(()) => ticked.push(todo.key()),
                    Err(err) => failures.push(format!(
                        "{}: {err}",
                        todo.text.chars().take(40).collect::<String>()
                    )),
                }
            }
            let _ = tx.send(Msg::TriageTicked { ticked, failures });
        });
    }

    fn on_triage_ticked(&mut self, ticked: Vec<String>, failures: Vec<String>) {
        if let Some(triage) = self.triage.as_mut()
            && !ticked.is_empty()
        {
            triage.note_removed(&ticked);
        }
        if !failures.is_empty() {
            self.notify_titled(
                "Some todos were not ticked (the line changed in Bear?)",
                &failures.join("\n"),
                Severity::Warning,
                Duration::from_secs(10),
            );
        } else if !ticked.is_empty() {
            self.notify(
                &format!("Ticked {} in Bear.", ticked.len()),
                Duration::from_secs(2),
            );
        }
        self.start_reload(None, None, false);
        if self.triage.is_some() {
            self.triage_load();
        }
    }

    fn triage_goto(&mut self, note_id: &str) {
        self.close_triage();
        if !self.notes.select_id(note_id) {
            self.drop_search();
            self.selection = Selection {
                view: View::All,
                workspace: self.selection.workspace.clone(),
                ..Selection::default()
            };
            self.sidebar.select_view(View::All);
            self.apply_selection(Some(note_id), false);
            self.notes.select_id(note_id);
        }
        self.focus = Pane::Notes;
        if let Some(note) = self.notes.current().cloned() {
            self.schedule_preview(note, true, false);
        }
    }

    fn triage_add(&mut self) {
        let Some(triage) = self.triage.as_mut() else {
            return;
        };
        if !triage.reminders_enabled {
            self.notify_titled(
                "Add to Reminders",
                "Reminders mode is off. Add to ~/.config/bjorn/config.toml:\n[reminders]\nenabled = true\nlist = \"<your list>\"",
                Severity::Warning,
                Duration::from_secs(8),
            );
            return;
        }
        let rows: Vec<TriageRow> = triage
            .targets()
            .into_iter()
            .filter(|r| r.status == Status::New)
            .collect();
        if rows.is_empty() {
            self.notify(
                "Mark rows that are not in Reminders yet.",
                Duration::from_secs(3),
            );
            return;
        }
        let Some(remctl) = self.remctl.clone() else {
            return;
        };
        let tx = self.tx.clone();
        let (list, due) = (
            self.config.reminders.list.clone(),
            self.config.reminders.due.clone(),
        );
        tokio::spawn(async move {
            let mut added = 0;
            let mut failures = Vec::new();
            let mut keys = Vec::new();
            for row in rows {
                keys.push(row.todo.key());
                match remctl.add(&row.todo, &list, &due).await {
                    Ok(_) => added += 1,
                    Err(err) => failures.push(format!(
                        "{}: {err}",
                        row.todo.text.chars().take(40).collect::<String>()
                    )),
                }
            }
            let _ = tx.send(Msg::TriageAdded {
                added,
                failures,
                keys,
            });
        });
    }

    fn on_triage_added(&mut self, added: usize, failures: Vec<String>, keys: Vec<String>) {
        if !failures.is_empty() {
            self.notify_titled(
                "Some reminders were not created",
                &failures.join("\n"),
                Severity::Warning,
                Duration::from_secs(10),
            );
        }
        if added > 0 {
            let list = self.config.reminders.list.clone();
            let where_ = if list.is_empty() {
                String::new()
            } else {
                format!(" to “{list}”")
            };
            self.notify(
                &format!(
                    "Added {added} reminder{}{where_}.",
                    if added == 1 { "" } else { "s" }
                ),
                Duration::from_secs(3),
            );
        }
        if let Some(triage) = self.triage.as_mut() {
            triage.unmark(&keys);
            self.triage_load();
        }
    }
}
