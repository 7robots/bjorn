# Roadmap

Single source of truth for planned and deferred work in bjorn (the Rust build). The active
plan lives in `docs/plans/bjorn-rust.md`.

## Next

  or the two keep coexisting (gate passed 2026-09-11; numbers in the README).
- Search-box completion candidates could be shown as a popup list now that
  the box is drawn by hand; the Python version was limited to one ghost line.
- A theme that follows the terminal's own palette, for people who dress their
  terminal and want the app to match. Every theme here is true color, which is
  what makes the two implementations agree.

## Deferred (carried from bjorn's ROADMAP, still open here)

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
- Hugo publishing, as an action rather than a key: `contrib/hugo-publish`
  (Python 3, standard library only) writes the note an action hands over as a
  post, a draft unless the entry says `--live`. It started as `P` inside the
  binary (PR #11) and moved out on 2026-09-25, the user's call: publishing is
  an integration like AI, which already goes through actions, so Bjorn keeps
  no Hugo knowledge, no `[hugo]` config and no YAML dependency. The guards
  the built-in had, and the review's fixes (never replacing a hand-written
  post or an existing image, front matter written key by key and quoted, no
  YAML aliases, wiki and local links reduced to text, shortcodes and raw
  HTML refused), live in the script and `tests/hugo_action.rs`. See
  `docs/actions.md`.
- PDF export, as the sixth format in the picker. The headless-browser concern
  recorded in bjorn's ROADMAP is why nothing draws a page inside the binary:
  the HTML rendering is handed to a converter the user already has, WeasyPrint
  or a Chromium browser (Chrome, Chromium, Brave, Edge, Vivaldi), the way RTF
  is handed to `textutil`. macOS ships neither (its own `cupsfilter` refuses
  HTML), so a machine without one gets a plain error naming both rather than a
  format that fails blankly. A browser is given a
  throwaway profile and no network, and the note's body handed to either
  converter is parsed and rebuilt from an allowlist (`ammonia`): no scripts,
  styles, frames or SVG, and an image only when it is already a `data:` URI.
  A note's inline HTML would otherwise reach the converter as written, and a
  remote image in one is a note telling somebody it was printed, while a local
  one bakes a file off the disk into a PDF that is usually about to be sent
  on. Attachments are `data:` URIs by then, so nothing that was going to print
  is lost, and the note's own words (a `url(` in a sentence, `<img>` in a code
  block) are text to the parser and come through untouched.
- Work notes: `D` opens today's daily note (title, dated tag and template
  from `[daily]`, found or made with `create --if-not-exists`), `N` makes a
  note from a template in `~/.config/bjorn/templates/` with `{{date}}`-style
  placeholders, and `bjorn capture` / `bjorn today` reach the daily note from
  the shell. Example templates ship in `config/templates/`. Daily notes are
  opt-in (the maintainer's call in the #10 review): off until the config has
  a `[daily]` table, since Bear itself has none; templates are always on.
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
