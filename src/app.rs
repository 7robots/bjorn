//! The Bjorn application: three panes over one bearcli snapshot.
//!
//! One `App` owns every piece of state and is driven by three inputs: terminal
//! events, `Msg` values sent back by the tokio tasks that talk to bearcli, and
//! `tick` for timers. Drawing reads the state; nothing here touches the
//! terminal. Results are tagged with a generation number and a stale one is
//! dropped, never canceled mid-flight.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::actions::{self, Action};
use crate::bear::{
    BearClient, BearError, Location, Note, NoteContent, Probe, Snapshot, display_tag,
    normalize_tag, recently_modified,
};
use crate::config::{Config, editor_available, resolve_editor};
use crate::daily;
use crate::editor::{self, EditorJob};
use crate::export::{
    FORMATS, Format, default_export_path, export_note, extension_for, format_by_id,
};
use crate::icons::IconSet;
use crate::model::{Selection, View, duplicate_titles, in_workspace, select_notes, today};
use crate::pty::PtySession;
use crate::reminders::{
    RemctlClient, Status, join as join_reminders, remctl_found, resolve_remctl,
};
use crate::search::{query_pattern, rewrite_subtags};
use crate::templates::{self, Template};
use crate::todos::{TodoScan, scan_rows};
use crate::ui::modals::{
    Field, LinkRow, LinkTarget, Overlay, Pending, Severity, TextPurpose, Toast, filter_links,
    filter_templates,
};
use crate::ui::note_list::{NoteList, ROW_HEIGHT};
use crate::ui::note_view::{Reader, outline_filter};
use crate::ui::sidebar::{Row, Sidebar};
use crate::ui::triage::{Triage, TriageRow};
use crate::util::strip_control;
use crate::wiki::{self, Backlinks, WikiLink};

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
/// What a screen says when the note behind a row cannot be shown any more.
pub const GONE_FROM_THE_LIST: &str = "That note is no longer in the list — press r to refresh.";
/// Notes kept in the back (and the forward) history.
pub const HISTORY_LIMIT: usize = 100;
/// Outgoing links listed by `L`; a note with more says how many are left out.
pub const OUTGOING_LIMIT: usize = 1000;

/// A place in the reading history: the note, the list it was read from, and
/// how far down it was scrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub id: String,
    pub selection: Selection,
    pub query: String,
    pub scroll: usize,
}

/// Where the reader should land once a note it is waiting for is drawn.
#[derive(Debug, Clone, PartialEq)]
enum Jump {
    /// A heading, from `[[Title/Heading]]`.
    Section(String),
    /// A scroll offset, going back or forward.
    Scroll(usize),
}

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
    /// Today's note was found or made: select it and show it, no editor.
    DailyReady {
        title: String,
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
    /// An interactive action is ready to start, or could not be prepared.
    /// `generation` is the start it answers; see `App::session_gen`.
    SessionReady {
        generation: u64,
        action: Box<Action>,
        payload: Result<Box<crate::actions::Payload>, String>,
    },
    /// The interactive action drew something; a redraw is due.
    SessionOutput,
    /// The interactive action's command ended.
    SessionExited(std::io::Result<portable_pty::ExitStatus>),
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
    /// An action finished: the name it ran under, and what it said.
    ActionDone {
        name: String,
        result: Result<String, String>,
    },
    /// An action whose `output` goes to Bear finished: what became of the
    /// output, or why the command failed (in which case nothing was written).
    ActionWrote {
        action: Box<Action>,
        note: Note,
        result: Result<ActionWrite, String>,
    },
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
    /// The notes linking to `note_id`, checked, for the Links list. Only
    /// the answer to the latest search (`generation`) is shown.
    Backlinks {
        generation: u64,
        note_id: String,
        result: Result<Backlinks, BearError>,
    },
}

/// What became of the output of an action that writes to Bear.
#[derive(Debug)]
pub enum ActionWrite {
    Appended,
    Created {
        id: String,
        title: String,
    },
    /// The note was replaced; `title` is what Bear calls it now, and `backup`
    /// holds its text from before.
    Replaced {
        title: String,
        backup: PathBuf,
    },
    /// Nothing was written, and why: the command printed nothing, too much or
    /// not text, or the note went to the trash. `kept` is `None` when there
    /// was no output worth keeping, else where it went or why it could not be
    /// saved.
    Skipped {
        why: String,
        kept: Option<std::io::Result<PathBuf>>,
    },
    /// The write failed. `kept` is where the output went, or why it could not
    /// be saved. `backup` is a `replace`'s copy of the note's text, kept when
    /// the failure cannot prove Bear left the note alone.
    Failed {
        message: String,
        conflict: bool,
        kept: std::io::Result<PathBuf>,
        backup: Option<PathBuf>,
    },
}

/// The end of a toast about output that did not reach Bear: where it is, or
/// that it is gone and why. Said once, so a failed save is never left out.
fn kept_phrase(kept: &std::io::Result<PathBuf>) -> String {
    match kept {
        Ok(path) => format!("the output is at {}", path.display()),
        Err(e) => format!("the output could not be saved either: {e}"),
    }
}

/// The toast for output that was never sent to Bear.
fn skipped_message(why: &str, kept: Option<&std::io::Result<PathBuf>>) -> String {
    match kept {
        Some(Ok(path)) => format!(
            "It ran, but {why}, so nothing was written to Bear; the output is at {}",
            path.display()
        ),
        Some(Err(e)) => format!(
            "It ran, but {why}, so nothing was written to Bear, and the output could not be saved: {e}"
        ),
        None => format!("It ran, but {why}, so nothing was written to Bear."),
    }
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
    /// The whole frame, for an interactive action that fills it.
    pub window: Rect,
}

/// An editor open in the reader pane.
pub struct Editing {
    pub job: EditorJob,
    pub pty: PtySession,
}

