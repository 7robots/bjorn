//! Command line entry point and the terminal event loop.

use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;
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
    /// start scoped to this tag as the workspace (overrides config)
    #[arg(long, value_name = "TAG")]
    tag: Option<String>,
    /// config file to read instead of the default
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// run against a built-in fake bearcli with sample notes
    #[arg(long)]
    demo: bool,
    /// accepted for compatibility with the Python Bjorn; the Rust build always keeps the mouse in cell mode
    #[arg(long)]
    no_mouse_pixels: bool,
}

/// A sibling binary of this executable (the fakes ship next to `bjorn`).
fn sibling(name: &str) -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no directory for {}", exe.display()))?;
    Ok(dir.join(name))
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
    let mut config = Config::load(cli.config.as_deref())?;
    if cli.demo {
        config.reminders.enabled = true;
    }
    let command = if cli.demo {
        vec![sibling("fake-bearcli")?.to_string_lossy().into_owned()]
    } else {
        vec![resolve_bearcli(&config.bearcli)]
    };
    let client = Arc::new(BearClient::new(command));
    let environ: std::collections::HashMap<String, String> = std::env::vars().collect();
    let (mut app, mut rx) = App::new(config, client, cli.tag.as_deref(), environ);

    let mut guard = Some(TerminalGuard::enter()?);
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
        // An editor takes the terminal: leave the alternate screen, run it, come back.
        if let Some(job) = app.take_editor_job() {
            drop(guard.take());
            let outcome = bjorn::editor::run(&job, false);
            guard = Some(TerminalGuard::enter()?);
            terminal.clear()?;
            app.editor_done(job, outcome);
        }
        app.tick(Instant::now());
    }
    drop(guard);
    Ok(())
}
