//! Phase 26 acceptance gate: the Rust Bjorn against the live Bear library.
//!
//! Runs the app headlessly through the real bearcli, prints one line per
//! check and exits non-zero on the first failure. A scratch note is created,
//! edited, pinned, exported, trashed, restored and trashed again; it is left
//! in Bear's trash.
//!
//!     bjorn-gate [body-term] [title-term] [tag-prefix]
//!     bjorn-gate --bench          # timings instead of checks
//!
//! Defaults: `hash-guarded`, `Bjorn`, `tec`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bjorn::app::Pane;
use bjorn::bear::{BearClient, Location, resolve_bearcli};
use bjorn::config::Config;
use bjorn::harness::Harness;
use bjorn::ui::markdown;

fn check(cond: bool, label: &str) {
    println!("{}{label}", if cond { "ok   " } else { "FAIL " });
    if !cond {
        std::process::exit(1);
    }
}

async fn wait(h: &mut Harness, label: &str, pred: impl Fn(&bjorn::app::App) -> bool) {
    if let Err(err) = h.wait_until(pred, Duration::from_secs(20)).await {
        println!("FAIL timeout waiting for: {label}\n{err}");
        std::process::exit(1);
    }
}

fn config() -> Config {
    Config {
        poll_seconds: 0,
        icon_style: "none".into(),
        export_dir: std::env::temp_dir().join("bjorn-gate-exports"),
        ..Config::default()
    }
}

async fn run_search(h: &mut Harness, query: &str) {
    if h.app.focus != Pane::Notes {
        h.app.set_focus(Pane::Notes);
    }
    h.press("slash");
    h.app.notes.search.value.clear();
    h.app.notes.search.cursor = 0;
    h.type_text(query);
    h.press("enter");
}

fn row_has_match(h: &Harness, y: u16) -> bool {
    let rows = h.app.rects.notes_rows;
    (rows.x..rows.x + rows.width).any(|x| h.cell_is_match(x, y))
}

