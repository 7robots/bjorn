//! The editor inside the reader pane: a pseudo-terminal the editor draws
//! into, a `vt100` screen that parses what it drew, and the key and mouse
//! encoding that carries the user's input back to it.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// What the editor sees as its terminal. vim reads terminfo for this name;
/// the `vt100` crate understands what an xterm emits.
pub const TERM: &str = "xterm-256color";

/// How the pty reports back: a redraw is due, or the child is gone.
pub trait Sink: Send + 'static {
    fn output(&self);
    fn exited(&self, status: std::io::Result<portable_pty::ExitStatus>);
}

impl<A, B> Sink for (A, B)
where
    A: Fn() + Send + Sync + 'static,
    B: Fn(std::io::Result<portable_pty::ExitStatus>) + Send + Sync + 'static,
{
    fn output(&self) {
        (self.0)()
    }
    fn exited(&self, status: std::io::Result<portable_pty::ExitStatus>) {
        (self.1)(status)
    }
}

/// One running editor: the pty it owns, the screen it has drawn so far.
pub struct PtySession {
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    rows: u16,
    cols: u16,
}

impl PtySession {
    /// Start `command` in a pty of `rows` × `cols`. A reader thread feeds the
    /// screen and calls `sink.output()` after each chunk, then waits for the
    /// child and calls `sink.exited()` once.
    pub fn spawn(
        command: &[String],
        rows: u16,
        cols: u16,
        sink: impl Sink,
    ) -> std::io::Result<PtySession> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| std::io::Error::other("empty editor command"))?;
        let rows = rows.max(1);
        let cols = cols.max(1);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.env("TERM", TERM);
        cmd.env_remove("COLUMNS");
        cmd.env_remove("LINES");
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(std::io::Error::other)?;
        // The slave end belongs to the child now; holding it open here would
        // keep the reader from seeing EOF.
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(std::io::Error::other)?;
        let writer = pair.master.take_writer().map_err(std::io::Error::other)?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));

        let feed = parser.clone();
        std::thread::Builder::new()
            .name("bjorn-pty".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut parser) = feed.lock() {
                                parser.process(&buf[..n]);
                            }
                            sink.output();
                        }
                    }
                }
                sink.exited(child.wait());
            })?;

        Ok(PtySession {
            parser,
            master: pair.master,
            writer,
            killer,
            rows,
            cols,
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Tell both the child and the screen about a new pane size; a no-op
    /// when nothing changed.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        if let Ok(mut parser) = self.parser.lock() {
            parser.screen_mut().set_size(rows, cols);
        }
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// The screen as drawn so far, locked for the caller's render.
    pub fn parser(&self) -> MutexGuard<'_, vt100::Parser> {
        self.parser
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The screen's text, for tests.
    pub fn contents(&self) -> String {
        self.parser().screen().contents()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn send_key(&mut self, key: KeyEvent) {
        let app_cursor = self.parser().screen().application_cursor();
        let bytes = encode_key(key, app_cursor);
        self.write(&bytes);
    }

    /// Forward a mouse event at pane-relative `(col, row)` if the editor
    /// asked for mouse reporting.
    pub fn send_mouse(&mut self, mouse: MouseEvent, col: u16, row: u16) {
        let (mode, encoding) = {
            let parser = self.parser();
            let screen = parser.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        let bytes = encode_mouse(mouse, col, row, mode, encoding);
        self.write(&bytes);
    }

    /// Ask the child to stop; the reader thread reports the exit as usual.
    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

/// The bytes an xterm would send for `key`.
pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let param = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let with_param = |seq: &str, suffix: char| -> Vec<u8> {
        if param == 1 {
            format!("\x1b[{seq}{suffix}").into_bytes()
        } else {
            format!("\x1b[{seq};{param}{suffix}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if param == 1 {
            format!("\x1b[{code}~").into_bytes()
        } else {
            format!("\x1b[{code};{param}~").into_bytes()
        }
    };
    let cursor = |letter: char| -> Vec<u8> {
        if param != 1 {
            format!("\x1b[1;{param}{letter}").into_bytes()
        } else if application_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let mut out = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let byte = match c.to_ascii_lowercase() {
                    ' ' | '@' | '2' => Some(0u8),
                    c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '7' | '/' => Some(0x1f),
                    '8' | '?' => Some(0x7f),
                    _ => None,
                };
                match byte {
                    Some(b) => vec![b],
                    None => c.to_string().into_bytes(),
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => {
            if ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            if param == 1 {
                format!("\x1bO{letter}").into_bytes()
            } else {
                with_param("1", letter)
            }
        }
        KeyCode::F(5) => tilde(15),
        KeyCode::F(6) => tilde(17),
        KeyCode::F(7) => tilde(18),
        KeyCode::F(8) => tilde(19),
        KeyCode::F(9) => tilde(20),
        KeyCode::F(10) => tilde(21),
        KeyCode::F(11) => tilde(23),
        KeyCode::F(12) => tilde(24),
        _ => Vec::new(),
    };
    // Alt on a plain key is an ESC prefix; the CSI forms above already carry it.
    if alt && !out.starts_with(b"\x1b") {
        out.insert(0, 0x1b);
    }
    out
}

/// The report an xterm would send for `mouse` at `(col, row)`, both
/// zero-based and relative to the pane, under the editor's chosen mode.
pub fn encode_mouse(
    mouse: MouseEvent,
    col: u16,
    row: u16,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Vec<u8> {
    if mode == MouseProtocolMode::None {
        return Vec::new();
    }
    let button_bits = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut button, release) = match mouse.kind {
        MouseEventKind::Down(b) => (button_bits(b), false),
        MouseEventKind::Up(b) => (button_bits(b), true),
        MouseEventKind::Drag(b) => {
            if matches!(
                mode,
                MouseProtocolMode::Press | MouseProtocolMode::PressRelease
            ) {
                return Vec::new();
            }
            (button_bits(b) + 32, false)
        }
        MouseEventKind::Moved => {
            if mode != MouseProtocolMode::AnyMotion {
                return Vec::new();
            }
            (3 + 32, false)
        }
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if release && mode == MouseProtocolMode::Press {
        return Vec::new();
    }
    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        button += 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        button += 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        button += 16;
    }
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            format!("\x1b[<{button};{x};{y}{end}").into_bytes()
        }
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let button = if release { 3 } else { button };
            // The legacy encoding cannot go past 223 columns or rows.
            if x > 223 || y > 223 {
                return Vec::new();
            }
            vec![
                0x1b,
                b'[',
                b'M',
                32 + button as u8,
                (32 + x) as u8,
                (32 + y) as u8,
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn plain_keys_are_their_bytes() {
        assert_eq!(
            encode_key(key(KeyCode::Char('a'), KeyModifiers::NONE), false),
            b"a"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('é'), KeyModifiers::NONE), false),
            "é".as_bytes()
        );
        assert_eq!(
            encode_key(key(KeyCode::Enter, KeyModifiers::NONE), false),
            b"\r"
        );
        assert_eq!(
            encode_key(key(KeyCode::Tab, KeyModifiers::NONE), false),
            b"\t"
        );
        assert_eq!(
            encode_key(key(KeyCode::Esc, KeyModifiers::NONE), false),
            b"\x1b"
        );
        assert_eq!(
            encode_key(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            b"\x7f"
        );
    }

    #[test]
    fn control_keys_fold_to_c0() {
        assert_eq!(
            encode_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            b"\x03"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('R'), KeyModifiers::CONTROL), false),
            b"\x12"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('['), KeyModifiers::CONTROL), false),
            b"\x1b"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char(']'), KeyModifiers::CONTROL), false),
            b"\x1d"
        );
        assert_eq!(
            encode_key(key(KeyCode::Char(' '), KeyModifiers::CONTROL), false),
            b"\x00"
        );
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            encode_key(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(
            encode_key(key(KeyCode::Enter, KeyModifiers::ALT), false),
            b"\x1b\r"
        );
    }

    #[test]
    fn cursor_keys_honor_modes_and_modifiers() {
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), false),
            b"\x1b[A"
        );
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(key(KeyCode::Left, KeyModifiers::SHIFT), false),
            b"\x1b[1;2D"
        );
        assert_eq!(
            encode_key(key(KeyCode::Right, KeyModifiers::CONTROL), true),
            b"\x1b[1;5C"
        );
        assert_eq!(
            encode_key(key(KeyCode::Home, KeyModifiers::NONE), false),
            b"\x1b[H"
        );
        assert_eq!(
            encode_key(key(KeyCode::Delete, KeyModifiers::NONE), false),
            b"\x1b[3~"
        );
        assert_eq!(
            encode_key(key(KeyCode::PageDown, KeyModifiers::SHIFT), false),
            b"\x1b[6;2~"
        );
        assert_eq!(
            encode_key(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(
            encode_key(key(KeyCode::F(1), KeyModifiers::NONE), false),
            b"\x1bOP"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(4), KeyModifiers::NONE), false),
            b"\x1bOS"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(2), KeyModifiers::SHIFT), false),
            b"\x1b[1;2Q"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(5), KeyModifiers::NONE), false),
            b"\x1b[15~"
        );
        assert_eq!(
            encode_key(key(KeyCode::F(12), KeyModifiers::NONE), false),
            b"\x1b[24~"
        );
        assert!(encode_key(key(KeyCode::Null, KeyModifiers::NONE), false).is_empty());
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_reports_follow_the_editor_mode() {
        let press = mouse(MouseEventKind::Down(MouseButton::Left));
        assert!(
            encode_mouse(
                press,
                4,
                2,
                MouseProtocolMode::None,
                MouseProtocolEncoding::Sgr
            )
            .is_empty()
        );
        assert_eq!(
            encode_mouse(
                press,
                4,
                2,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<0;5;3M"
        );
        let release = mouse(MouseEventKind::Up(MouseButton::Left));
        assert_eq!(
            encode_mouse(
                release,
                4,
                2,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<0;5;3m"
        );
        assert!(
            encode_mouse(
                release,
                4,
                2,
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Sgr
            )
            .is_empty()
        );
        let wheel = mouse(MouseEventKind::ScrollDown);
        assert_eq!(
            encode_mouse(
                wheel,
                0,
                0,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<65;1;1M"
        );
        assert_eq!(
            encode_mouse(
                press,
                4,
                2,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default
            ),
            b"\x1b[M\x20\x25\x23"
        );
        let moved = mouse(MouseEventKind::Moved);
        assert!(
            encode_mouse(
                moved,
                0,
                0,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr
            )
            .is_empty()
        );
        assert_eq!(
            encode_mouse(
                moved,
                0,
                0,
                MouseProtocolMode::AnyMotion,
                MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<35;1;1M"
        );
    }

    #[test]
    fn a_session_draws_and_reads_keys() {
        let (tx, rx) = std::sync::mpsc::channel::<Option<bool>>();
        let out = tx.clone();
        let sink = (
            move || {
                let _ = out.send(None);
            },
            move |status: std::io::Result<portable_pty::ExitStatus>| {
                let _ = tx.send(Some(status.map(|s| s.success()).unwrap_or(false)));
            },
        );
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf 'hello pty'; read -r line; printf '\\n[%s]' \"$line\"".to_string(),
        ];
        let mut session = PtySession::spawn(&command, 5, 20, sink).expect("spawn");
        assert_eq!(session.size(), (5, 20));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.contents().contains("hello pty") {
            assert!(
                std::time::Instant::now() < deadline,
                "no output: {:?}",
                session.contents()
            );
            let _ = rx.recv_timeout(std::time::Duration::from_millis(50));
        }
        session.resize(6, 30);
        assert_eq!(session.parser().screen().size(), (6, 30));
        session.send_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        session.send_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        session.send_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let mut exited = None;
        while exited.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "no exit: {:?}",
                session.contents()
            );
            if let Ok(Some(status)) = rx.recv_timeout(std::time::Duration::from_millis(50)) {
                exited = Some(status);
            }
        }
        assert_eq!(exited, Some(true));
        assert!(
            session.contents().contains("[ok]"),
            "{:?}",
            session.contents()
        );
    }
}