/// An `interactive = true` action, running in a pty with the window and the
/// keyboard, until its command exits.
pub struct Session {
    pub name: String,
    pub pty: PtySession,
    /// Holds the rendered note on disk for as long as the command can read it.
    pub payload: crate::actions::Payload,
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
    pub session: Option<Session>,
    /// An interactive action whose note is still being rendered: its name.
    /// Keys are dropped meanwhile (`esc` cancels) so nothing opens underneath
    /// the session.
    session_starting: Option<String>,
    /// Counts interactive starts. A start that was called off can still have
    /// its note rendering; when it finishes, its `SessionReady` must not be
    /// taken for the start that is pending now, which may be for another
    /// note, so each start is numbered and only the latest may run.
    session_gen: u64,
    /// A note just created, to open in the editor once the reload shows it.
    pending_edit: Option<String>,
    /// A note to select and show once the reload has it (today's, after `D`).
    pending_reveal: Option<String>,
    /// The triage screen while it is up; it covers the three columns.
    pub triage: Option<Triage>,
    pub remctl: Option<Arc<RemctlClient>>,
    reminders_notice_shown: bool,
    reader_width: usize,
    reader_height: usize,
    /// Notes left by following a link, most recent last; `backspace` returns.
    pub back: Vec<Place>,
    /// Notes left by going back, most recent last.
    pub forward: Vec<Place>,
    /// Where to land in a note once the reader draws it.
    pending_jump: Option<(String, Jump)>,
    /// Snapshot positions by lowercased title, for following links.
    title_index: HashMap<String, Vec<usize>>,
    /// The latest backlink search; only its answer is shown.
    backlinks_gen: u64,
    /// The note whose backlinks are being searched for, if any.
    backlinks_running: Option<String>,
    /// A Links list opened while that search ran, for another note.
    backlinks_queued: Option<Note>,
    /// The search box shows a query put back by `backspace`/`alt+→` and not
    /// touched since; `backspace` keeps going back through history.
    search_restored: bool,
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
        // gate, the test harness) picks it up from the config it was given. An
        // unknown name keeps the default: the config file is shared with the
        // Python Bjorn, which knows themes this build does not.
        let theme_known = crate::ui::theme::set(&config.theme);
        if !theme_known {
            crate::ui::theme::set(crate::ui::theme::DEFAULT_THEME);
        }
        let unknown_theme = (!theme_known).then(|| config.theme.clone());
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
            session: None,
            session_starting: None,
            session_gen: 0,
            pending_edit: None,
            pending_reveal: None,
            triage: None,
            remctl,
            reminders_notice_shown: false,
            reader_width: 80,
            reader_height: 24,
            back: Vec::new(),
            forward: Vec::new(),
            pending_jump: None,
            title_index: HashMap::new(),
            backlinks_gen: 0,
            backlinks_running: None,
            backlinks_queued: None,
            search_restored: false,
        };
        app.reader.clear("Loading\u{2026}");
        if let Some(name) = unknown_theme {
            app.notify_titled(
                "Theme",
                &format!(
                    "Unknown theme {name:?}; drawing with {}. This build knows: {}.",
                    crate::ui::theme::DEFAULT_THEME,
                    crate::ui::theme::names().collect::<Vec<_>>().join(", ")
                ),
                Severity::Warning,
                Duration::from_secs(10),
            );
        }
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
                // `D` asked for a note this reload was to show; the moment
                // has passed, so a later reload must not jump to it.
                self.pending_reveal = None;
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
        self.rebuild_title_index();
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
        if let Some(id) = self.pending_reveal.take() {
            if self.reveal_note(&id) {
                // Said only once the note is on screen. The title is Bear's,
                // so it is shown without control characters.
                let title: String = self
                    .notes
                    .current()
                    .map(|n| n.title.chars().filter(|c| !c.is_control()).collect())
                    .unwrap_or_default();
                self.notify(&format!("Today: “{title}”"), Duration::from_secs(3));
            } else {
                self.notify(GONE_FROM_THE_LIST, Duration::from_secs(5));
            }
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
            Msg::DailyReady { title, result } => match result {
                Ok(id) => {
                    // A second note by this title is what a race with another
                    // process leaves behind; the reload below warns about it.
                    self.note_written(&title);
                    self.pending_reveal = Some(id);
                    self.start_reload(None, None, false);
                }
                Err(err) => self.error("Daily note failed", &err),
            },
            Msg::EditContent { note, result } => match result {
                Ok(before) => self.open_editor(note, before),
                Err(err) => self.error("Read failed", &err),
            },
            Msg::Written { job, result } => self.on_written(job, result),
            Msg::SessionReady {
                generation,
                action,
                payload,
            } => self.start_session(generation, *action, payload),
            Msg::SessionOutput => {}
            Msg::SessionExited(status) => self.session_exited(status),
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
            Msg::ActionDone { name, result } => match result {
                Ok(output) => self.notify_titled(
                    &name,
                    if output.is_empty() { "Done." } else { &output },
                    Severity::Information,
                    Duration::from_secs(6),
                ),
                Err(message) => self.notify_titled(
                    &format!("{name} failed"),
                    &message,
                    Severity::Error,
                    Duration::from_secs(10),
                ),
            },
            Msg::ActionWrote {
                action,
                note,
                result,
            } => self.on_action_wrote(*action, note, result),
            Msg::Backlinks {
                generation,
                note_id,
                result,
            } => self.on_backlinks(generation, &note_id, result),
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
        // A jump waiting for another note is moot once the reader moves on.
        if self
            .pending_jump
            .as_ref()
            .is_some_and(|(id, _)| *id != note.id)
        {
            self.pending_jump = None;
        }
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
            self.apply_pending_jump();
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
                self.apply_pending_jump();
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
        self.search_restored = false;
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
        self.search_restored = false;
        let query = self.search_query.clone();
        self.notes.search.open_with(&query);
        let tags = self.query_tags();
        self.notes.search.refresh_suggestion(&tags);
        self.focus = Pane::Search;
    }

    /// `esc`: close the box and drop the search and its highlights.
    pub fn clear_search(&mut self) {
        self.search_restored = false;
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
        self.search_restored = false;
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

    /// `o`: the outline of the note in the reader, the current section
    /// highlighted.
    fn open_outline(&mut self) {
        if self.reader.headings.is_empty() {
            let message = if self.reader.note.is_none() {
                "No note in the reader."
            } else if self.reader.full_text.is_some() {
                "This note has no headings."
            } else {
                "This note cannot be read, so it has no outline."
            };
            self.notify(message, Duration::from_secs(3));
            return;
        }
        let index = self
            .reader
            .current_heading(self.reader_width, self.reader_height)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Outline {
            field: Field::default(),
            index,
        });
    }

    /// `}` / `{`: the next or previous heading in the reader.
    fn jump_heading(&mut self, delta: i64) {
        if self.reader.headings.is_empty() {
            return;
        }
        self.reader
            .jump_heading(delta, self.reader_width, self.reader_height);
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
            Pending::RunAction(action, note, input) => self.spawn_action(action, note, input),
            Pending::DeleteAction(action, note) => self.delete_action(action, note),
            Pending::CreateLinked(title) => {
                self.push_history();
                let tags = self.new_note_tags();
                self.create_note(title, tags, String::new());
            }
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

    /// The tags a new note starts with: the selected tag, else the workspace.
    fn new_note_tags(&self) -> String {
        if self.selection.tag.is_empty() {
            self.selection.workspace.clone()
        } else {
            self.selection.tag.clone()
        }
    }

    fn new_note(&mut self) {
        self.new_note_from(None);
    }

    /// The new-note prompt, tags defaulting to the tag in view (else the
    /// workspace), the title to the template's heading when there is one.
    fn new_note_from(&mut self, template: Option<Template>) {
        let default_tags = self.new_note_tags();
        let title = template
            .as_ref()
            .map(|t| {
                t.title(
                    &Local::now(),
                    &self.selection.workspace,
                    &split_tags(&default_tags),
                )
            })
            .unwrap_or_default();
        self.overlay = Some(Overlay::NewNote {
            title: Field::new(&title),
            tags: Field::new(&default_tags),
            field: 0,
            template,
        });
    }

    /// `N`: pick a template from the templates directory, then the same
    /// title and tags prompt as `n`.
    /// The daily template is left out: it is `D`'s, and its title is the date.
    fn open_templates(&mut self) {
        let dir = &self.config.templates_dir;
        let daily = templates::path_for(dir, &self.config.daily.template);
        let mut listing = templates::list(dir);
        listing
            .templates
            .retain(|t| !templates::same_file(&t.path, &daily));
        self.overlay = Some(Overlay::Templates {
            field: Field::default(),
            index: 0,
            templates: listing.templates,
            skipped: listing.skipped,
        });
    }

    /// `D`: today's note, made from the daily template the first time. One
    /// already in the snapshot is shown without asking bearcli; otherwise
    /// `create --if-not-exists` finds or makes it.
    fn open_daily(&mut self) {
        let daily = match daily::today(&self.config, &Local::now()) {
            Ok(daily) => daily,
            Err(message) => {
                self.notify_titled(
                    "Daily note",
                    &message,
                    Severity::Error,
                    Duration::from_secs(10),
                );
                return;
            }
        };
        let wanted = daily.title.to_lowercase();
        let known = self
            .snapshot
            .notes
            .iter()
            .find(|n| n.location == Location::Notes && n.title.to_lowercase() == wanted)
            .map(|n| n.id.clone());
        if let Some(id) = known {
            if !self.reveal_note(&id) {
                self.notify(GONE_FROM_THE_LIST, Duration::from_secs(5));
            }
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = daily::ensure(&client, &daily).await;
            let _ = tx.send(Msg::DailyReady {
                title: daily.title,
                result,
            });
        });
    }

    /// Select `id` in the notes list and show it in the reader. When the list
    /// in view does not hold it, widen to the view its location belongs in,
    /// leaving the workspace only when the note is outside it.
    ///
    /// False means the jump did not happen, and the caller must say so rather
    /// than report the note as shown. An id the snapshot does not hold is
    /// refused before anything is touched, so the view and the cursor stay
    /// where they were.
    pub fn reveal_note(&mut self, id: &str) -> bool {
        let Some(note) = self.snapshot.by_id(id).cloned() else {
            return false;
        };
        if !self.notes.select_id(id) {
            if !in_workspace(&note, &self.selection.workspace) {
                self.set_workspace("");
            }
            self.action_view(match note.location {
                Location::Trash => View::Trash,
                Location::Archive => View::Archive,
                _ => View::All,
            });
            if !self.notes.select_id(id) {
                return false;
            }
        }
        if let Some(current) = self.notes.current().cloned() {
            self.schedule_preview(current, true, false);
        }
        true
    }

    fn create_note(&mut self, title: String, tags: String, content: String) {
        let tag_list = split_tags(&tags);
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = client.create(&title, &tag_list, &content).await;
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
        if let Some(running) = self
            .session
            .as_ref()
            .map(|s| s.name.clone())
            .or(self.session_starting.clone())
        {
            self.notify(
                &format!("“{running}” is running; the editor did not open."),
                Duration::from_secs(5),
            );
            return;
        }
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
        match PtySession::spawn(&job.command, rows, cols, &[], None, sink) {
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
    /// Render the note and describe the command's world, the same way a
    /// captured action does.
    async fn action_payload(
        client: &BearClient,
        action: &Action,
        note: &Note,
        input: &str,
    ) -> Result<crate::actions::Payload, String> {
        let fmt = crate::export::format_by_id(&action.format);
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
        actions::prepare(action, note, &content.content, &images, input)
            .await
            .map_err(|e| e.0)
    }

    /// Give an `interactive = true` action the window and the keyboard.
    fn start_session(
        &mut self,
        generation: u64,
        action: Action,
        payload: Result<Box<crate::actions::Payload>, String>,
    ) {
        // Canceled with `esc` while the note was being rendered, or canceled
        // and then replaced by a newer start: dropping the payload removes its
        // temp directory, and the pending start, if any, stays pending.
        if generation != self.session_gen || self.session_starting.take().is_none() {
            return;
        }
        if let Some(running) = self.running_session() {
            self.notify(
                &format!(
                    "“{running}” is still running; “{}” did not start.",
                    action.name
                ),
                Duration::from_secs(5),
            );
            return;
        }
        let payload = match payload {
            Ok(payload) => *payload,
            Err(err) => {
                self.notify_titled(
                    &format!("“{}” could not start", action.name),
                    &err,
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
                let _ = output.send(Msg::SessionOutput);
            },
            move |status| {
                let _ = exited.send(Msg::SessionExited(status));
            },
        );
        let (cols, rows) = self.session_viewport();
        let command = vec!["sh".to_string(), "-c".to_string(), action.command.clone()];
        match PtySession::spawn(
            &command,
            rows,
            cols,
            &payload.env,
            Some(payload.dir.path()),
            sink,
        ) {
            Ok(pty) => {
                self.busy = true;
                self.session = Some(Session {
                    name: action.name.clone(),
                    pty,
                    payload,
                });
            }
            Err(err) => self.notify_titled(
                &format!("“{}” could not start", action.name),
                &format!("Could not run the command: {err}"),
                Severity::Error,
                Duration::from_secs(10),
            ),
        }
    }

    /// The command ended: the window comes back, and a non-zero exit is said
    /// out loud because nothing was captured to say it.
    fn session_exited(&mut self, status: std::io::Result<portable_pty::ExitStatus>) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.busy = false;
        match status {
            Err(err) => self.notify_titled(
                &session.name,
                &format!("Could not run the command: {err}"),
                Severity::Error,
                Duration::from_secs(10),
            ),
            Ok(status) if !status.success() => self.notify_titled(
                &session.name,
                &format!("exit {}", status.exit_code()),
                Severity::Error,
                Duration::from_secs(10),
            ),
            Ok(_) => self.notify(
                &format!("“{}” finished.", session.name),
                Duration::from_secs(3),
            ),
        }
    }

    /// An interactive action gets the whole window: its program owns the screen,
    /// unlike the editor, which sits beside the note it is editing.
    fn session_viewport(&self) -> (u16, u16) {
        // The pane `draw` will give it, so a command that reads its size once
        // at startup lays out to the size it really gets.
        let pane = crate::ui::session_pane(self.rects.window);
        (pane.width.max(1), pane.height.max(1))
    }

    /// What is holding the terminal, if anything: an interactive action,
    /// running or being prepared, or the editor.
    fn running_session(&self) -> Option<String> {
        if let Some(session) = &self.session {
            return Some(session.name.clone());
        }
        if let Some(name) = &self.session_starting {
            return Some(name.clone());
        }
        self.editing.as_ref().map(|_| "the editor".to_string())
    }

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
        if let Some(session) = self.session.as_mut() {
            session.pty.kill();
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

    // -- actions -------------------------------------------------------------------

    /// The note an action would run on, or a toast saying why there is none.
    fn action_target(&mut self) -> Option<Note> {
        let Some(note) = self.current_note().cloned() else {
            self.notify("No note selected.", Duration::from_secs(3));
            return None;
        };
        if note.locked {
            self.notify_titled(
                "",
                "Locked notes cannot be sent to an action.",
                Severity::Warning,
                Duration::from_secs(5),
            );
            return None;
        }
        Some(note)
    }

    /// `a`: the menu, with the search box focused. With no actions configured
    /// it holds only the row that adds one.
    fn open_actions(&mut self) {
        let Some(note) = self.action_target() else {
            return;
        };
        self.overlay = Some(Overlay::Actions {
            field: Field::default(),
            index: 0,
            note,
        });
    }

    /// `!`: the default action straight away, or the menu when there is no
    /// single obvious one to run.
    fn run_default_action(&mut self) {
        let Some(action) = actions::default_action(&self.config.actions).cloned() else {
            self.open_actions();
            return;
        };
        let Some(note) = self.action_target() else {
            return;
        };
        self.start_action(action, note);
    }

    /// The config file this run read, which is where a new action is written.
    pub fn config_path(&self) -> PathBuf {
        self.config
            .path
            .clone()
            .unwrap_or_else(crate::config::default_config_path)
    }

    /// Write `action` into the config file, as a new entry or over `editing`,
    /// and read the actions back so the menu shows it at once. On failure the
    /// form stays up with what was typed.
    fn save_action(&mut self, action: Action, editing: Option<Action>, form: Overlay, note: Note) {
        let path = self.config_path();
        let written = match &editing {
            Some(original) => actions::update_in_config(&path, original, &action),
            None => actions::add_to_config(&path, &action),
        };
        let saved = written.and_then(|()| {
            Config::load(Some(&path)).map_err(|e| actions::ActionError(format!("{e:#}")))
        });
        match saved {
            Ok(loaded) => {
                self.config.actions = loaded.actions;
                let index = self
                    .config
                    .actions
                    .iter()
                    .rposition(|a| a.name == action.name && a.command == action.command)
                    .unwrap_or(0);
                let (title, verb) = if editing.is_some() {
                    ("Action updated", "updated")
                } else {
                    ("Action added", "saved")
                };
                self.notify_titled(
                    title,
                    &format!("“{}” is {verb} in {}.", action.name, path.display()),
                    Severity::Information,
                    Duration::from_secs(6),
                );
                self.overlay = Some(Overlay::Actions {
                    field: Field::default(),
                    index,
                    note,
                });
            }
            Err(err) => {
                self.notify_titled(
                    "Could not save the action",
                    &err.0,
                    Severity::Error,
                    Duration::from_secs(10),
                );
                self.overlay = Some(form);
            }
        }
    }

    /// Delete `action` from the config file, once confirmed, then go back to
    /// the menu with the actions read again.
    fn delete_action(&mut self, action: Action, note: Note) {
        let path = self.config_path();
        let position = self
            .config
            .actions
            .iter()
            .position(|a| a == &action)
            .unwrap_or(0);
        let removed = actions::remove_from_config(&path, &action).and_then(|()| {
            Config::load(Some(&path)).map_err(|e| actions::ActionError(format!("{e:#}")))
        });
        match removed {
            Ok(loaded) => {
                self.config.actions = loaded.actions;
                self.notify_titled(
                    "Action deleted",
                    &format!("“{}” is gone from {}.", action.name, path.display()),
                    Severity::Information,
                    Duration::from_secs(6),
                );
            }
            Err(err) => self.notify_titled(
                "Could not delete the action",
                &err.0,
                Severity::Error,
                Duration::from_secs(10),
            ),
        }
        // The highlight stays where the deleted row was.
        self.overlay = Some(Overlay::Actions {
            field: Field::default(),
            index: position.min(self.config.actions.len()),
            note,
        });
    }

    /// Run `action` on `note`. An action with a `prompt` asks for its line of
    /// text first; `confirm = true` then asks to go ahead, in that order, so
    /// the dialog can quote what was typed.
    fn start_action(&mut self, action: Action, note: Note) {
        if let Some(problem) = action.misconfigured() {
            self.notify_titled(
                &format!("{} did not run", action.name),
                &format!("{problem}."),
                Severity::Error,
                Duration::from_secs(10),
            );
            return;
        }
        if let Some(title) = action.prompt.clone() {
            self.overlay = Some(Overlay::Text {
                title,
                field: Field::default(),
                hint: "enter to run · esc to cancel".into(),
                purpose: TextPurpose::ActionInput { action, note },
            });
        } else {
            self.confirm_action(action, note, String::new());
        }
    }

    /// The confirm dialog when the action asks for one, else straight to it.
    fn confirm_action(&mut self, action: Action, note: Note, input: String) {
        if action.asks_first() {
            let target = match (input.is_empty(), action.prompt.is_some()) {
                // A prompt action acts on the answer, not on the note, so an
                // empty answer must not name the note the cursor happens to be on.
                (true, true) => "no input".to_string(),
                (true, false) => format!("“{}”", note.title),
                _ => format!("“{input}”"),
            };
            // Replacing the note always asks, and says so: the dialog is the
            // last chance to keep what is there.
            let replacing = action.output == actions::ActionOutput::Replace;
            let (message, label) = match (replacing, action.prompt.is_some()) {
                (true, true) => (
                    format!(
                        "Run “{}” on {target} and replace “{}” with what it prints?",
                        action.name, note.title
                    ),
                    "Replace",
                ),
                (true, false) => (
                    format!(
                        "Run “{}” and replace “{}” with what it prints?",
                        action.name, note.title
                    ),
                    "Replace",
                ),
                (false, _) => (format!("Run “{}” on {target}?", action.name), "Run"),
            };
            self.overlay = Some(Overlay::Confirm {
                message,
                confirm_label: label.into(),
                action: Pending::RunAction(action, note, input),
            });
        } else {
            self.spawn_action(action, note, input);
        }
    }

    fn spawn_action(&mut self, action: Action, note: Note, input: String) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        let name = action.name.clone();
        if action.interactive {
            if let Some(running) = self.running_session() {
                self.notify(
                    &format!("“{running}” is already running."),
                    Duration::from_secs(3),
                );
                return;
            }
            self.session_starting = Some(name.clone());
            self.session_gen += 1;
            let generation = self.session_gen;
            self.notify(
                &format!("Starting “{name}”… (esc cancels)"),
                Duration::from_secs(3),
            );
            tokio::spawn(async move {
                let payload = Self::action_payload(&client, &action, &note, &input).await;
                let _ = tx.send(Msg::SessionReady {
                    generation,
                    action: Box::new(action),
                    payload: payload.map(Box::new),
                });
            });
            return;
        }
        self.notify(&format!("Running “{name}”…"), Duration::from_secs(3));
        if let Some(write) = action.output.write() {
            // A new note lands where `n` would put one.
            let tags = if self.selection.tag.is_empty() {
                self.selection.workspace.clone()
            } else {
                self.selection.tag.clone()
            };
            tokio::spawn(async move {
                let result = async {
                    let (before, captured) =
                        Self::run_captured(&client, &action, &note, &input).await?;
                    Ok(Self::write_output(
                        &client, write, &action, &note, &before, &captured, &tags,
                    )
                    .await)
                }
                .await;
                let _ = tx.send(Msg::ActionWrote {
                    action: Box::new(action),
                    note,
                    result,
                });
            });
            return;
        }
        tokio::spawn(async move {
            let result = Self::run_captured(&client, &action, &note, &input)
                .await
                .map(|(_, captured)| captured.summary());
            let _ = tx.send(Msg::ActionDone { name, result });
        });
    }

    /// Read the note, render it and run the command, capturing its output.
    /// Returns the note as it was read, whose hash guards a `replace`.
    async fn run_captured(
        client: &BearClient,
        action: &Action,
        note: &Note,
        input: &str,
    ) -> Result<(NoteContent, actions::Captured), String> {
        let fmt = crate::export::format_by_id(&action.format);
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
        let captured = actions::execute(action, note, &content.content, &images, input)
            .await
            .map_err(|e| e.0)?;
        Ok((content, captured))
    }

    /// Hand a successful command's output to Bear the way its `output` says.
    /// Nothing is written for empty, oversized or non-UTF-8 output, nor into a
    /// note that went to the trash while the command ran; a refused or failed
    /// write keeps the output in a temp file.
    async fn write_output(
        client: &BearClient,
        write: actions::BearWrite,
        action: &Action,
        note: &Note,
        before: &NoteContent,
        captured: &actions::Captured,
        tags: &str,
    ) -> ActionWrite {
        use actions::BearWrite;
        let keep = |bytes: &[u8]| actions::keep_output(&action.name, bytes);
        let skipped = |why: String, kept: Option<std::io::Result<PathBuf>>| ActionWrite::Skipped {
            why,
            kept,
        };
        let text = match actions::text_for_bear(captured) {
            Ok(text) => text,
            Err(refusal) => {
                let kept = refusal.keep.then(|| keep(&captured.stdout));
                return skipped(refusal.reason, kept);
            }
        };
        // Without a hash the overwrite would be unconditional. Checked before
        // anything is asked of Bear.
        if write == BearWrite::Replace && before.hash.is_empty() {
            return skipped(
                format!(
                    "Bear gave no hash for “{}” to guard the replace",
                    note.title
                ),
                Some(keep(text.as_bytes())),
            );
        }
        if write != BearWrite::NewNote {
            // The hash guards the text, not where the note is: a note trashed
            // while the command ran reads back unchanged.
            match client.title_and_location(&note.id).await {
                Ok((_, location)) if location == "trash" => {
                    return skipped(
                        format!("“{}” went to the trash while it ran", note.title),
                        Some(keep(text.as_bytes())),
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    return ActionWrite::Failed {
                        message: err.to_string(),
                        conflict: false,
                        kept: keep(text.as_bytes()),
                        backup: None,
                    };
                }
            }
        }
        // A `replace` whose overwrite failed in a way that does not prove the
        // note untouched: the copy of the old text stays and is named.
        let mut uncertain_backup = None;
        let written = match write {
            BearWrite::Append => client
                .append(&note.id, &text, action.section.as_deref())
                .await
                .map(|()| ActionWrite::Appended),
            BearWrite::NewNote => {
                let tags: Vec<String> = [tags.to_string()]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect();
                client
                    .create_from_content(&tags, &text)
                    .await
                    .map(|(id, title)| ActionWrite::Created { id, title })
            }
            BearWrite::Replace => {
                // Replacing cannot be undone in Bear, so the text it replaces
                // is kept first: no copy, no replace.
                let backup = match actions::keep_output(
                    &format!("{} (before {})", note.title, action.name),
                    before.content.as_bytes(),
                ) {
                    Ok(backup) => backup,
                    Err(e) => {
                        return skipped(
                            format!("the note's text could not be saved first ({e})"),
                            Some(keep(text.as_bytes())),
                        );
                    }
                };
                match client.overwrite(&note.id, &text, &before.hash).await {
                    Ok(()) => {
                        // Bear takes the title from the new text.
                        let title = client
                            .title_and_location(&note.id)
                            .await
                            .map(|(title, _)| title)
                            .ok()
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| note.title.clone());
                        Ok(ActionWrite::Replaced { title, backup })
                    }
                    // Only the stale-hash refusal proves Bear wrote nothing,
                    // so only then is the copy thrown away. A timeout or a
                    // killed bearcli may come after Bear already replaced
                    // the text, and then the copy is all that is left of it.
                    Err(err) if err.is_conflict() => {
                        let _ = std::fs::remove_file(&backup);
                        if let Some(dir) = backup.parent() {
                            let _ = std::fs::remove_dir(dir);
                        }
                        Err(err)
                    }
                    Err(err) => {
                        uncertain_backup = Some(backup);
                        Err(err)
                    }
                }
            }
        };
        written.unwrap_or_else(|err| ActionWrite::Failed {
            message: err.to_string(),
            conflict: err.is_conflict(),
            kept: keep(text.as_bytes()),
            backup: uncertain_backup,
        })
    }

    fn on_action_wrote(&mut self, action: Action, note: Note, result: Result<ActionWrite, String>) {
        let name = action.name.clone();
        let written = match result {
            Ok(written) => written,
            Err(message) => {
                self.notify_titled(
                    &format!("{name} failed"),
                    &format!("{message} Nothing was written to Bear."),
                    Severity::Error,
                    Duration::from_secs(10),
                );
                return;
            }
        };
        let done = |app: &mut App, message: String| {
            app.notify_titled(
                &name,
                &message,
                Severity::Information,
                Duration::from_secs(6),
            )
        };
        match written {
            // The reloads keep the cursor wherever it is now: the command
            // may have run for a minute, and you may have moved on.
            ActionWrite::Appended => {
                self.forget_content(Some(&note.id));
                self.start_reload(None, None, true);
                done(
                    self,
                    match &action.section {
                        Some(section) => format!("Added under “{section}” in “{}”.", note.title),
                        None => format!("Added to the end of “{}”.", note.title),
                    },
                );
            }
            ActionWrite::Created { id, title } => {
                self.start_reload(None, Some(id), false);
                done(self, format!("Created “{title}”."));
            }
            ActionWrite::Replaced { title, backup } => {
                self.forget_content(Some(&note.id));
                self.note_written(&title);
                self.start_reload(None, None, true);
                self.notify_titled(
                    &name,
                    &format!(
                        "Replaced “{}”. The text it had is at {}",
                        note.title,
                        backup.display()
                    ),
                    Severity::Information,
                    Duration::from_secs(15),
                );
            }
            ActionWrite::Skipped { why, kept } => self.notify_titled(
                &name,
                &skipped_message(&why, kept.as_ref()),
                Severity::Warning,
                Duration::from_secs(if kept.is_some() { 30 } else { 8 }),
            ),
            ActionWrite::Failed {
                message,
                conflict,
                kept,
                backup,
            } => {
                let mut kept = kept_phrase(&kept);
                if let Some(backup) = backup {
                    // Bear may have written before the failure; say where the
                    // old text is rather than that nothing changed, and read
                    // the note again so the reader shows what Bear holds.
                    self.forget_content(Some(&note.id));
                    self.start_reload(None, None, true);
                    kept = format!(
                        "{kept}. Bear may have replaced “{}” anyway; the text it had is at {}",
                        note.title,
                        backup.display()
                    );
                }
                let (title, message) = if conflict {
                    (
                        "Edit conflict".to_string(),
                        format!(
                            "“{}” changed in Bear while “{name}” ran. Nothing was written; {kept}",
                            note.title
                        ),
                    )
                } else {
                    (
                        format!("{name}: write failed"),
                        format!("{message} — {kept}"),
                    )
                };
                self.notify_titled(&title, &message, Severity::Error, Duration::from_secs(30));
            }
        }
    }

    // -- input ---------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if let Some(session) = self.session.as_mut() {
            session.pty.send_key(key);
            return;
        }
        // The session is about to take the keyboard; a key now would act on
        // Bjorn behind it (a quit dialog nobody can see), so only `esc`, to
        // call it off, counts. The rest are dropped, not saved for the
        // command: they were typed before it drew anything, so a replayed `y`
        // could answer a question nobody has read yet.
        if let Some(name) = &self.session_starting {
            if key.code == KeyCode::Esc {
                let name = name.clone();
                self.session_starting = None;
                self.notify(&format!("“{name}” canceled."), Duration::from_secs(3));
            }
            return;
        }
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // History: backspace and ctrl+o as in a pager or vim, alt+← and
            // alt+→ as in a browser. Terminals that send option+arrow as a
            // word motion deliver alt+b and alt+f instead.
            // With the search box open, backspace belongs to the query (even
            // once `enter` has moved the focus to the list), never to history;
            // unless the box only shows a query that going back put there.
            KeyCode::Backspace => {
                if !self.notes.search.open || self.search_restored {
                    self.go_back();
                }
            }
            KeyCode::Char('o') if ctrl => self.go_back(),
            KeyCode::Left | KeyCode::Char('b') if alt => self.go_back(),
            KeyCode::Right | KeyCode::Char('f') if alt => self.go_forward(),
            KeyCode::Char('L') => self.open_links(),
            KeyCode::Char('q') => self.confirm_quit(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help { scroll: 0 }),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Esc => self.clear_search(),
            KeyCode::Char('n') => self.new_note(),
            KeyCode::Char('N') => self.open_templates(),
            KeyCode::Char('D') => self.open_daily(),
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
            KeyCode::Char('!') => self.run_default_action(),
            KeyCode::Char('a') => self.open_actions(),
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
            KeyCode::Char('}') => self.jump_heading(1),
            KeyCode::Char('{') => self.jump_heading(-1),
            KeyCode::Char('o') => self.open_outline(),
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
                KeyCode::Esc | KeyCode::Char('n') => {
                    // Cancelling a delete goes back to the menu, on that action.
                    self.overlay = match action {
                        Pending::DeleteAction(kept, note) => Some(Overlay::Actions {
                            field: Field::default(),
                            index: self
                                .config
                                .actions
                                .iter()
                                .position(|a| a == &kept)
                                .unwrap_or(0),
                            note,
                        }),
                        _ => None,
                    }
                }
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
                    match purpose {
                        // Nothing to export to; the prompt is simply canceled.
                        TextPurpose::ExportPath { .. } if value.is_empty() => {}
                        TextPurpose::ExportPath { format_id, note } => {
                            self.export_to(format_id, note, Path::new(&value))
                        }
                        // An empty answer is a real one: the command decides
                        // what no input means.
                        TextPurpose::ActionInput { action, note } => {
                            self.confirm_action(action, note, value)
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
            Overlay::Actions {
                mut field,
                index,
                note,
            } => {
                let matched = actions::filter(&self.config.actions, &field.value).len();
                // The last row adds a new action, so there is always one to land on.
                let rows = matched + 1;
                let step = |index: usize, delta: i32| {
                    (index as i32 + delta).rem_euclid(rows as i32) as usize
                };
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('d') {
                    // Deleting asks first, the way quitting does.
                    let chosen = actions::filter(&self.config.actions, &field.value)
                        .get(index)
                        .map(|a| (*a).clone());
                    self.overlay = Some(match chosen {
                        Some(action) => Overlay::Confirm {
                            message: format!("Delete “{}” from the config?", action.name),
                            confirm_label: "Delete".into(),
                            action: Pending::DeleteAction(action, note),
                        },
                        None => Overlay::Actions { field, index, note },
                    });
                    return;
                }
                if ctrl && key.code == KeyCode::Char('e') {
                    // Letters go to the search box, so editing takes ctrl.
                    let chosen = actions::filter(&self.config.actions, &field.value)
                        .get(index)
                        .map(|a| (*a).clone());
                    self.overlay = Some(match chosen {
                        Some(action) => Overlay::NewAction {
                            name: Field::new(&action.name),
                            command: Field::new(&action.command),
                            format: if action.format_error.is_some() {
                                None
                            } else {
                                Some(
                                    FORMATS
                                        .iter()
                                        .position(|f| f.id == action.format)
                                        .unwrap_or(0),
                                )
                            },
                            confirm: action.confirm,
                            default: action.default,
                            output: output_index(action.output),
                            section: Field::new(action.section.as_deref().unwrap_or("")),
                            focus: 0,
                            note,
                            editing: Some(action),
                        },
                        None => Overlay::Actions { field, index, note },
                    });
                    return;
                }
                let delta = match key.code {
                    KeyCode::Down | KeyCode::Tab => Some(1),
                    KeyCode::Up | KeyCode::BackTab => Some(-1),
                    KeyCode::Char('n') if ctrl => Some(1),
                    KeyCode::Char('p') if ctrl => Some(-1),
                    _ => None,
                };
                if let Some(delta) = delta {
                    self.overlay = Some(Overlay::Actions {
                        field,
                        index: step(index, delta),
                        note,
                    });
                    return;
                }
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Enter if index >= matched => {
                        // Whatever was typed in the search box is a good name.
                        let name = field.value.trim().to_string();
                        self.overlay = Some(Overlay::NewAction {
                            focus: if name.is_empty() { 0 } else { 1 },
                            name: Field::new(&name),
                            command: Field::default(),
                            format: Some(0),
                            confirm: false,
                            default: false,
                            output: 0,
                            section: Field::default(),
                            note,
                            editing: None,
                        });
                    }
                    KeyCode::Enter => {
                        let chosen = actions::filter(&self.config.actions, &field.value)
                            .get(index)
                            .map(|a| (*a).clone());
                        if let Some(action) = chosen {
                            self.overlay = None;
                            self.start_action(action, note);
                        }
                    }
                    _ => {
                        // Typing filters, so the highlight goes back to the top.
                        let index = if Self::field_key(&mut field, &key) {
                            0
                        } else {
                            index
                        };
                        self.overlay = Some(Overlay::Actions { field, index, note });
                    }
                }
            }
            Overlay::Outline { mut field, index } => {
                let shown = outline_filter(&self.reader.headings, &field.value);
                // A refresh can shorten the note under the open outline.
                let index = index.min(shown.len().saturating_sub(1));
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let delta = match key.code {
                    KeyCode::Down | KeyCode::Tab => Some(1),
                    KeyCode::Up | KeyCode::BackTab => Some(-1),
                    KeyCode::Char('n') if ctrl => Some(1),
                    KeyCode::Char('p') if ctrl => Some(-1),
                    _ => None,
                };
                if let Some(delta) = delta {
                    let index = if shown.is_empty() {
                        0
                    } else {
                        (index as i32 + delta).rem_euclid(shown.len() as i32) as usize
                    };
                    self.overlay = Some(Overlay::Outline { field, index });
                    return;
                }
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Enter => {
                        if let Some(&heading) = shown.get(index) {
                            self.overlay = None;
                            self.reader.scroll_to_heading(
                                heading,
                                self.reader_width,
                                self.reader_height,
                            );
                            self.focus = Pane::Reader;
                        }
                    }
                    _ => {
                        // A changed filter sends the highlight back to the top;
                        // moving the text cursor does not.
                        let before = field.value.clone();
                        Self::field_key(&mut field, &key);
                        let index = if field.value != before { 0 } else { index };
                        self.overlay = Some(Overlay::Outline { field, index });
                    }
                }
            }
            Overlay::NewAction {
                mut name,
                mut command,
                mut format,
                mut confirm,
                mut default,
                mut output,
                mut section,
                mut focus,
                note,
                editing,
            } => {
                let outputs = actions::ActionOutput::ALL.len();
                let fields = crate::ui::modals::NEW_ACTION_FIELDS;
                match key.code {
                    KeyCode::Esc => {
                        // Back to the menu, on the action that was being edited.
                        let index = editing
                            .as_ref()
                            .and_then(|e| self.config.actions.iter().position(|a| a == e))
                            .unwrap_or(0);
                        self.overlay = Some(Overlay::Actions {
                            field: Field::default(),
                            index,
                            note,
                        });
                        return;
                    }
                    KeyCode::Tab | KeyCode::Down => focus = (focus + 1) % fields,
                    KeyCode::BackTab | KeyCode::Up => focus = (focus + fields - 1) % fields,
                    KeyCode::Enter if name.value.trim().is_empty() => focus = 0,
                    KeyCode::Enter if command.value.trim().is_empty() => focus = 1,
                    KeyCode::Enter => {
                        let form = Overlay::NewAction {
                            name,
                            command,
                            format,
                            confirm,
                            default,
                            output,
                            section,
                            focus,
                            note: note.clone(),
                            editing: editing.clone(),
                        };
                        let Some(action) = form.form_action() else {
                            return;
                        };
                        // Saving an action that would refuse to run is a trap:
                        // the form says why and stays up.
                        if let Some(problem) = action.misconfigured() {
                            self.notify_titled(
                                "Not saved",
                                &format!("{problem}."),
                                Severity::Warning,
                                Duration::from_secs(8),
                            );
                            self.overlay = Some(form);
                            return;
                        }
                        self.save_action(action, editing, form, note);
                        return;
                    }
                    // From an unknown format, either way lands on the first.
                    KeyCode::Left if focus == 2 => {
                        format = Some(format.map_or(0, |f| (f + FORMATS.len() - 1) % FORMATS.len()))
                    }
                    KeyCode::Right | KeyCode::Char(' ') if focus == 2 => {
                        format = Some(format.map_or(0, |f| (f + 1) % FORMATS.len()))
                    }
                    KeyCode::Char(' ') if focus == 3 => confirm = !confirm,
                    KeyCode::Char(' ') if focus == 4 => default = !default,
                    KeyCode::Left if focus == 5 => output = (output + outputs - 1) % outputs,
                    KeyCode::Right | KeyCode::Char(' ') if focus == 5 => {
                        output = (output + 1) % outputs
                    }
                    _ if focus == 0 => {
                        Self::field_key(&mut name, &key);
                    }
                    _ if focus == 1 => {
                        Self::field_key(&mut command, &key);
                    }
                    _ if focus == 6 => {
                        Self::field_key(&mut section, &key);
                    }
                    _ => {}
                }
                self.overlay = Some(Overlay::NewAction {
                    name,
                    command,
                    format,
                    confirm,
                    default,
                    output,
                    section,
                    focus,
                    note,
                    editing,
                });
            }
            Overlay::Links {
                mut field,
                index,
                note,
                outgoing,
                more,
                backlinks,
                capped,
                error,
            } => {
                let (rows, chosen) = {
                    let (out, back) = filter_links(&outgoing, backlinks.as_deref(), &field.value);
                    let chosen = if index < out.len() {
                        out.get(index)
                    } else {
                        back.get(index - out.len())
                    }
                    .map(|r| r.target.clone());
                    (out.len() + back.len(), chosen)
                };
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let delta = match key.code {
                    KeyCode::Down | KeyCode::Tab => Some(1),
                    KeyCode::Up | KeyCode::BackTab => Some(-1),
                    KeyCode::Char('n') if ctrl => Some(1),
                    KeyCode::Char('p') if ctrl => Some(-1),
                    _ => None,
                };
                let mut index = index;
                match key.code {
                    _ if delta.is_some() => {
                        if rows > 0 {
                            index = (index as i32 + delta.unwrap_or(0)).rem_euclid(rows as i32)
                                as usize;
                        }
                    }
                    KeyCode::Esc => {
                        self.overlay = None;
                        return;
                    }
                    KeyCode::Enter => {
                        let Some(target) = chosen else { return };
                        self.overlay = None;
                        match target {
                            LinkTarget::Wiki(link) => self.follow_link(link),
                            LinkTarget::Note { id } => self.follow_note(&id),
                        }
                        return;
                    }
                    _ => {
                        // Typing filters, so the highlight goes back to the top.
                        if Self::field_key(&mut field, &key) {
                            index = 0;
                        }
                    }
                }
                self.overlay = Some(Overlay::Links {
                    field,
                    index,
                    note,
                    outgoing,
                    more,
                    backlinks,
                    capped,
                    error,
                });
            }
            Overlay::NewNote {
                mut title,
                mut tags,
                mut field,
                template,
            } => match key.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Tab | KeyCode::BackTab => {
                    self.overlay = Some(Overlay::NewNote {
                        title,
                        tags,
                        field: 1 - field,
                        template,
                    })
                }
                KeyCode::Enter => {
                    let title_text = title.value.trim().to_string();
                    if field == 0 {
                        if !title_text.is_empty() {
                            field = 1;
                        }
                        self.overlay = Some(Overlay::NewNote {
                            title,
                            tags,
                            field,
                            template,
                        });
                    } else if title_text.is_empty() {
                        self.overlay = Some(Overlay::NewNote {
                            title,
                            tags,
                            field: 0,
                            template,
                        });
                    } else {
                        self.overlay = None;
                        let content = template
                            .map(|t| {
                                t.content(
                                    &Local::now(),
                                    &title_text,
                                    &self.selection.workspace,
                                    &split_tags(&tags.value),
                                )
                            })
                            .unwrap_or_default();
                        self.create_note(title_text, tags.value.trim().to_string(), content);
                    }
                }
                _ => {
                    Self::field_key(if field == 0 { &mut title } else { &mut tags }, &key);
                    self.overlay = Some(Overlay::NewNote {
                        title,
                        tags,
                        field,
                        template,
                    });
                }
            },
            Overlay::Templates {
                mut field,
                index,
                templates,
                skipped,
            } => {
                let matched = filter_templates(&templates, &field.value).len();
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let delta = match key.code {
                    KeyCode::Down | KeyCode::Tab => Some(1),
                    KeyCode::Up | KeyCode::BackTab => Some(-1),
                    KeyCode::Char('n') if ctrl => Some(1),
                    KeyCode::Char('p') if ctrl => Some(-1),
                    _ => None,
                };
                if let Some(delta) = delta {
                    let index = if matched == 0 {
                        0
                    } else {
                        (index as i32 + delta).rem_euclid(matched as i32) as usize
                    };
                    self.overlay = Some(Overlay::Templates {
                        field,
                        index,
                        templates,
                        skipped,
                    });
                    return;
                }
                match key.code {
                    KeyCode::Esc => self.overlay = None,
                    KeyCode::Enter => {
                        let chosen = filter_templates(&templates, &field.value)
                            .get(index)
                            .map(|t| (*t).clone());
                        match chosen {
                            Some(template) => self.new_note_from(Some(template)),
                            None => {
                                self.overlay = Some(Overlay::Templates {
                                    field,
                                    index,
                                    templates,
                                    skipped,
                                })
                            }
                        }
                    }
                    _ => {
                        // Typing filters, so the highlight goes back to the top.
                        let index = if Self::field_key(&mut field, &key) {
                            0
                        } else {
                            index
                        };
                        self.overlay = Some(Overlay::Templates {
                            field,
                            index,
                            templates,
                            skipped,
                        });
                    }
                }
            }
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(session) = self.session.as_mut() {
            let pane = self.rects.editor;
            if mouse.column >= pane.x
                && mouse.column < pane.right()
                && mouse.row >= pane.y
                && mouse.row < pane.bottom()
            {
                session
                    .pty
                    .send_mouse(mouse, mouse.column - pane.x, mouse.row - pane.y);
            }
            return;
        }
        if self.session_starting.is_some() {
            return;
        }
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
                    // The text starts two cells in from the pane's edge.
                    let left = self.rects.reader_body.x + 2;
                    let row = (y - self.rects.reader_body.y) as usize;
                    if x >= left
                        && let Some(link) =
                            self.reader
                                .link_at((x - left) as usize, row, self.reader_width)
                    {
                        self.follow_link(link);
                    }
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

