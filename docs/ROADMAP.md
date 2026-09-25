# Roadmap

Single source of truth for planned and deferred work in bjorn (the Rust build). The active
plan lives in `docs/plans/bjorn-rust.md`.

## Next

  or the two keep coexisting (gate passed 2026-09-11; numbers in the README).
- Search-box completion candidates could be shown as a popup list now that
  the box is drawn by hand; the Python version was limited to one ghost line.
- A theme that follows the terminal's own palette, for people who dress their
  terminal and want the app to match. Every theme here is true colour, which is
  what makes the two implementations agree.

## Deferred (carried from bjorn's ROADMAP, still open here)

- PDF written by Bjorn itself (the headless-browser concern recorded in bjorn's
  ROADMAP still stands: no converter in the binary). The HTML export's print
  stylesheet and an action are the way there — docs/actions.md.
- EPUB export.
- Math in the HTML export. Bear renders `$x$` and `$$…$$`; Bjorn prints them as
  the note wrote them, because typesetting them needs an engine (KaTeX, MathML
  with a font) that the binary does not carry and would not be able to fetch.
  Ruled fine as it stands 2026-09-19, with the note that the text is at least
  never lost. Bear's callout icons are the other thing an exported note does
  not draw — the panel's color and bar carry the type instead.
- Search-box completion inside a multi-word tag; completion candidate cycling.
- Notes list snippet line (first body line, as Bear shows).
- Permanent delete from Trash (no bearcli command yet).
- Attachments listing and save; tag rename/delete; archive from the TUI;
  locked-note explanation; change detection smarter than polling; triage
  follow-ons (edit todo text, snooze, un-tick `@done`, "Todo in workspace"
  view).

## Closed since

- An action's output can go back into Bear (`output` in its config entry):
  `append` adds it to the note or under a `section` heading, `new-note` makes a
  note of it, tagged the way `n` tags one, and `replace` writes it over the
  note after asking, hash-guarded like the editor, keeping the old text in a
  temp file. Empty output, a failed command, anything over 1 MB or not UTF-8,
  and a note trashed meanwhile never write; output that is not written is
  kept in a private temp file. An action whose entry cannot work (an unknown
  `output`, `replace` on a non-Markdown format) does not run at all.
- An action can take the window and the keyboard (`interactive = true`), running
  in a pty like the editor does, for commands that ask their own questions
  rather than printing one line and leaving.
- Wiki links: `[[Title]]`, `[[Title/Heading]]` and `[[Title|shown text]]` are
  drawn as links and followed by a click or from the `L` list, which also
  shows the notes linking in (a `bearcli search` for `[[Title` in Notes and
  the Archive, at most 200 candidates per search, each hit's body checked, so
  prefixes and mentions in code are dropped). A missing title offers to create the note; `backspace` / `alt+→`
  walk back and forward.
- An action can ask for one line of text before it runs (`prompt` in its
  config entry, the answer in `$BJORN_ACTION_INPUT`). One action covers what
  used to need one entry per argument; with `confirm` as well the dialog
  quotes the answer.
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

- Section navigation: `o` opens an outline of the note's headings, indented
  by level and searchable, and `enter` scrolls the reader to one; `}` / `{`
  step between headings, and the reader's footer names the current section.
  The headings come from the reader's own markdown parse rather than
  `bearcli outline`: the reader has the body already and has to map a heading
  to a wrapped row anyway, so this is instant, offline and exact about where
  each heading is drawn. Fenced code is excluded the same way Bear excludes it.

## Closed by design in the Rust port

- Long-note render time and the head-then-rest truncation: the renderer draws
  only the viewport.
- Notes list windowing in 120-row batches: same reason.
- Search highlights inside fenced code and tables: spans over rendered lines
  cover them.
- Reader match count wrong before the first `]` on a truncated note: nothing
  is truncated.
