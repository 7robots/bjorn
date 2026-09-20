# Roadmap

Single source of truth for planned and deferred work in bjorn (the Rust build). The active
plan lives in `docs/plans/bjorn-rust.md`.

## Next

  or the two keep coexisting (gate passed 2026-09-11; numbers in the README).
- `bjorn capture -- "text"` appending a line into today's section of a
  configured note. The primitive is in place (`sections::append_to_section`);
  the CLI surface is left out here because a `capture` subcommand is landing on
  another branch, and the two must agree on one command rather than fight over
  it.
- Search-box completion candidates could be shown as a popup list now that
  the box is drawn by hand; the Python version was limited to one ghost line.
- A theme that follows the terminal's own palette, for people who dress their
  terminal and want the app to match. Every theme here is true colour, which is
  what makes the two implementations agree.

## Deferred (carried from bjorn's ROADMAP, still open here)

- PDF export (headless-browser concern recorded in bjorn's ROADMAP).
- EPUB export.
- Search-box completion inside a multi-word tag; completion candidate cycling.
- Notes list snippet line (first body line, as Bear shows).
- Permanent delete from Trash (no bearcli command yet).
- Attachments listing and save; tag rename/delete; archive from the TUI;
  section navigation from `bearcli outline` (the day screen navigates to a
  section by its heading, but nothing reads a note's outline yet);
  locked-note explanation;
  change detection smarter than polling; triage follow-ons (edit todo text,
  snooze, un-tick `@done`, "Todo in workspace" view).

## Closed since

- Dated sections: `s` writes today's section into the note under the cursor
  from the `[sections]` template (above the first dated section, so the newest
  stays on top; never a second one for the same day), and `T` opens a day
  screen listing every section written on one day across the notes and the
  archive, with `←`/`→` to step days. Sections are found two ways and the
  results merged: the day's tag, and the day's heading searched as a phrase, so
  a note that dates its sections by heading alone is listed too. The day tag
  and the date heading are strftime patterns, so any dating scheme works. See
  the README.

- Actions: `[[actions]]` in the config, `!` for the default one and `a` for a
  searchable palette. The note is rendered through the exporter, handed to a
  shell command as a temp file and on stdin, and the command's first line comes
  back as a toast. The menu's **+ New action** row adds an entry, `ctrl+e`
  edits one and `ctrl+d` deletes one after asking, all as text edits to the config file, so comments survive. See `docs/actions.md`.
- Persisted body previews: `~/.cache/bjorn/previews.json`, written after every
  snapshot that moved and read at start-up, so a launch lists metadata only
  (918 notes: 1197 ms to the first frame, 440 ms once the cache is there). The
  Python Bjorn writes the same file in the same shape, so either warms the
  other.
- Themes: `theme` in the config, or `--theme`. `red-graphite-dark` is the
  default; every theme Bear ships is converted from its own theme files by
  `tools/bear_theme.py`.

## Closed by design in the Rust port

- Long-note render time and the head-then-rest truncation: the renderer draws
  only the viewport.
- Notes list windowing in 120-row batches: same reason.
- Search highlights inside fenced code and tables: spans over rendered lines
  cover them.
- Reader match count wrong before the first `]` on a truncated note: nothing
  is truncated.
