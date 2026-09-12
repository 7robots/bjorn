# Bjorn in Rust
Bear mirror: 7ED80908-BD76-459C-A416-22F8BA784083

Status: complete 2026-09-11; all phases done, acceptance gate passed.

A re-implementation of Bjorn (`~/GitHub/bjorn`, Python + Textual) in Rust with
ratatui, to find out how well the design works as a native binary: startup,
reload, rendering of long notes, memory, and install without a Python runtime.
Feature parity with bjorn at commit `e60f93e` is the target; the Python repo is
untouched. Phase numbers continue from bjorn's last (21).

## Rulings

- 2026-09-11 — "approved, let's do it!" (truncation and windowing dropped, benchmark table in README, as proposed).

## Decisions

- **Toolchain.** `brew install rustup`, stable channel, edition 2024. `cargo` for
  everything; no Python in the repo. `rustfmt` and `clippy -D warnings` clean.
- **Crates.** `ratatui` + `crossterm` (event stream), `tokio` (process, sync,
  time), `pulldown-cmark`, `serde`/`serde_json`, `toml`, `clap`, `chrono`,
  `regex`, `unicode-width`, `tempfile`, `sha1`, `base64`, `percent-encoding`,
  `thiserror`/`anyhow`. Dev: `pretty_assertions`. No snapshot-test crate.
