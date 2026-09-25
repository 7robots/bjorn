//! Command line entry point and the terminal event loop.

use std::io::{IsTerminal, Read, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use clap::{Parser, Subcommand};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use bjorn::app::App;
use bjorn::bear::{BearClient, resolve_bearcli};
use bjorn::config::Config;

#[derive(Parser)]
#[command(name = "bjorn", version, about = "A terminal front end for Bear.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// start scoped to this tag as the workspace (overrides config)
    #[arg(long, value_name = "TAG")]
    tag: Option<String>,
    /// config file to read instead of the default (before a subcommand)
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// run against a built-in fake bearcli with sample notes (before a subcommand)
    #[arg(long)]
    demo: bool,
    /// palette to draw with (overrides config); `--list-themes` prints the names
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,
    /// list the palettes `--theme` accepts and exit
    #[arg(long)]
    list_themes: bool,
    /// accepted for compatibility with the Python Bjorn; the Rust build always keeps the mouse in cell mode
    #[arg(long)]
    no_mouse_pixels: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Add a line to today's daily note, making the note if needed. The text
    /// comes from the arguments, or from stdin when there are none. Flags go
    /// before `capture`; a word starting with `-` after it is an error, so
    /// write `bjorn capture -- "$text"` for text that might start with one.
    /// Needs a `[daily]` table in the config.
    #[command(disable_help_flag = true)]
    Capture {
        /// what to capture; the words are joined with spaces
        text: Vec<String>,
    },
    /// Print today's daily note as `id<TAB>title`, making it if needed.
    /// Needs a `[daily]` table in the config.
    Today,
}

/// A sibling binary of this executable (the fakes ship next to `bjorn`).
fn sibling(name: &str) -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    // Installed as a symlink in ~/bin: the fakes sit next to the real file.
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no directory for {}", exe.display()))?;
    Ok(dir.join(name))
}

/// The most `bjorn capture` reads from stdin.
const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;

/// The text to capture: the arguments, else stdin when it is not a terminal.
/// Blank text is refused rather than written as an empty line.
fn capture_text(words: Vec<String>) -> Result<String, String> {
    let text = if words.is_empty() {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            return Err("nothing to capture: give the text as arguments or on stdin".into());
        }
        let mut bytes = Vec::new();
        stdin
            .lock()
            .take(MAX_CAPTURE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("reading stdin: {e}"))?;
        // Refused whole rather than cut short: half a paste is worse than none.
        if bytes.len() as u64 > MAX_CAPTURE_BYTES {
            return Err(format!(
                "stdin is over {} MiB; capture takes a line or a block, not a file",
                MAX_CAPTURE_BYTES / (1024 * 1024)
            ));
        }
        String::from_utf8(bytes).map_err(|_| "stdin is not UTF-8 text".to_string())?
    } else {
        words.join(" ")
    };
    if text.trim().is_empty() {
        return Err("nothing to capture: the text is empty".into());
    }
    Ok(text)
}

/// `bjorn today`: find or make today's note and print its id and title.
async fn today(client: &BearClient, config: &Config) -> Result<(), String> {
    let daily = bjorn::daily::today(config, &chrono::Local::now())?;
    let id = bjorn::daily::ensure(client, &daily)
        .await
        .map_err(|e| e.message)?;
    println!("{id}\t{}", daily.title);
    Ok(())
}

/// Raw mode plus the alternate screen, undone on drop (also on panic).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<TerminalGuard> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.list_themes {
        for name in bjorn::ui::theme::names() {
            println!("{name}");
        }
        return Ok(());
    }
    let mut config = Config::load(cli.config.as_deref())?;
    if let Some(command) = cli.command {
        // Both subcommands are daily-note commands. Without `[daily]` they
        // stop here: nothing is read from stdin and bearcli is never called.
        if let Err(message) = bjorn::daily::settings(&config) {
            eprintln!("bjorn: {message}");
            std::process::exit(1);
        }
        let bearcli = if cli.demo {
            sibling("fake-bearcli")?.to_string_lossy().into_owned()
        } else {
            resolve_bearcli(&config.bearcli)
        };
        let client = BearClient::new(vec![bearcli]);
        // `{{workspace}}` in capture_format follows `--tag` like the app does.
        if let Some(tag) = cli.tag.as_deref() {
            config.workspace = bjorn::bear::normalize_tag(tag);
        }
        let outcome = match command {
            Command::Capture { text } => match capture_text(text) {
                Ok(text) => {
                    bjorn::daily::capture(&client, &config, &text, &chrono::Local::now()).await
                }
                Err(message) => Err(message),
            },
            Command::Today => today(&client, &config).await,
        };
        if let Err(message) = outcome {
            eprintln!("bjorn: {message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // A theme named on the command line was typed just now, so a typo is an
    // error. One from the config file is not: the file is shared with the
    // Python Bjorn, which also accepts Textual's own theme names, so an
    // unknown name there falls back to the default with a warning in the app.
    if let Some(theme) = cli.theme.as_deref() {
        if bjorn::ui::theme::lookup(theme).is_none() {
            anyhow::bail!(
                "unknown theme {:?}; try one of: {}",
                theme,
                bjorn::ui::theme::names().collect::<Vec<_>>().join(", ")
            );
        }
        config.theme = theme.trim().to_lowercase();
    }
    if cli.demo {
        config.reminders.enabled = true;
    }
    let command = if cli.demo {
        vec![sibling("fake-bearcli")?.to_string_lossy().into_owned()]
    } else {
        vec![resolve_bearcli(&config.bearcli)]
    };
    // Previews from the last run turn what would be a cold snapshot (every
    // body read to build them) into a warm one. Demo runs get their own file
    // so the fake library never stands in for the real one.
    let cache = if cli.demo {
        bjorn::config::cache_dir().join("previews-demo.json")
    } else {
        bjorn::config::preview_cache_path()
    };
    let client = Arc::new(BearClient::new(command).with_preview_cache(cache));
    let environ: std::collections::HashMap<String, String> = std::env::vars().collect();
    let (mut app, mut rx) = App::new(config, client, cli.tag.as_deref(), environ);
    if cli.demo {
        app.remctl = Some(Arc::new(bjorn::reminders::RemctlClient::new(vec![
            sibling("fake-remctl")?.to_string_lossy().into_owned(),
        ])));
    }

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut events = EventStream::new();
    app.start();

    while app.running {
        terminal.draw(|frame| bjorn::ui::draw(frame, &mut app))?;
        let deadline = app
            .next_deadline()
            .unwrap_or_else(|| Instant::now() + std::time::Duration::from_secs(3600));
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key(key);
                    }
                }
                Some(Ok(Event::Mouse(mouse))) => app.handle_mouse(mouse),
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.into()),
                None => break,
            },
            msg = rx.recv() => match msg {
                Some(msg) => app.handle_msg(msg),
                None => break,
            },
            _ = tokio::time::sleep_until(deadline.into()) => {}
        }
        // Drain whatever else is ready before the next frame.
        while let Ok(msg) = rx.try_recv() {
            app.handle_msg(msg);
        }
        app.tick(Instant::now());
    }
    app.shutdown();
    Ok(())
}
