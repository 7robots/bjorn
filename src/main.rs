//! Command line entry point.

use std::path::PathBuf;

use clap::Parser;

use bjorn::bear::{BearClient, Location, resolve_bearcli};
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load(cli.config.as_deref())?;
    if let Some(tag) = cli.tag {
        config.workspace = tag.trim().trim_matches('#').to_string();
    }
    let command = if cli.demo {
        vec![sibling("fake-bearcli")?.to_string_lossy().into_owned()]
    } else {
        vec![resolve_bearcli(&config.bearcli)]
    };
    let client = BearClient::new(command);
    let runtime = tokio::runtime::Runtime::new()?;
    // Phase 22: the client and model are in; the TUI arrives in Phase 23.
    let snapshot = runtime.block_on(client.snapshot())?;
    println!(
        "bjorn (Rust, phase 22): {} notes via {} — {} active, {} archived, {} trashed{}",
        snapshot.notes.len(),
        client.describe(),
        snapshot.in_location(Location::Notes).len(),
        snapshot.in_location(Location::Archive).len(),
        snapshot.in_location(Location::Trash).len(),
        if config.workspace.is_empty() {
            String::new()
        } else {
            format!("; workspace #{}", config.workspace)
        },
    );
    Ok(())
}
