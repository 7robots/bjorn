# Bjorn in Rust

A terminal front end for [Bear](https://bear.app), written in Rust with
[ratatui](https://ratatui.rs). It is a re-implementation of
[Bjorn](https://github.com/7robots/bjorn) (Python and Textual), built to find
out how the same design behaves as a native binary. Everything goes through
`bearcli`, the command line tool that ships inside Bear.app; the two
implementations share one config file and one feature set, and the Rust fakes
of `bearcli` and `remctl` pass the Python suite.

Three columns, like the app: smart views and a nested tag tree on the left,
the notes list in the middle, the rendered note on the right. Editing is
delegated to `$VISUAL`, then `$EDITOR`, then `vim`, which runs in a
pseudo-terminal drawn inside the reader pane so the other columns stay up;
writes back are hash-guarded. Search uses Bear's syntax through `bearcli search`, with
operator and tag completion in the box and match highlighting in the reader.
`t` opens todo triage, with Apple Reminders through `remctl` when enabled.
The keys, the config and the triage screen are documented in the Python
project's README; they are the same here.

## Install

Requires macOS with Bear installed and a Rust toolchain (`brew install rustup
&& rustup default stable`).

```sh
git clone https://github.com/7robots/bjorn-rust.git
cd bjorn-rust
./install.sh          # builds --release, puts a `bjorn-rs` launcher in ~/bin
bjorn-rs              # or: cargo run --release --bin bjorn
bjorn-rs --tag work   # start scoped to a tag subtree
bjorn-rs --demo       # sample notes through the built-in fake bearcli, no Bear needed
```

The launcher is `bjorn-rs` so the Python `bjorn` can stay installed beside it.

## What differs from the Python Bjorn

- The reader renders the whole note in one pass and draws only the viewport.
  The Python version renders long notes in two halves (80 lines, then the
  rest) and mounts the notes list in windows of 120 rows; neither exists here.
- Search highlighting reaches fenced code and table cells.
- Every `bearcli` and `remctl` call has a 30 s timeout.
- `mouse_pixels` and `--no-mouse-pixels` are accepted and ignored: the mouse
  always stays in cell mode, so the SwiftTerm workaround is moot.
- HTML export percent-encodes image URLs that point at paths.

## Measurements

Both implementations against the same live library of 1002 notes, headless,
under `caffeinate`, each number the second of two runs that agreed. Timings
that go through `bearcli` are the same in both, as they should be: the Rust
build changes what happens after bearcli answers.

| | Python (Textual) | Rust (ratatui) |
|---|---|---|
| Start to first frame | 1189 ms | 1088 ms |
| Cold snapshot (`list` with content) | 986 ms | 893 ms |
| Warm snapshot (metadata only) | 291 ms | 286 ms |
| Change probe (two `list` calls) | 38 ms | 36 ms |
| Longest note (169 KB, 3313 lines): render and show | 5848 ms | 62 ms |
| Peak resident memory | 379 MB | 43 MB |
| Installed size | 23 MB venv plus Python 3.12 | 4.2 MB binary |

The Rust build's own work at startup is under 200 ms of the 1.1 s; the rest is
bearcli listing a thousand notes with their content. The long-note number is
the difference that is felt: Textual mounts one widget per markdown block,
ratatui draws styled lines.

## Development

```sh
cargo test                                 # 153 tests: unit, client, UI through a headless harness
cargo clippy --all-targets -- -D warnings
cargo run --release --bin bjorn-gate       # acceptance gate against the live library
cargo run --release --bin bjorn-gate -- --bench
```

The Python suite runs against the Rust fakes with the plugin in `tools/`:

```sh
cd ~/GitHub/bjorn
BJORN_RUST_BIN=~/GitHub/bjorn-rust/target/release PYTHONPATH=~/GitHub/bjorn-rust/tools \
  uv run pytest -p pytest_rust_fake -q
```

The plan and its status live in `docs/plans/bjorn-rust.md`; deferred work in
`docs/ROADMAP.md`. Module names mirror the Python package so the two trees
read side by side.

## Acknowledgements

Bjorn exists because of [Shiny Frog](https://shinyfrog.net) and Bear, and
`bearcli` is what makes a terminal client possible at all. Thank you.