// -- wiki links and history ----------------------------------------------------------

impl App {
    /// Index the snapshot's titles for `resolve_title`, once per snapshot.
    fn rebuild_title_index(&mut self) {
        self.title_index.clear();
        for (i, note) in self.snapshot.notes.iter().enumerate() {
            self.title_index
                .entry(wiki::title_key(&note.title))
                .or_default()
                .push(i);
        }
    }

    /// The note a wiki link names: an exact title match, ignoring case. When
    /// several notes share the title, an active note beats an archived one,
    /// which beats one in the trash, and then the most recently modified wins.
    pub fn resolve_title(&self, title: &str) -> Option<&Note> {
        let rank = |n: &Note| match n.location {
            Location::Notes => 0,
            Location::Archive => 1,
            Location::Trash => 2,
        };
        self.title_index
            .get(&wiki::title_key(title))?
            .iter()
            .filter_map(|&i| self.snapshot.notes.get(i))
            .min_by_key(|n| (rank(n), std::cmp::Reverse(n.modified)))
    }

    /// The note a link points at; `[[/Heading]]` is the note on the page.
    /// With the reading of the link that named it: the first of
    /// `WikiLink::readings` whose title is a note.
    fn link_target(&self, link: &WikiLink) -> Option<(Note, WikiLink)> {
        link.readings().into_iter().find_map(|reading| {
            let note = if reading.title.is_empty() {
                self.reader.note.clone()
            } else {
                self.resolve_title(&reading.title).cloned()
            }?;
            Some((note, reading))
        })
    }

