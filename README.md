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
git clone https://github.com/7robots/bjorn.git
cd bjorn
./install.sh          # builds --release, puts a `bjorn` launcher in ~/bin
bjorn                 # or: cargo run --release --bin bjorn
bjorn --tag work      # start scoped to a tag subtree
bjorn --demo          # sample notes through the built-in fake bearcli, no Bear needed
```

The launcher was `bjorn-rs` while the Python build held the `bjorn` name; the
Python build is archived at `7robots/bjorn-python` and the name is this one's now.

## Themes

The palette comes from `theme` in the shared config file, or `--theme` on the
command line (`--list-themes` prints them):

| Name | |
|---|---|
| `textual-dark` | the default; what the Python Bjorn draws through Textual's own theme, matched colour for colour |
| `red-graphite` | Bear's Red Graphite: the graphite sidebar beside a white page, coral red (`#CD5654`) on the focused column, the cursor, the bullets, the links and the tags |
| `red-graphite-dark` | the same red over Bear's graphite, for a dark terminal |

```sh
bjorn --theme red-graphite
```

```toml
# ~/.config/bjorn/config.toml
theme = "red-graphite"
```

The Python Bjorn carries the same three, so one config file dresses both. An
unknown name in the config falls back to the default with a warning rather
than stopping the app, because the config is shared and the Python build also
accepts Textual's own themes. An unknown `--theme` on the command line is an
error.

## Actions

`x` exports a note to a file you pick. `!` and `a` hand that same file to a
command of yours: publish it, copy it, POST it, push it.

```toml
# ~/.config/bjorn/config.toml
[[actions]]
name = "Publish to S3"
command = 'aws s3 cp "$BJORN_NOTE_FILE" "s3://notes/$BJORN_NOTE_TITLE.html"'
format = "html"        # md (default), html, txt, rtf, textbundle
confirm = true         # ask first
default = true         # this is what `!` runs

[[actions]]
name = "Copy as plain text"
command = "pbcopy"
format = "txt"
```

`!` runs the default action; `a` opens the palette — a search box over your
actions, filtered as you type, `enter` runs. The note is rendered the way
export renders it, written to a temp file the command gets as
`$BJORN_NOTE_FILE` (and on stdin), with the title, id, tags and stamps in the
environment; the file goes away when the command ends. The first line the
command prints comes back as a toast, and a non-zero exit is reported with its
stderr. Full reference: [docs/actions.md](docs/actions.md).

## What differs from the Python Bjorn

- The reader renders the whole note in one pass and draws only the viewport.
  The Python version renders long notes in two halves (80 lines, then the
  rest) and mounts the notes list in windows of 120 rows; neither exists here.
- The reader does not wait for a note it already has: the cursor's neighbours
  are read ahead, a cold start keeps the bodies the preview listing had to read
  anyway, and the 120 ms debounce applies only to a body that has to come from
  bearcli. Holding `j` down scrolls the reader with the list.
- Search highlighting reaches fenced code and table cells.
- Every `bearcli` and `remctl` call has a 30 s timeout.
- `mouse_pixels` and `--no-mouse-pixels` are accepted and ignored: the mouse
  always stays in cell mode, so the SwiftTerm workaround is moot.
- HTML export percent-encodes image URLs that point at paths.

## Measurements

Both implementations against the same live library, headless, under
`caffeinate`. Timings that go through `bearcli` are the same in both, as they
should be: the Rust build changes what happens after bearcli answers.

| | Python (Textual) | Rust (ratatui) |
|---|---|---|
| Start to first frame, previews cached | 883 ms | 440 ms |
| Start to first frame, no cache | 1331 ms | 1197 ms |
| Cold snapshot (`list` with content) | 1143 ms | 1022 ms |
| Warm snapshot (metadata only) | 301 ms | 293 ms |
| Change probe (two `list` calls) | 63 ms | 40 ms |
| Longest note (97 KB, 1778 lines): render and show | 5437 ms | 46 ms |
| Peak resident memory | 379 MB | 43 MB |
| Installed size | 23 MB venv plus Python 3.12 | 4.2 MB binary |

Input to frame, median of repeated presses on a 213-note library:

| | Python (Textual) | Rust (ratatui) |
|---|---|---|
| `j` in the notes list, key to frame | 120 ms | 0.4 ms |
| the reader catching up with the cursor | 55 ms | 0 ms |
| switching smart views (whole library) | 307 ms | 0.4 ms |
| `tab` between columns | 106 ms | 0.3 ms |
| a frame with nothing changed | 43 ms | 0.3 ms |
| `F`, unfolding every tag | 213 ms | 0.4 ms |

Everything the app does itself lands inside a frame; what is left is bearcli,
which both pay equally. Two things made the difference to how it feels: the
previews are kept in `~/.cache/bjorn/previews.json` between runs, so a launch
lists metadata instead of reading every body to build them again; and the
reader never waits for a note it already has — the cold listing's bodies are
kept, the cursor's neighbours are read ahead, and only a body that has to come
from bearcli waits out the debounce. The long-note number is the other
difference that is felt: Textual mounts one widget per markdown block, ratatui
draws styled lines.

## Development

```sh
cargo test                                 # 189 tests: unit, client, UI through a headless harness
cargo clippy --all-targets -- -D warnings
cargo run --release --bin bjorn-gate       # acceptance gate against the live library
cargo run --release --bin bjorn-gate -- --bench
cargo run --release --bin bjorn-gate -- --latency
cargo run --example shot -- --theme red-graphite --out /tmp/shot.json
uv run --with pillow python tools/shot.py /tmp/shot.json /tmp/shot.png
```

The Python suite runs against the Rust fakes with the plugin in `tools/`:

```sh
cd ../bjorn-python
PYTHONPATH=../bjorn/tools uv run pytest -p pytest_rust_fake -q
```

The plan and its status live in `docs/plans/bjorn-rust.md`; deferred work in
`docs/ROADMAP.md`. Module names mirror the Python package so the two trees
read side by side.

## Acknowledgements

Bjorn exists because of [Shiny Frog](https://shinyfrog.net) and Bear, and
`bearcli` is what makes a terminal client possible at all. Thank you.