async fn gate(body_term: String, title_term: String, tag_prefix: String) {
    let client = Arc::new(BearClient::new(vec![resolve_bearcli("")]));
    let editor = std::env::temp_dir().join("bjorn-gate-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf '\\ngate edit line\\n' >> \"$1\"\n",
    )
    .expect("editor script");
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    let environ: HashMap<String, String> =
        HashMap::from([("EDITOR".to_string(), editor.to_string_lossy().into_owned())]);
    let mut h = Harness::with_env(config(), client.clone(), None, (140, 44), environ);
    h.load().await;
    let total = h.app.notes.len();
    check(
        total > 0,
        &format!("loaded {total} notes through {}", client.describe()),
    );

    // 1. A body-only term lists the note and the reader highlights it.
    run_search(&mut h, &body_term).await;
    let term = body_term.clone();
    wait(&mut h, "body search", move |app| {
        app.notes.header.starts_with(&format!("“{term}”"))
            && !app.notes.is_empty()
            && app.notes.len() < total
    })
    .await;
    let listed = h.app.notes.notes.clone();
    check(
        listed
            .iter()
            .all(|n| !n.title.to_lowercase().contains(&body_term.to_lowercase())),
        &format!(
            "“{body_term}” lists {} note(s), none by title",
            listed.len()
        ),
    );
    let first_id = listed[0].id.clone();
    wait(
        &mut h,
        "reader shows the first result with matches",
        move |app| {
            app.reader.note.as_ref().is_some_and(|n| n.id == first_id)
                && !app.reader.matches.is_empty()
        },
    )
    .await;
    let n = h.app.reader.matches.len();
    check(
        h.app
            .reader
            .plain_text()
            .to_lowercase()
            .contains(&body_term.to_lowercase()),
        &format!("reader holds the term in {n} block(s)"),
    );
    let label = if n == 1 {
        "1 match".to_string()
    } else {
        format!("{n} matches")
    };
    check(
        h.app.reader.header.contains(&label),
        &format!("header counts: {:?}", h.app.reader.header),
    );
    let rows = h.app.rects.notes_rows;
    check(
        !(rows.y..rows.y + 3).any(|y| row_has_match(&h, y)),
        "list row shows no highlight for a body-only match",
    );

    // 2. ] and [ move the reader.
    h.press("]");
    check(
        h.app.reader.match_index == 0 && h.app.focus == Pane::Reader,
        &format!("] → {:?}, reader focused", h.app.reader.header),
    );
    let y_first = h.app.reader.scroll;
    if n > 1 {
        h.press("]");
        check(
            h.app.reader.match_index == 1 && h.app.reader.header.contains(&format!("match 2/{n}")),
            "] again → match 2",
        );
        h.press("[");
        check(
            h.app.reader.match_index == 0 && h.app.reader.scroll == y_first,
            "[ returns to the first match's position",
        );
    } else {
        h.press("[");
        check(
            h.app.reader.match_index == 0,
            "[ with a single match stays on it",
        );
    }

    // 3. A title term highlights the list row.
    run_search(&mut h, &title_term).await;
    let term = title_term.clone();
    wait(&mut h, "title search", move |app| {
        app.notes.header.starts_with(&format!("“{term}”")) && !app.notes.is_empty()
    })
    .await;
    let titled: Vec<_> = h
        .app
        .notes
        .notes
        .iter()
        .filter(|x| x.title.to_lowercase().contains(&title_term.to_lowercase()))
        .cloned()
        .collect();
    check(
        !titled.is_empty(),
        &format!("“{title_term}” lists a note with it in the title"),
    );
    h.app.notes.select_id(&titled[0].id);
    h.draw();
    let rows = h.app.rects.notes_rows;
    let needle: String = titled[0].title.chars().take(5).collect();
    let hit = (rows.y..rows.y + rows.height).find_map(|y| {
        h.find_cell(y, &needle, rows.x, rows.x + rows.width)
            .map(|x| (x, y))
    });
    let styled = hit.is_some_and(|(x, y)| {
        let term_x = h
            .find_cell(y, &title_term, x, rows.x + rows.width)
            .unwrap_or(x);
        h.cell_is_match(term_x, y)
    });
    if !styled {
        println!(
            "     rows:\n{}",
            (rows.y..rows.y + 8)
                .map(|y| h.row(y))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    check(
        styled,
        &format!("row highlights the title term in {:?}", titled[0].title),
    );

    // 4. esc clears everything.
    h.app.set_focus(Pane::Notes);
    h.press("escape");
    wait(&mut h, "esc clears", move |app| {
        app.search_query.is_empty() && app.reader.pattern.is_none() && app.notes.len() == total
    })
    .await;
    check(
        h.app.reader.matches.is_empty() && h.app.notes.pattern.is_none(),
        "esc: no pattern, no matches",
    );
    let rows = h.app.rects.notes_rows;
    let stray = (rows.y..rows.y + rows.height)
        .flat_map(|y| (rows.x..rows.x + rows.width).map(move |x| (x, y)))
        .find(|(x, y)| h.cell_is_match(*x, *y));
    if let Some((x, y)) = stray {
        println!("     stray match cell at ({x}, {y}): row {:?}", h.row(y));
    }
    check(stray.is_none(), "esc: visible rows are plain");
    check(
        !h.app.reader.header.contains("  · "),
        "esc: header has no match count",
    );

    // 5. The search box completes operators and live tags.
    h.press("slash");
    check(
        h.text().contains("\"phrase\"  -term"),
        "hint row shows with the box",
    );
    h.type_text("@to");
    check(
        h.app.notes.search.suggestion.as_deref() == Some("@todo"),
        "@to suggests @todo",
    );
    h.press("right");
    check(
        h.app.notes.search.value == "@todo",
        "→ accepts the completion",
    );
    h.press("enter");
    wait(&mut h, "@todo search", |app| {
        app.notes.header.starts_with("“@todo”") && !app.notes.is_empty()
    })
    .await;
    let notes = h.app.notes.notes.clone();
    check(
        notes.iter().all(|n| n.todos > 0),
        &format!("@todo lists {} notes, all with open todos", notes.len()),
    );
    h.press("slash");
    h.app.notes.search.value.clear();
    h.app.notes.search.cursor = 0;
    h.type_text(&format!("#{tag_prefix}"));
    let suggestion = h.app.notes.search.suggestion.clone().unwrap_or_default();
    check(
        suggestion.starts_with(&format!("#{tag_prefix}"))
            && suggestion.len() > tag_prefix.len() + 1,
        &format!("#{tag_prefix} suggests {suggestion:?}"),
    );
    let tags = h.app.query_tags();
    check(
        tags.contains(&suggestion.trim_matches('#').to_string()),
        "suggestion is a tag from the snapshot",
    );
    h.press("escape");
    check(
        !h.app.notes.search.open && !h.text().contains("\"phrase\"  -term"),
        "esc hides the box and the hint",
    );

    // 6. A scratch note: create → edit → pin → export → trash → restore → trash.
    let title = format!(
        "Bjorn gate {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    h.press("n");
    h.type_text(&title);
    h.press("enter");
    h.app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::End,
        crossterm::event::KeyModifiers::NONE,
    ));
    for _ in 0..40 {
        h.press("backspace");
    }
    h.type_text("techne/dev");
    h.press("enter");
    let t = title.clone();
    wait(&mut h, "scratch note created and edited", move |app| {
        app.reader.note.as_ref().is_some_and(|n| n.title == t)
            && app.reader.plain_text().contains("gate edit line")
    })
    .await;
    let note = h
        .app
        .snapshot
        .notes
        .iter()
        .find(|n| n.title == title)
        .cloned()
        .expect("scratch note in the snapshot");
    check(
        note.tags.contains(&"techne/dev".to_string()),
        &format!("created {:?} ({}) tagged #techne/dev", note.title, note.id),
    );
    let content = client.cat(&note.id).await.expect("cat scratch");
    check(
        content.content.ends_with("gate edit line\n"),
        "editor round trip wrote back through overwrite --base",
    );
    h.app.notes.select_id(&note.id);
    h.press("p");
    let id = note.id.clone();
    wait(&mut h, "pinned", move |app| {
        app.snapshot.by_id(&id).is_some_and(|n| n.pinned_globally())
    })
    .await;
    check(true, "p pins globally");
    h.app.notes.select_id(&note.id);
    h.press("p");
    let id = note.id.clone();
    wait(&mut h, "unpinned", move |app| {
        app.snapshot
            .by_id(&id)
            .is_some_and(|n| !n.pinned_globally())
    })
    .await;
    check(true, "p again unpins");
    h.app.notes.select_id(&note.id);
    h.press("x");
    h.press("enter");
    let path = match h.app.overlay.clone() {
        Some(bjorn::ui::modals::Overlay::Text { field, .. }) => {
            std::path::PathBuf::from(field.value)
        }
        other => {
            println!("FAIL expected the export path prompt, got {other:?}");
            std::process::exit(1);
        }
    };
    h.press("enter");
    let p = path.clone();
    wait(&mut h, "export written", move |_| p.exists()).await;
    let exported = std::fs::read_to_string(&path).unwrap_or_default();
    check(
        exported.contains("gate edit line"),
        &format!("x exported Markdown to {}", path.display()),
    );
    let _ = std::fs::remove_file(&path);
    h.app.notes.select_id(&note.id);
    h.press("d");
    h.press("y");
    let id = note.id.clone();
    wait(&mut h, "trashed", move |app| {
        app.snapshot
            .by_id(&id)
            .is_some_and(|n| n.location == Location::Trash)
    })
    .await;
    check(true, "d + y trashes");
    h.press("7");
    let id = note.id.clone();
    wait(&mut h, "trash view lists it", move |app| {
        app.notes.notes.iter().any(|n| n.id == id)
    })
    .await;
    h.app.notes.select_id(&note.id);
    h.press("u");
    let id = note.id.clone();
    wait(&mut h, "restored", move |app| {
        app.snapshot
            .by_id(&id)
            .is_some_and(|n| n.location == Location::Notes)
    })
    .await;
    check(true, "u restores from the Trash view");
    h.press("1");
    let id = note.id.clone();
    wait(&mut h, "notes view lists it", move |app| {
        app.notes.notes.iter().any(|n| n.id == id)
    })
    .await;
    h.app.notes.select_id(&note.id);
    h.press("d");
    h.press("y");
    let id = note.id.clone();
    wait(&mut h, "trashed again", move |app| {
        app.snapshot
            .by_id(&id)
            .is_some_and(|n| n.location == Location::Trash)
    })
    .await;
    check(
        true,
        &format!("scratch note left in Bear's trash ({})", note.id),
    );
    let _ = std::fs::remove_file(&editor);
    println!("GATE PASSED");
}

async fn bench() {
    let started = Instant::now();
    let client = Arc::new(BearClient::new(vec![resolve_bearcli("")]));
    let mut h = Harness::new(config(), client.clone(), None, (140, 44));
    h.load().await;
    let first_frame = started.elapsed();
    let notes = h.app.snapshot.notes.len();

    let fresh = BearClient::new(vec![resolve_bearcli("")]);
    let t = Instant::now();
    let snap = fresh.snapshot().await.expect("cold snapshot");
    let cold = t.elapsed();
    let t = Instant::now();
    fresh.snapshot().await.expect("warm snapshot");
    let warm = t.elapsed();
    let t = Instant::now();
    fresh.probe().await.expect("probe");
    let probe = t.elapsed();

    let longest = snap
        .notes
        .iter()
        .filter(|n| !n.locked)
        .max_by_key(|n| n.length)
        .expect("a note");
    let content = fresh.cat(&longest.id).await.expect("cat longest");
    let t = Instant::now();
    let lines = markdown::render(&content.content);
    let rows: usize = lines.iter().map(|l| markdown::wrap(l, 100).len()).sum();
    let render = t.elapsed();
    let t = Instant::now();
    h.app.notes.select_id(&longest.id);
    let note = longest.clone();
    h.app.schedule_preview(note, true, true);
    let id = longest.id.clone();
    h.until(|app| {
        app.reader.note.as_ref().is_some_and(|n| n.id == id) && app.reader.full_text.is_some()
    })
    .await;
    h.draw();
    let show = t.elapsed();

    println!("implementation=rust");
    println!("notes={notes}");
    println!("first_frame_ms={:.0}", first_frame.as_secs_f64() * 1000.0);
    println!("cold_snapshot_ms={:.0}", cold.as_secs_f64() * 1000.0);
    println!("warm_snapshot_ms={:.0}", warm.as_secs_f64() * 1000.0);
    println!("probe_ms={:.0}", probe.as_secs_f64() * 1000.0);
    println!(
        "longest_note_bytes={} lines={} rows={rows}",
        content.content.len(),
        content.content.lines().count()
    );
    println!("render_longest_ms={:.1}", render.as_secs_f64() * 1000.0);
    println!("show_longest_ms={:.0}", show.as_secs_f64() * 1000.0);
}

/// Input-to-frame latency for the interactions that are felt: moving the
/// list cursor, the reader following it, switching views, cycling focus,
/// and a broad search. Each figure is the median of `n` repetitions.
async fn latency() {
    let client = Arc::new(BearClient::new(vec![resolve_bearcli("")]));
    let mut h = Harness::new(config(), client.clone(), None, (140, 44));
    h.load().await;
    let n = h.app.notes.len();
    println!("implementation=rust notes={n}");

    fn median(mut v: Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;

    // 1. j: key handled and the frame drawn (the reader waits for the debounce).
    let mut key_frame = Vec::new();
    let mut reader_follow = Vec::new();
    for _ in 0..20 {
        let before = h.app.reader.note.as_ref().map(|n| n.id.clone());
        let t = Instant::now();
        h.press("j");
        key_frame.push(ms(t.elapsed()));
        let t = Instant::now();
        let prev = before.clone();
        h.until(move |app| {
            app.reader.note.as_ref().map(|n| n.id.clone()) != prev && app.reader.full_text.is_some()
        })
        .await;
        reader_follow.push(ms(t.elapsed()));
    }
    println!("cursor_key_to_frame_ms={:.1}", median(key_frame));
    println!(
        "cursor_to_reader_ms={:.0}   (includes the 120 ms debounce and one bearcli cat)",
        median(reader_follow)
    );

    // 2. View switches on the whole library: list rebuilt and drawn.
    let mut views = Vec::new();
    for _ in 0..10 {
        let t = Instant::now();
        h.press("2");
        h.press("1");
        views.push(ms(t.elapsed()) / 2.0);
    }
    println!("view_switch_ms={:.1}", median(views));

    // 3. tab through the three panes.
    let mut tabs = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        h.press("tab");
        tabs.push(ms(t.elapsed()));
    }
    println!("tab_focus_ms={:.2}", median(tabs));

    // 4. A frame with nothing changed.
    let mut frames = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        h.draw();
        frames.push(ms(t.elapsed()));
    }
    println!("idle_frame_ms={:.2}", median(frames));

    // 5. A broad search: results listed and drawn.
    h.app.set_focus(Pane::Notes);
    let t = Instant::now();
    h.press("slash");
    h.type_text("the");
    h.press("enter");
    h.until(|app| app.notes.header.starts_with("“the”")).await;
    println!(
        "search_the_ms={:.0} results={}",
        ms(t.elapsed()),
        h.app.notes.len()
    );
    h.press("escape");
    h.until(|app| app.search_query.is_empty() && app.notes.len() == n)
        .await;

    // 6. Sidebar: F expands every tag, then j walks 20 rows (each applies a tag selection).
    h.app.set_focus(Pane::Sidebar);
    let t = Instant::now();
    h.press("F");
    println!(
        "fold_all_ms={:.1} rows={}",
        ms(t.elapsed()),
        h.app.sidebar.rows.len()
    );
    h.app
        .sidebar
        .move_to_tag(&h.app.sidebar.tag_roots()[0].clone());
    let mut side = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        h.press("j");
        side.push(ms(t.elapsed()));
    }
    println!("sidebar_step_ms={:.1}", median(side));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--bench") {
        bench().await;
        return;
    }
    if args.first().map(String::as_str) == Some("--latency") {
        latency().await;
        return;
    }
    let body = args
        .first()
        .cloned()
        .unwrap_or_else(|| "hash-guarded".into());
    let title = args.get(1).cloned().unwrap_or_else(|| "Bjorn".into());
    let tag = args.get(2).cloned().unwrap_or_else(|| "tec".into());
    gate(body, title, tag).await;
}
