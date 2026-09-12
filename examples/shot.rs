//! Draw the app headlessly through the fake bearcli and dump the terminal
//! buffer as JSON, one entry per cell: the glyph, its colours and its
//! modifiers. `tools/shot.py` turns that into a PNG, which is how the two
//! implementations get compared side by side.
//!
//!     cargo run --example shot -- --theme red-graphite --out /tmp/shot.json
//!
//! Nothing here touches Bear.

use std::sync::Arc;

use bjorn::bear::BearClient;
use bjorn::config::Config;
use bjorn::harness::Harness;

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn color(c: ratatui::style::Color) -> String {
    match c {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        other => format!("{other:?}"),
    }
}

#[tokio::main]
async fn main() {
    let theme = arg("--theme", bjorn::ui::theme::DEFAULT_THEME);
    let out = arg("--out", "/tmp/bjorn-shot.json");
    let width: u16 = arg("--width", "140").parse().unwrap();
    let height: u16 = arg("--height", "42").parse().unwrap();
    let screen = arg("--screen", "main");

    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("bear.json");
    let exe = std::env::current_exe().unwrap();
    let fake = exe
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("fake-bearcli");
    let client = BearClient::with_env(
        vec![fake.to_string_lossy().into_owned()],
        vec![(
            "BJORN_FAKE_BEAR_STATE".into(),
            state.to_string_lossy().into_owned(),
        )],
    );
    let config = Config {
        poll_seconds: 0,
        theme: theme.clone(),
        // Only the action palette draws these; they never run here.
        actions: ["Publish to S3", "Copy as plain text", "Gist it"]
            .iter()
            .map(|name| bjorn::actions::Action {
                name: (*name).to_string(),
                command: "true".into(),
                default: *name == "Publish to S3",
                ..bjorn::actions::Action::default()
            })
            .collect(),
        ..Config::default()
    };
    let mut h = Harness::new(config, Arc::new(client), None, (width, height));
    h.load().await;
    if screen == "triage" {
        h.press("t");
        h.settle().await;
    }
    if screen == "actions" {
        h.press("a");
        h.settle().await;
    }
    if screen == "new-action" {
        h.press("a");
        h.type_text("Upload");
        h.press("enter");
        h.type_text("scp \"$BJORN_NOTE_FILE\" notes-host:notes/");
        for _ in 0..3 {
            h.press("tab");
        }
        h.press("space");
        h.settle().await;
    }
    h.draw();

    let buffer = h.buffer();
    let mut cells = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            cells.push(serde_json::json!({
                "x": x, "y": y,
                "s": cell.symbol(),
                "fg": color(cell.fg),
                "bg": color(cell.bg),
                "mods": format!("{:?}", cell.modifier),
            }));
        }
    }
    let doc = serde_json::json!({
        "theme": theme, "width": width, "height": height, "cells": cells,
    });
    std::fs::write(&out, serde_json::to_string(&doc).unwrap()).unwrap();
    println!("{out}");
}