    /// Is the reader showing note `id`, body and all?
    fn reader_shows(&self, id: &str) -> bool {
        self.reader.full_text.is_some() && self.reader.note.as_ref().is_some_and(|n| n.id == id)
    }

    /// Follow a wiki link: open the note it names, at its heading if it names
    /// one. A title no note has offers to create that note.
    pub fn follow_link(&mut self, link: WikiLink) {
        let Some((note, link)) = self.link_target(&link) else {
            // No reading names a note: offer the first, the most literal.
            // For `[[A/B testing]]` that is the whole text, so nothing the
            // link says is dropped, and the link then resolves to what it made.
            let title = link.readings().swap_remove(0).title;
            self.overlay = Some(Overlay::Confirm {
                message: format!("No note is called “{}”. Create it?", strip_control(&title)),
                confirm_label: "Create".into(),
                action: Pending::CreateLinked(title),
            });
            return;
        };
        if self.reader_shows(&note.id) {
            // A link into the page itself: scroll, nothing to load.
            if !link.section.is_empty() {
                self.push_history();
                self.pending_jump = Some((note.id, Jump::Section(link.section)));
                self.apply_pending_jump();
            }
            self.focus = Pane::Reader;
            return;
        }
        self.push_history();
        self.reveal(&note.id);
        // Set after `reveal`, whose preview would drop a jump for another note.
        self.pending_jump =
            (!link.section.is_empty()).then_some((note.id, Jump::Section(link.section)));
        self.apply_pending_jump();
    }