- **Layout of the crate.** One package, four binaries: `bjorn`, `fake-bearcli`,
  `fake-remctl`, `bjorn-gate`. Library modules mirror the Python ones so the two
  trees can be read side by side: `bear` (client), `model`, `render`,
  `render_html`, `search`, `search_box`, `todos`, `reminders`, `export`,
  `icons`, `config`, `app` (state + update), `ui/` (sidebar, note_list,
  note_view, modals, triage, search input), `fake/` (the two fakes' engines).
- **bearcli contract unchanged.** Same argv, same `--fields` lists, same JSON
  field names and error shapes, conflict detected by the same stderr substring,
  `escape_flag` only on `--find`/`--replace`/`--section`. Binary resolution
  order and `$BJORN_BEARCLI` as in Python. Preview cache (stamp-keyed, 2 s
  recency rule, 24-stale threshold, `cat` concurrency 6), the two-call probe, and
  the write lock port verbatim. One addition: every bearcli call gets a 30 s
  timeout and reports a `timeout` error, since Python has none and a hung
  bearcli hangs the app.
- **Fakes are Rust binaries with the Python fakes' contract.** Same state-file
  env vars (`BJORN_FAKE_BEAR_STATE`, `BJORN_FAKE_REMCTL_STATE`), same default
  paths under `~/.cache/bjorn/`, same JSON shapes, same seven seed notes, same
  `.opened` side file, same atomic writes, same query engine. `bjorn --demo`
  locates them next to its own executable. Both fakes must pass the Python
  suite's expectations; the acceptance gate proves it by running bjorn's
  pytest suite with `BJORN_BEARCLI` pointed at the Rust fake.
- **Runtime shape.** Single UI task owns `App` state and draws each frame from
  it. Bearcli work runs in tokio tasks and reports back on one `mpsc` channel as
  `Msg` values tagged with a generation number; stale results are dropped, never
  cancelled mid-render. The Python idioms that survive: `_load_gen`, the
  content cache (64 entries, epoch counter), the busy flag pausing the poll,
  `PREVIEW_DEBOUNCE` 120 ms. Modals are an `Overlay` enum on the state, and
  multi-step flows (new note → create → editor; export → format → path → write)
  are explicit state machines instead of `push_screen_wait`.
- **Markdown is rendered by Bjorn, not a widget.** `pulldown-cmark` events →
  styled `Line`s with a block index per line. Bear syntax handled natively:
  `- [ ]`/`- [x]` → `☐`/`☑`, `==x==` reverse style, `~x~` underline, tag lines
  as a tag row, fences with a plain fixed style, tables as aligned columns.
  Because ratatui draws only the viewport, the whole note is rendered once and
  scrolling is free, so the 80/200-line head-then-rest truncation and the
  120-row notes-list windowing are not ported. Search highlighting becomes span
  styling over those lines, and fenced code and table cells are highlighted too
  (closes three bjorn ROADMAP lines). Match navigation and counts keep bjorn's
  semantics: blocks, not lines, are the unit.
- **Terminal.** Alternate screen, raw mode, cell-mode mouse capture. crossterm
  never negotiates pixel mouse mode, so the Tecolot workaround is moot;
  `mouse_pixels` and `--no-mouse-pixels` are accepted and ignored so the shared
  config file keeps working. Editor: `$VISUAL`/`$EDITOR`/`vim` (config
  `editor` wins) runs in a pty (`portable-pty`) sized to the reader pane; a
  `vt100` screen parses its output and `tui-term` draws it there, keys and
  mouse are encoded back as xterm sequences (`src/pty.rs`). The headless
  harness takes the same path. Toasts are a timed queue drawn bottom-right.
- **Config and icons.** Same `~/.config/bjorn/config.toml`, same keys and
  defaults, so both implementations can share it. `lucide-codepoints.json`
  copied and embedded with `include_str!`; the same style detection.
- **Install.** `install.sh` builds `--release` and puts the binary at
  `~/bin/bjorn-rs`, leaving the Python `~/bin/bjorn` in place for the
  side-by-side comparison. Renaming is a decision for after the gate.
- **Tests.** Unit tests beside each module. Integration tests in `tests/` spawn
  `env!("CARGO_BIN_EXE_fake-bearcli")` as a real subprocess, as bjorn does. UI
  tests drive `App` with a `TestBackend` and injected `Event`s through a
  `Harness` that runs the same update loop and yields the buffer as text;
  a `wait_until` helper polls with a 5 s cap. Fake editors are shell scripts.
- **Not ported.** `render.snippet` (dead), `SafeSelectMixin` and `QuietFooter`
  (Textual bug insurance), `tools/mouseprobe.py`, `tools/screenshot.py`.
  ROADMAP items of bjorn stay unbuilt; the ones this port makes moot are
  listed in this repo's ROADMAP as closed by design.

## Keys

Identical to bjorn's README table, including `1`–`7`, `c`, `F`, `W`, `]`/`[`,
the search box's `tab`/`→` acceptance, and the triage screen's bindings.

## Phases

### Phase 22 — Toolchain, client, model, fakes
Install rustup; create the crate; port `config` + CLI flags, `bear` (every
command, error contract, timeout, preview cache, probe), `model` (views,
selection, sort, tag tree, duplicate titles), and the `fake-bearcli` and
`fake-remctl` binaries. Client tests run the fake as a subprocess and repeat
`test_bear.py`'s call-shape assertions.
Verify: `cargo test` and `cargo clippy --all-targets -- -D warnings`
Status: [x] done 2026-09-11 (43 passed; clippy clean; live library listed: 1002 notes)

### Phase 23 — Read-only TUI
Three columns, sidebar (views, tag tree, fold, `F`, workspace `w`/`W`, icons,
right-aligned counts), notes list (4-row items, lead line), reader with the
native renderer, column cycle `c`, poll loop, `r`, help, quit confirm,
`--demo`, `--tag`. The `Harness` lands here with the first UI tests.
Verify: `cargo test`
Status: [x] done 2026-09-11 (87 passed: 48 unit, 10 client, 29 UI; demo checked in tmux)

### Phase 24 — Writes, export, search
Editor round trip with hash guard and kept temp file on conflict, duplicate
title warning, `n` create, `d` trash, `u` restore, `p` pin, `b` open,
`x` export in md/html/txt/rtf(+rtfd)/textbundle via `textutil`, search box
with operator and tag completion, sub-tag rewrite, reader and row highlighting,
`]`/`[`, `enter` to first match.
Verify: `cargo test`
Status: [x] done 2026-09-11 (134 passed: 66 unit, 10 client, 29 UI, 13 search, 16 writes)

### Phase 25 — Triage and Reminders
Todo parser and key scheme, triage screen (mark, tick via `edit`, go to note,
open in Bear at section, filter, reload), `[reminders]` mode with remctl
add/read-back and the ⏰/✓ glyphs; demo forces reminders on with the fake.
Verify: `cargo test`
Status: [x] done 2026-09-11 (153 passed: 73 unit, 10 client, 29 UI, 13 search, 16 writes, 12 triage)

### Phase 26 — Acceptance gate
1. `cargo run --release --bin bjorn-gate` against live Bear: bjorn's
   `gate_search` and `gate_search_box` checks, plus a create → edit → pin →
   export → trash → restore round trip on a scratch note it makes and removes.
2. bjorn's Python suite passes with `BJORN_BEARCLI` set to the Rust
   `fake-bearcli` (contract proof).
3. Benchmarks, under `caffeinate`, run twice, on the live library: cold start
   to first frame, warm reload, render of the longest note, peak RSS, binary
   size; Python bjorn measured the same way. Table goes in this repo's README.
4. `./install.sh`, then `~/bin/bjorn-rs --demo` from a fresh shell.
Verify: the four steps above, in order, all passing
Status: [x] done 2026-09-11 — bjorn-gate GATE PASSED against 1002 live notes (scratch note 6F4B13C5 left in Bear's trash); Python suite 170 passed against the Rust fakes; benchmarks in README (longest note 5.8 s → 62 ms, RSS 379 → 43 MB); ~/bin/bjorn-rs --demo from a clean shell
