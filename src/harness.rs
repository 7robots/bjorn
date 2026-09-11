//! A headless driver for `App`: injected keys and clicks, a test backend to
//! draw into, and a wait loop that runs the message and timer plumbing the
//! real main loop would. Tests read the drawn buffer as text.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::{App, Msg};
use crate::bear::BearClient;
use crate::config::Config;

pub struct Harness {
    pub app: App,
    rx: UnboundedReceiver<Msg>,
    terminal: Terminal<TestBackend>,
}

impl Harness {
    pub fn new(
        config: Config,
        client: Arc<BearClient>,
        workspace: Option<&str>,
        size: (u16, u16),
    ) -> Harness {
        Self::with_env(config, client, workspace, size, HashMap::new())
    }

    /// A harness whose app sees `environ` instead of the process environment
    /// (`EDITOR`, `VISUAL`, `TERM_PROGRAM`).
    pub fn with_env(
        config: Config,
        client: Arc<BearClient>,
        workspace: Option<&str>,
        size: (u16, u16),
        environ: HashMap<String, String>,
    ) -> Harness {
        let (mut app, rx) = App::new(config, client, workspace, environ);
        app.start();
        let terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("test backend");
        let mut harness = Harness { app, rx, terminal };
        harness.draw();
        harness
    }

    pub fn draw(&mut self) {
        let app = &mut self.app;
        self.terminal
            .draw(|frame| crate::ui::draw(frame, app))
            .expect("draw");
    }

    /// Deliver pending messages, editor sessions and timers, then redraw.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.app.handle_msg(msg);
        }
        // Editors run inline here; the real main loop suspends the terminal first.
        if let Some(job) = self.app.take_editor_job() {
            let outcome = crate::editor::run(&job, true);
            self.app.editor_done(job, outcome);
        }
        self.app.tick(Instant::now());
        self.draw();
    }

    /// Run the plumbing until `pred` holds or `timeout` passes.
    pub async fn wait_until(
        &mut self,
        pred: impl Fn(&App) -> bool,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if pred(&self.app) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "condition not met within {timeout:?}\n{}",
                    self.text()
                ));
            }
            let next = self
                .app
                .next_deadline()
                .unwrap_or(Instant::now() + Duration::from_millis(25));
            let step = next
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(25));
            tokio::select! {
                msg = self.rx.recv() => {
                    if let Some(msg) = msg { self.app.handle_msg(msg); }
                }
                _ = tokio::time::sleep(step) => {}
            }
        }
    }

    /// `wait_until` with the default five seconds, panicking on timeout.
    pub async fn until(&mut self, pred: impl Fn(&App) -> bool) {
        if let Err(err) = self.wait_until(pred, Duration::from_secs(5)).await {
            panic!("{err}");
        }
    }

    /// Let queued work drain for a moment.
    pub async fn settle(&mut self) {
        let _ = self.wait_until(|_| false, Duration::from_millis(60)).await;
    }

    /// Wait for the first snapshot and the reader to show the first note.
    pub async fn load(&mut self) {
        self.until(|app| app.loaded && !app.notes.is_empty()).await;
        self.until(|app| app.reader.note.is_some()).await;
    }

    pub fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.app.handle_key(KeyEvent::new(code, modifiers));
        self.draw();
    }

    /// Press a key by the names the Python tests used: `j`, `down`, `enter`,
    /// `escape`, `tab`, `shift+tab`, `question_mark`, `slash`, `F`, `W`, `?`.
    pub fn press(&mut self, name: &str) {
        let (code, modifiers) = match name {
            "enter" => (KeyCode::Enter, KeyModifiers::NONE),
            "escape" | "esc" => (KeyCode::Esc, KeyModifiers::NONE),
            "tab" => (KeyCode::Tab, KeyModifiers::NONE),
            "shift+tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
            "down" => (KeyCode::Down, KeyModifiers::NONE),
            "up" => (KeyCode::Up, KeyModifiers::NONE),
            "left" => (KeyCode::Left, KeyModifiers::NONE),
            "right" => (KeyCode::Right, KeyModifiers::NONE),
            "space" => (KeyCode::Char(' '), KeyModifiers::NONE),
            "backspace" => (KeyCode::Backspace, KeyModifiers::NONE),
            "end" => (KeyCode::End, KeyModifiers::NONE),
            "home" => (KeyCode::Home, KeyModifiers::NONE),
            "pagedown" => (KeyCode::PageDown, KeyModifiers::NONE),
            "pageup" => (KeyCode::PageUp, KeyModifiers::NONE),
            "question_mark" => (KeyCode::Char('?'), KeyModifiers::NONE),
            "slash" => (KeyCode::Char('/'), KeyModifiers::NONE),
            other => {
                let ch = other.chars().next().expect("a key name");
                let modifiers = if ch.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                (KeyCode::Char(ch), modifiers)
            }
        };
        self.key(code, modifiers);
    }

    pub fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.key(
                KeyCode::Char(ch),
                if ch.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            );
        }
    }

    pub fn click(&mut self, x: u16, y: u16) {
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        self.app.handle_mouse(event);
        self.draw();
    }

    pub fn scroll(&mut self, x: u16, y: u16, down: bool) {
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        self.app.handle_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        self.draw();
    }

    /// The screen as text, one string per row, trailing spaces trimmed.
    pub fn lines(&self) -> Vec<String> {
        let buffer = self.terminal.backend().buffer();
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                let mut row = String::new();
                for x in 0..area.width {
                    row.push_str(buffer[(x, y)].symbol());
                }
                row.trim_end().to_string()
            })
            .collect()
    }

    pub fn text(&self) -> String {
        self.lines().join("\n")
    }

    /// The text of one row, untrimmed.
    pub fn row(&self, y: u16) -> String {
        let buffer = self.terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    pub fn cell_bg(&self, x: u16, y: u16) -> Color {
        self.terminal.backend().buffer()[(x, y)].bg
    }

    pub fn cell_fg(&self, x: u16, y: u16) -> Color {
        self.terminal.backend().buffer()[(x, y)].fg
    }

    /// The x of the first cell where `needle` starts on row `y`, looking only
    /// between `x_from` and `x_to`. Wide glyphs take two cells; this maps
    /// through the cells rather than counting characters.
    pub fn find_cell(&self, y: u16, needle: &str, x_from: u16, x_to: u16) -> Option<u16> {
        let buffer = self.terminal.backend().buffer();
        let mut text = String::new();
        let mut starts: Vec<(usize, u16)> = Vec::new();
        for x in x_from..x_to.min(buffer.area.width) {
            starts.push((text.len(), x));
            text.push_str(buffer[(x, y)].symbol());
        }
        let offset = text.find(needle)?;
        starts.iter().find(|(o, _)| *o == offset).map(|(_, x)| *x)
    }

    /// Does the cell carry the reversed+bold search-match style?
    pub fn cell_is_match(&self, x: u16, y: u16) -> bool {
        let cell = &self.terminal.backend().buffer()[(x, y)];
        cell.modifier.contains(ratatui::style::Modifier::REVERSED)
            && cell.modifier.contains(ratatui::style::Modifier::BOLD)
    }

    pub fn size(&self) -> (u16, u16) {
        let area = self.terminal.backend().buffer().area;
        (area.width, area.height)
    }
}