    /// Open a note by id, remembering where the reader was.
    pub fn follow_note(&mut self, id: &str) {
        self.push_history();
        self.pending_jump = None;
        self.reveal(id);
    }

    /// Put the note under the list cursor and in the reader. When the list
    /// does not hold it, the list widens to the view it lives in (Notes,
    /// Archive or Trash), and a workspace that excludes it is cleared, with a
    /// toast saying so. False when the note is not in the snapshot.
    fn reveal(&mut self, id: &str) -> bool {
        if !self.notes.select_id(id) {
            let Some(note) = self.snapshot.by_id(id).cloned() else {
                self.notify("That note is no longer in Bear.", Duration::from_secs(4));
                return false;
            };
            let view = match note.location {
                Location::Archive => View::Archive,
                Location::Trash => View::Trash,
                Location::Notes => View::All,
            };
            let mut workspace = self.selection.workspace.clone();
            if !in_workspace(&note, &workspace) {
                self.notify_titled(
                    "Workspace cleared",
                    &format!(
                        "“{}” is outside {}; backspace goes back to it, w sets a workspace again.",
                        note.title,
                        display_tag(&workspace)
                    ),
                    Severity::Information,
                    Duration::from_secs(6),
                );
                workspace.clear();
            }
            self.drop_search();
            self.selection = Selection {
                view,
                workspace: workspace.clone(),
                ..Selection::default()
            };
            self.sidebar.populate(&self.snapshot, &workspace, "", view);
            self.apply_selection(Some(id), false);
            if !self.notes.select_id(id) {
                return false;
            }
        }
        if let Some(note) = self.notes.current().cloned() {
            self.schedule_preview(note, true, false);
        }
        self.focus = Pane::Reader;
        true
    }

