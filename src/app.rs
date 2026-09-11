//! The Bjorn application: three panes over one bearcli snapshot.
//!
//! One `App` owns every piece of state and is driven by three inputs: terminal
//! events, `Msg` values sent back by the tokio tasks that talk to bearcli, and
//! `tick` for timers. Drawing reads the state; nothing here touches the
//! terminal. Results are tagged with a generation number and a stale one is
//! dropped, never cancelled mid-flight.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::bear::{
    BearClient, BearError, Note, NoteContent, Probe, Snapshot, display_tag, normalize_tag,
    recently_modified,
};
use crate::config::Config;
use crate::icons::IconSet;
use crate::model::{Selection, View, duplicate_titles, select_notes, today};
use crate::ui::modals::{Overlay, Pending, Severity, Toast};
use crate::ui::note_list::{NoteList, ROW_HEIGHT};
use crate::ui::note_view::Reader;
use crate::ui::sidebar::{Row, Sidebar};

/// Delay between the list cursor moving and the note being fetched and rendered.
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(120);
/// Note bodies kept in memory, most recently read last. Each is keyed by the
/// note's modification stamp, so a changed note is fetched again on its own.
pub const CONTENT_CACHE_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Notes,
    Reader,
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
    Content {
        generation: u64,
        epoch: u64,
        result: Result<NoteContent, BearError>,
    },
}

/// Where the last frame put each pane, for mouse events.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rects {
    pub sidebar_header: Rect,
    pub sidebar_rows: Rect,
    pub notes_header: Rect,
    pub notes_rows: Rect,
    pub note_bar: Rect,
    pub glyph: Rect,
    pub reader_body: Rect,
}

pub struct App {
    pub config: Config,
    pub client: Arc<BearClient>,
    pub icons: IconSet,
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
    content_cache: VecDeque<(String, String, NoteContent)>,
    cache_epoch: u64,
    load_gen: u64,
    reload_gen: u64,
    preview_due: Option<(Instant, Note)>,
    last_probe: Option<Probe>,
    next_poll: Option<Instant>,
    poll_inflight: bool,
    reload_inflight: bool,
    /// Set while the terminal is handed to an editor; the poll waits.
    pub busy: bool,
    written_titles: Vec<String>,
    /// The width the reader was last drawn at, so jumps can find a block's row.
    reader_width: usize,
    reader_height: usize,
}

impl App {
    pub fn new(
        config: Config,
        client: Arc<BearClient>,
        workspace: Option<&str>,
        term_program: Option<&str>,
    ) -> (App, UnboundedReceiver<Msg>) {
        let (tx, rx) = unbounded_channel();
        let icons = IconSet::new(&config.icon_style, &config.icons, term_program);
        let ws = normalize_tag(workspace.unwrap_or(&config.workspace));
        let mut sidebar = Sidebar::new(icons.clone());
        sidebar.set_workspace(&ws);
        let app = App {
            selection: Selection {
                workspace: ws,
                ..Selection::default()
            },
            config,
            client,
            icons,
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
            reader_width: 80,
            reader_height: 24,
        };
        (app, rx)
    }

    /// Kick off the first load and the poll timer.
    pub fn start(&mut self) {
        self.start_reload(None, None, false);
        if self.config.poll_seconds > 0 {
            self.next_poll = Some(Instant::now() + Duration::from_secs(self.config.poll_seconds));
        }
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
        let snapshot = match result {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.notify_titled(
                    "bearcli",
                    &err.message,
                    Severity::Error,
                    Duration::from_secs(10),
                );
                if !self.loaded {
                    self.reader.clear(&format!("Could not read Bear: {err}"));
                }
                return;
            }
        };
        self.snapshot = snapshot;
        self.loaded = true;
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
                } else if generation < self.reload_gen && result.is_ok() {
                    // An older reload finishing after a newer one started: the newer result will follow.
                }
            }
            Msg::Probed(result) => self.on_probed(result),
            Msg::Content {
                generation,
                epoch,
                result,
            } => self.on_content(generation, epoch, result),
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
        if immediate {
            self.preview_due = None;
            self.load_note(note);
        } else {
            self.preview_due = Some((Instant::now() + PREVIEW_DEBOUNCE, note));
        }
    }

    fn cached_content(&mut self, note: &Note) -> Option<NoteContent> {
        let stamp = note.modified.map(|m| m.to_rfc3339()).unwrap_or_default();
        if recently_modified(note.modified, None) {
            return None;
        }
        let pos = self
            .content_cache
            .iter()
            .position(|(id, s, _)| *id == note.id && *s == stamp)?;
        let entry = self.content_cache.remove(pos)?;
        let content = entry.2.clone();
        self.content_cache.push_back(entry);
        Some(content)
    }

    fn remember_content(&mut self, note_id: &str, stamp: &str, content: &NoteContent) {
        self.content_cache.retain(|(id, _, _)| id != note_id);
        self.content_cache
            .push_back((note_id.to_string(), stamp.to_string(), content.clone()));
        while self.content_cache.len() > CONTENT_CACHE_SIZE {
            self.content_cache.pop_front();
        }
    }

    /// Drop one note's body (or all of them) and outdate any read in flight.
    pub fn forget_content(&mut self, note_id: Option<&str>) {
        match note_id {
            None => self.content_cache.clear(),
            Some(id) => self.content_cache.retain(|(cached, _, _)| cached != id),
        }
        self.cache_epoch += 1;
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
        if let Some(content) = self.cached_content(&note) {
            self.reader.show(&note, &content.content);
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
                    self.remember_content(&note.id, &stamp, &content);
                }
                self.reader.show(&note, &content.content);
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
        self.search_query.clear();
        self.selection = Selection {
            view,
            workspace: self.selection.workspace.clone(),
            ..Selection::default()
        };
        self.apply_selection(None, false);
    }

    fn select_tag(&mut self, tag: &str) {
        self.search_query.clear();
        self.selection = Selection {
            view: View::All,
            tag: tag.to_string(),
            workspace: self.selection.workspace.clone(),
            search_ids: None,
        };
        self.apply_selection(None, false);
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

    // -- columns and focus ----------------------------------------------------------

    pub fn set_columns(&mut self, count: u8) {
        self.columns = count.clamp(1, 3);
        let hidden = match self.focus {
            Pane::Sidebar => self.columns < 3,
            Pane::Notes => self.columns < 2,
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
        self.search_query.clear();
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

    // -- actions -------------------------------------------------------------------

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

    // -- input ---------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if let Some(overlay) = self.overlay.clone() {
            self.handle_overlay_key(overlay, key);
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => self.confirm_quit(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help { scroll: 0 }),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('c') => self.cycle_columns(),
            KeyCode::Char('w') => self.toggle_workspace(),
            KeyCode::Char('W') => {
                if !self.selection.workspace.is_empty() {
                    self.set_workspace("");
                }
            }
            KeyCode::Char('f') => self.fold_tag(),
            KeyCode::Char('F') => self.fold_all(),
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
                Pane::Reader => {}
            },
            KeyCode::Char(c) if c.is_ascii_digit() && !shift => {
                if let Some(view) = c.to_digit(10).and_then(|d| View::ALL.get(d as usize - 1)) {
                    self.action_view(*view);
                }
            }
            _ => {}
        }
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
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
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