    /// Land where `pending_jump` says, once the reader shows that note.
    fn apply_pending_jump(&mut self) {
        let Some((id, jump)) = self.pending_jump.clone() else {
            return;
        };
        if !self.reader_shows(&id) {
            return;
        }
        self.pending_jump = None;
        match jump {
            Jump::Scroll(row) => self.reader.scroll = row,
            Jump::Section(section) => {
                if !self
                    .reader
                    .scroll_to_section(&section, self.reader_width, self.reader_height)
                {
                    let title = self.reader.header.clone();
                    self.notify(
                        &format!("No heading “{section}” in “{title}”."),
                        Duration::from_secs(4),
                    );
                }
            }
        }
    }

    /// Where the reader is now, as a history entry.
    fn here(&self) -> Option<Place> {
        let note = self.reader.note.as_ref()?;
        Some(Place {
            id: note.id.clone(),
            selection: self.selection.clone(),
            query: self.search_query.clone(),
            scroll: self.reader.scroll,
        })
    }

    /// Remember the current place before leaving it; a new trail drops the
    /// forward history, as a browser does.
    fn push_history(&mut self) {
        if let Some(place) = self.here()
            && self.back.last() != Some(&place)
        {
            self.back.push(place);
            if self.back.len() > HISTORY_LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// The newest entry of `stack` whose note still exists, dropping any
    /// above it whose note is gone.
    fn pop_live(snapshot: &Snapshot, stack: &mut Vec<Place>) -> Option<Place> {
        while let Some(place) = stack.pop() {
            if snapshot.by_id(&place.id).is_some() {
                return Some(place);
            }
        }
        None
    }

    /// `backspace`: the note before the last link followed.
    pub fn go_back(&mut self) {
        let Some(place) = Self::pop_live(&self.snapshot, &mut self.back) else {
            self.notify("Nothing to go back to.", Duration::from_secs(2));
            return;
        };
        if let Some(here) = self.here() {
            self.forward.push(here);
        }
        self.restore(place);
    }

    /// `alt+→`: undo a `backspace`.
    pub fn go_forward(&mut self) {
        let Some(place) = Self::pop_live(&self.snapshot, &mut self.forward) else {
            self.notify("Nothing to go forward to.", Duration::from_secs(2));
            return;
        };
        if let Some(here) = self.here() {
            self.back.push(here);
        }
        self.restore(place);
    }

    /// Return to a place: its list (view, tag, workspace, search), its note
    /// and its scroll offset.
    fn restore(&mut self, place: Place) {
        let Place {
            id,
            selection,
            query,
            scroll,
        } = place;
        if query.is_empty() {
            if self.notes.search.open {
                self.notes.search.close();
            }
        } else {
            self.notes.search.open_with(&query);
        }
        self.search_restored = !query.is_empty();
        self.search_query = query;
        self.sidebar.populate(
            &self.snapshot,
            &selection.workspace,
            &selection.tag,
            selection.view,
        );
        self.selection = selection;
        self.apply_selection(Some(&id), false);
        if self.notes.current().is_some_and(|n| n.id == id) {
            if let Some(note) = self.notes.current().cloned() {
                self.schedule_preview(note, true, false);
            }
        } else {
            // It has left that list since (retagged, archived): find it anyway.
            self.reveal(&id);
        }
        self.focus = Pane::Reader;
        self.pending_jump = Some((id, Jump::Scroll(scroll)));
        self.apply_pending_jump();
    }

    /// `L`: the note's wiki links, and a search for the notes linking to it.
    pub fn open_links(&mut self) {
        let Some(note) = self.current_note().cloned() else {
            self.notify("No note selected.", Duration::from_secs(3));
            return;
        };
        if note.locked {
            self.notify_titled(
                "",
                "Locked notes do not show their links.",
                Severity::Warning,
                Duration::from_secs(5),
            );
            return;
        }
        if !self.reader_shows(&note.id) {
            self.notify("The note is still loading.", Duration::from_secs(2));
            return;
        }
        let links = self.reader.wiki_links();
        let more = links.len().saturating_sub(OUTGOING_LIMIT);
        let outgoing = links
            .into_iter()
            .take(OUTGOING_LIMIT)
            .map(|link| self.outgoing_row(link))
            .collect();
        self.overlay = Some(Overlay::Links {
            field: Field::default(),
            index: 0,
            note: note.clone(),
            outgoing,
            more,
            backlinks: None,
            capped: false,
            error: String::new(),
        });
        self.start_backlinks(note);
    }

    /// The generation a backlink answer must carry to be shown.
    pub fn backlinks_generation(&self) -> u64 {
        self.backlinks_gen
    }

    /// Search for the notes linking to `note`. One search runs at a time: a
    /// list opened again for the note being searched waits for that answer,
    /// and one for another note starts when it lands.
    fn start_backlinks(&mut self, note: Note) {
        if let Some(running) = &self.backlinks_running {
            if *running != note.id {
                self.backlinks_queued = Some(note);
            }
            return;
        }
        self.backlinks_gen += 1;
        self.backlinks_running = Some(note.id.clone());
        let generation = self.backlinks_gen;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let (title, id) = (note.title.clone(), note.id.clone());
        let search = tokio::spawn(async move {
            client.backlink_rows(&title).await.map(|(rows, capped)| {
                wiki::backlinks(&rows, capped, &title, &id, crate::ui::markdown::wiki_links)
            })
        });
        // The search runs in a task of its own, so a panic in it still
        // answers: without an answer `backlinks_running` never clears, and
        // every later `L` would wait on it for good.
        tokio::spawn(async move {
            let _ = tx.send(Msg::Backlinks {
                generation,
                note_id: note.id,
                result: search_outcome(search.await),
            });
        });
    }

    fn outgoing_row(&self, link: WikiLink) -> LinkRow {
        let target = self.link_target(&link);
        // The reading that resolved names the target best; with none, the
        // one `enter` would create.
        let first = link.readings().swap_remove(0);
        let reading = target.as_ref().map_or(&first, |(_, r)| r);
        let label = if link.alias.is_empty() {
            reading.target_label()
        } else {
            link.alias.clone()
        };
        let mut detail: Vec<String> = Vec::new();
        if !link.alias.is_empty() {
            detail.push(format!("→ {}", reading.target_label()));
        }
        match target.as_ref().map(|(note, _)| note) {
            None => detail.push("no such note yet · enter creates it".into()),
            Some(note) if note.location != Location::Notes => {
                detail.push(format!("in the {}", note.location))
            }
            Some(_) => {}
        }
        // The label and the heading come from the note's own text.
        LinkRow {
            label: strip_control(&label),
            detail: strip_control(&detail.join(" · ")),
            missing: target.is_none(),
            target: LinkTarget::Wiki(link),
        }
    }

    fn on_backlinks(
        &mut self,
        generation: u64,
        note_id: &str,
        result: Result<wiki::Backlinks, BearError>,
    ) {
        if generation != self.backlinks_gen {
            // An older search's answer; a newer one is (or was) running.
            return;
        }
        self.backlinks_running = None;
        if let Some(Overlay::Links {
            note,
            backlinks,
            capped,
            error,
            ..
        }) = self.overlay.as_mut()
            && note.id == note_id
        {
            match result {
                Ok(found) => {
                    *capped = found.capped;
                    *backlinks = Some(
                        found
                            .notes
                            .into_iter()
                            .map(|b| {
                                let mut detail: Vec<String> =
                                    b.sections.iter().map(|s| format!("› {s}")).collect();
                                if b.location != Location::Notes {
                                    detail.push(format!("in the {}", b.location));
                                }
                                LinkRow {
                                    label: strip_control(&b.title),
                                    detail: strip_control(&detail.join(" · ")),
                                    target: LinkTarget::Note { id: b.id },
                                    missing: false,
                                }
                            })
                            .collect(),
                    )
                }
                Err(err) => {
                    *backlinks = Some(Vec::new());
                    *error = err.message;
                }
            }
        }
        // A list opened for another note meanwhile gets its search now.
        if let Some(next) = self.backlinks_queued.take()
            && matches!(&self.overlay, Some(Overlay::Links { note, .. }) if note.id == next.id)
        {
            self.start_backlinks(next);
        }
    }
}

/// A spawned search's answer, with a panic or cancellation in the task
/// turned into an error the Links list can show.
fn search_outcome<T>(
    joined: Result<Result<T, BearError>, tokio::task::JoinError>,
) -> Result<T, BearError> {
    joined.unwrap_or_else(|err| {
        Err(BearError::new(if err.is_panic() {
            "The backlink search failed unexpectedly."
        } else {
            "The backlink search was canceled."
        }))
    })
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

    /// `enter` in triage: the note, as a history entry, with the list focused.
    fn triage_goto(&mut self, note_id: &str) {
        self.close_triage();
        self.follow_note(note_id);
        self.focus = Pane::Notes;
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

/// The new-note prompt's comma-separated tags, normalized, blanks dropped.
fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(normalize_tag)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Where `output` sits in `ActionOutput::ALL`, for the form's picker.
fn output_index(output: actions::ActionOutput) -> usize {
    actions::ActionOutput::ALL
        .iter()
        .position(|o| *o == output)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a hash from the read, `overwrite` would run unguarded, so the
    /// replace is refused and the output kept. The client points at nothing:
    /// the refusal must come before any bearcli call.
    #[tokio::test]
    async fn replace_refuses_a_note_read_without_a_hash() {
        let client = BearClient::with_env(vec!["/nonexistent/bearcli".into()], Vec::new());
        let action = Action {
            name: "Tidy".into(),
            command: "true".into(),
            output: actions::ActionOutput::Replace,
            ..Action::default()
        };
        let note = Note {
            id: "NOTE-1".into(),
            title: "Sprint Planning".into(),
            ..Note::default()
        };
        let before = NoteContent {
            id: "NOTE-1".into(),
            content: "# Sprint Planning\n".into(),
            hash: String::new(),
        };
        let captured = actions::Captured {
            stdout: b"# Sprint Planning\n\ntidied\n".to_vec(),
            ..actions::Captured::default()
        };
        let written = App::write_output(
            &client,
            actions::BearWrite::Replace,
            &action,
            &note,
            &before,
            &captured,
            "",
        )
        .await;
        let ActionWrite::Skipped {
            why,
            kept: Some(Ok(kept)),
        } = written
        else {
            panic!("{written:?}");
        };
        assert!(why.contains("no hash"), "{why}");
        assert_eq!(
            std::fs::read_to_string(&kept).unwrap(),
            "# Sprint Planning\n\ntidied\n"
        );
        std::fs::remove_file(&kept).unwrap();
        std::fs::remove_dir(kept.parent().unwrap()).unwrap();
    }

    /// A failed save of the output is said, with why, not passed over: the
    /// output exists nowhere else.
    #[test]
    fn output_that_could_not_be_saved_says_so_and_why() {
        let full = || std::io::Error::other("No space left on device");
        assert_eq!(
            skipped_message("the note went to the trash", Some(&Err(full()))),
            "It ran, but the note went to the trash, so nothing was written to Bear, \
             and the output could not be saved: No space left on device"
        );
        assert_eq!(
            skipped_message("it printed nothing", None),
            "It ran, but it printed nothing, so nothing was written to Bear."
        );
        assert_eq!(
            skipped_message("x", Some(&Ok(PathBuf::from("/tmp/o.md")))),
            "It ran, but x, so nothing was written to Bear; the output is at /tmp/o.md"
        );
        assert_eq!(
            kept_phrase(&Err(full())),
            "the output could not be saved either: No space left on device"
        );
    }

    /// `keep_output` reports a failure instead of swallowing it: here its
    /// directory cannot be made under a temp dir that is a file.
    #[test]
    fn keep_output_returns_why_it_failed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, "").unwrap();
        let err = actions::keep_output_in(&file, "Sum", b"text").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn a_panicking_backlink_search_still_answers() {
        let search = tokio::spawn(async {
            if true {
                panic!("the search blew up");
            }
            Ok::<(), BearError>(())
        });
        let err = search_outcome(search.await).expect_err("a panic is an error");
        assert_eq!(err.message, "The backlink search failed unexpectedly.");

        let search = tokio::spawn(std::future::pending::<Result<(), BearError>>());
        search.abort();
        let err = search_outcome(search.await).expect_err("a cancel is an error");
        assert_eq!(err.message, "The backlink search was canceled.");

        let search = tokio::spawn(async { Ok::<u8, BearError>(7) });
        assert_eq!(search_outcome(search.await).ok(), Some(7));
    }
}
