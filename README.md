# Bjorn

A terminal front end for [Bear](https://bear.app), written in Rust with
[ratatui](https://ratatui.rs). Everything goes through `bearcli`, the command
line tool that ships inside Bear.app, so Bjorn works with Bear open or closed
and never touches the database directly.

![Bjorn: smart views and tags, notes list, rendered note](docs/screenshot.png)

Three columns, like the app: smart views and a nested tag tree on the left,
the notes list in the middle, the rendered note on the right. Editing is
delegated to `$VISUAL`, then `$EDITOR`, then `vim`, which runs in a
pseudo-terminal drawn inside the reader pane so the other columns stay up;
writes back are hash-guarded, so if the note changed in Bear while you were
editing, nothing is written and your version is kept in a temp file. Search
uses Bear's syntax through `bearcli search`, with operator and tag completion
in the box and match highlighting in the reader. `t` opens todo triage, with
Apple Reminders through `remctl` when enabled.

## Install

Requires macOS with Bear installed and a Rust toolchain (`brew install rustup
&& rustup default stable`). Bjorn finds `bearcli` on `PATH` or inside
`/Applications/Bear.app`; set `bearcli = "..."` in the config for anywhere else.

```sh
git clone https://github.com/7robots/bjorn.git
cd bjorn
./install.sh          # builds --release, puts a `bjorn` launcher in ~/bin
bjorn                 # or: cargo run --release --bin bjorn
bjorn --tag work      # start scoped to a tag subtree
bjorn --demo          # sample notes through the built-in fake bearcli, no Bear needed
```

`git pull && ./install.sh` is the update path.

## Keys

| Key | Action | Key | Action |
|---|---|---|---|
| `tab` / `shift+tab` | cycle panes | `/` | search (Bear syntax); `@` and `#` complete, `tab` or `→` accepts; `enter` runs it |
| `j` `k` `↑` `↓` | move within a pane; in the sidebar the cursor runs from the views into the tags; in the reader they scroll | `esc` | clear the search and its highlights |
| `enter` | move into the reader for the highlighted note, at the first match while searching | `1`–`7` | Notes, Untagged, Todo, Today, Pinned, Archive, Trash |
| `n` | new note (title, tags), then edit | `d` | move the note to the trash, after a confirm |
| `e` | edit in `$VISUAL` / `$EDITOR` | `u` | restore from Trash or Archive |
| `p` | toggle the global pin | `x` | export: Markdown, HTML, text, RTF, TextBundle, or Hugo (`←` `→` pick, `enter` confirms) |
| `P` | publish as a Hugo post, after a confirm (see [Publishing to Hugo](#publishing-to-hugo)) | | |
| `b` | open in Bear.app | `!` / `a` | run the default action / open the action menu (see [Actions](#actions)) |
| `w` | make the highlighted tag the workspace; again on it to leave | `W` | clear the workspace |
| `f` | fold / unfold the highlighted tag's subtree | `F` | fold every tag, or unfold them all when all are folded |
| `t` | triage the workspace's open todos | `c` / click `▮▮▮` | hide the tag column, then the note column too, then show all three |
| `]` / `[` | next / previous match in the reader while searching | `r` | refresh now |
| `?` | help (`esc` `q` `?` close it) | `q` | quit, after a confirm |

The **workspace** is a tag subtree that scopes the whole app: the tag tree
shows only it, the smart views count only inside it, search results are
filtered to it, and new notes default to it.

Search goes to `bearcli search` unchanged, so Bear's whole syntax works and
plain terms match body text, not just titles. While you type, `@` completes
bearcli's operators (`@todo`, `@title`, `@last7days`, `@date(`…) and `#`
completes your tags, workspace first, as ghost text that `tab` or `→`
accepts; a one-line cheat sheet sits under the box. Bear matches `#name`
against full tag paths only, so a bare sub-tag (`#Build` for
`#kybernetes/Build`) is completed and searched as `#*/Build`, Bear's
sub-tag form. `enter` runs the search and moves to the results; the box
stays open with the query, dimmed until you press `/` or click it to edit
again. While a search is active the reader highlights the terms, including
inside fenced code and table cells, the header counts the matching blocks,
`]` and `[` step through them, and `enter` on a note lands on its first match.

Views are computed from one `bearcli list` snapshot, so the counts in the
sidebar and the notes list always agree. **Pinned** means any pin, global or
inside a tag. **Today** means modified today, local time.

The reader renders the whole note in one pass and draws only the viewport, so
a 100 KB note shows as fast as a short one. It does not wait for a note it
already has: the cursor's neighbours are read ahead, a cold start keeps the
bodies the preview listing had to read anyway, and only a body that has to
come from bearcli waits out a 120 ms debounce. Holding `j` down scrolls the
reader with the list. Every `bearcli` and `remctl` call has a 30 s timeout.

## Todo triage

`t` opens a screen listing every open `- [ ]` item from the `@todo` notes in
the workspace (all notes when none is set), grouped by note with the section
each item sits under. `space` marks rows, `x` ticks the marked (or highlighted)
items in Bear through `bearcli edit`, `enter` jumps to the note, `b` opens it
in Bear.app at that section, `/` filters, `r` reloads, `esc` or `q` closes.

![Triage: open todos grouped by note, with Reminders status](docs/screenshot-triage.png)

With `[reminders] enabled = true` and [remctl](https://github.com/7robots/remctl)
on your PATH, `a` also pushes marked items to Apple Reminders. Each reminder's
notes carry the note's `bear://` link and a `bear-todo: <key>` line (the same
scheme remtui uses, so reminders it created are recognised); on every load
they are read back and rows show ⏰ for an open reminder or ✓ for one you
completed in Reminders, ready to `x` in Bear. Nothing is written into Bear when
a reminder is added.

## Configuration

`~/.config/bjorn/config.toml` (or `$XDG_CONFIG_HOME/bjorn/config.toml`), or
`--config PATH`. Every key is optional:

```toml
editor = "nvim"               # overrides $VISUAL / $EDITOR
export_dir = "~/Downloads"    # where `x` proposes to write
export_format = "md"          # preselected in the export picker: md | html | txt | rtf | textbundle
poll_seconds = 5              # 0 disables the background refresh
workspace = "work"            # start scoped to this tag
bearcli = "/usr/local/bin/bearcli"  # optional; default searches PATH, then Bear.app
icon_style = "auto"           # auto | nerd | emoji | lucide | none
theme = "red-graphite-dark"   # see Themes below; `bjorn --list-themes` prints the names

[icons]                       # top-level tag -> Lucide icon name, or emoji:<glyph>
tech = "terminal"
school = "emoji:🎓"

[reminders]                   # triage can push todos to Apple Reminders
enabled = false               # off by default
list = "Bear"                 # target list; remctl's default when empty
due = "today"                 # due date for new reminders; "" for none
remctl = ""                   # path to remctl; default searches PATH

[hugo]                        # `P` publishes a note as a Hugo post; see Publishing to Hugo
site = ""                     # the site root (the folder with content/); empty disables `P`

[[actions]]                   # shell commands for `!` and `a`; see Actions below
```

`mouse_pixels` and `--no-mouse-pixels` are accepted from older config files
and ignored: the mouse always stays in cell mode.

Bjorn keeps one cache file, `~/.cache/bjorn/previews.json` (or under
`$XDG_CACHE_HOME`): the notes list's body previews, keyed by note id and
modification stamp. It is what lets a launch list metadata only instead of
reading every note's body to build the previews again. Deleting it costs one
slow start. `--demo` runs keep their own copy, so the sample library never
stands in for the real one.

The poll is cheap: two `bearcli list` probes run together and a reload only
when something changed. A reload lists metadata only and reads the body of
just the notes whose modification time moved.

### Themes

The palette comes from `theme` in the config file, or `--theme` on the
command line (`--list-themes` prints them):

| Name | |
|---|---|
| `red-graphite-dark` | the default: Bear's Red Graphite over graphite, coral red (`#CD5654`) on the focused column, the cursor, the bullets, the links and the tags |
| `red-graphite` | the same red in Bear's light Red Graphite: a graphite sidebar beside a white page |
| `textual-dark` | the original palette: a dark grey page with blue and amber accents |

Every other theme Bear ships is here too, under its Bear name in lower case
with hyphens for spaces:

> `academia` `atom` `ayu` `ayu-mirage` `catppuccin-latte`
> `catppuccin-macchiato` `charcoal` `cobalt` `d-boring` `dark-graphite`
> `dark-notes` `dieci` `dracula` `duotone-heat` `duotone-light`
> `duotone-snow` `everforest-dark` `everforest-light` `gandalf` `gotham`
> `gruvbox` `high-contrast` `lighthaus` `nord` `nord-light` `notes`
> `olive-dunk` `panic-mode` `print` `rose-pine` `rose-pine-dawn`
> `shibuya-jazz` `shibuya-lo-fi` `solarized-dark` `solarized-light`
> `tokyo-night` `tokyo-night-light` `toothpaste`

```sh
bjorn --theme nord
```

They are generated from the theme files inside Bear.app by
`tools/bear_theme.py`, which maps Bear's keys onto the app's palette fields
(page, sidebar, headers, cursor, links, code, tags). The toast colours come
from the palette a theme is named after, or from Bear's highlighter colours
for Bear's own designs; text over the accent is chosen for contrast. When a
Bear update adds a theme, `python3 tools/bear_theme.py > src/ui/palettes.rs`
picks it up. `?` inside the app lists the names and marks the one in use.

An unknown name in the config falls back to the default with a warning rather
than stopping the app. An unknown `--theme` on the command line is an error.

### Icons

Top-level tags and the smart views carry icons: Nerd Font (Material Design)
glyphs when the terminal is Ghostty or WezTerm or a Nerd Font is installed,
emoji otherwise. Built-in defaults cover common top-level tags (`work`, `home`,
`projects`, `ideas`, `journal`, `books`, `reading`, `tech`, `code`, `garden`,
`travel`, `health`, `music`, `robotics`, `school`); anything else gets a tag
glyph. Names are Lucide's (`bot`, `book-open`, `compass`, ...); see
`src/icons.rs` for the table.

`icon_style = "lucide"` draws Lucide's own glyphs from its icon font instead of
Nerd Font look-alikes, and any of Lucide's 2,000+ names works under `[icons]`.
It is opt-in because the terminal has to be told about the font:

```sh
# 1. Install the font. Pin the version Bjorn's codepoint table was built from.
curl -L -o ~/Library/Fonts/lucide.ttf https://unpkg.com/lucide-static@1.43.0/font/lucide.ttf

# 2. Ghostty: route Lucide's codepoint range to it (~/.config/ghostty/config).
font-codepoint-map = U+E038-U+E768=Lucide
```

Open a new Ghostty window afterwards. Kitty's `symbol_map` does the same job.

Two caveats. Lucide reassigns codepoints between releases, so the installed
`lucide.ttf` must match the bundled table (lucide-static 1.43.0; both are noted
in `src/icons.rs`). And U+E000–U+E7FF is where Nerd Fonts keep the
Powerline, Pomicons, Seti and Codicons sets, so that mapping takes those glyphs
away from everything in the window: prompt themes, `eza`/`lsd` file icons,
Neovim statuslines. Bjorn's Material Design glyphs live above U+F0000 and are
unaffected.

## Publishing to Hugo

`P` (or `x` then `p`) turns the note into a post in a [Hugo](https://gohugo.io)
site: one Markdown file with YAML front matter, written under
`content/<section>/` in the site you name. Bjorn only writes files; uploading
images, building and committing stay with you (see the
[recipes](docs/actions.md#hugo-recipes)). Nothing is written until you confirm
a dialog that shows, each on its own line, the note's title, the file, the
URL, draft or live, the tags, any of the note's keys that were not published,
and anything unusual (a slug already taken, a post that went missing).

```toml
[hugo]
site = "~/sites/blog"                 # required: the site root (the folder with content/)
section = "posts"                     # under content/
path = "{year}/{month}/{slug}.md"     # a new post's file: {year} {month} {day} {slug}; ending in /index.md makes a page bundle
permalink = "/:year/:month/:day/:slug/"  # the site's permalink for the section; only shown in the dialog
tag_prefix = "blog"                   # publish only #blog/... tags, without the prefix; "" publishes no tags
publish_tag = "blog/published"        # draft: false with this tag; "" means always a draft
media_url = "https://media.example.com/blog"  # where uploaded images are served from
media_dir = "~/Downloads/bjorn-media" # where images wait for your upload
summary_divider = true                # <!--more--> after the lead paragraph
```

What a new post gets from the note:

| Front matter | From |
|---|---|
| `title` | the note's `# ` title (a plain or `##` first line equal to the note's title also goes), the note's own tags taken out |
| `slug` | the title, lower case with every run of other characters a hyphen (`new-post.sh`'s rule: `C++ & Rust: 2026?` is `c-rust-2026`) |
| `date` | when you publish, local time with its offset, as `new-post.sh` does |
| `draft` | `true` unless the note has `publish_tag` |
| `tags` | the note's tags under `tag_prefix`, prefix removed, most specific only, lower case with hyphens: `#blog/image processing#` is `image-processing`. With no `tag_prefix`, no tags: a public site gets only tags you meant for it |
| `description` | `""`, for you to fill in |

A note can start with its own front matter between `---` fences, at the very
top and nowhere else. Only `title`, `slug`, `date`, `description`, `cover`,
`tags`, `draft`, `summary` and `showtoc` are published, written fresh from what
was parsed; anything else (`layout`, `url`, `aliases`, `markup`, `outputs`,
`build`, `type`...) is dropped and listed in the dialog. A note that starts
with `---` but not with YAML keys is refused.

**Publishing again** updates the same file, even after the note's title
changed: which note wrote which file is kept in `hugo-published.json` beside
the config file, outside the site, so the post carries no Bear id and no local
path. `slug` and `date` stay as the post has them (the note's own `slug:` and
`date:` only shape a new post), so the URL never moves; `title`, `tags`, the
body and a new `lastmod` are refreshed; keys you added by hand and comments
stay. A post's tags are kept when the note has none to give, and when Bjorn
did not write the post, unless the note's own front matter names `tags:`. A live post (`draft: false`, or no `draft`) stays live whatever the note
says. If the post Bjorn wrote is gone, the dialog says so; before writing a new
file Bjorn looks for a post with the same slug in the section and offers to
update it; a second note whose slug is taken gets `-2`. A post whose front
matter Bjorn cannot read (TOML, JSON, broken YAML, keys twice, not UTF-8) is
refused rather than rewritten, and everything Bjorn writes is parsed again
before it is written.

**Images**: only png, jpg, jpeg, gif, webp and avif attachments are published.
Each linked one is renamed `<slug>-<name>` (web-safe), copied to
`media_dir/YYYY/MM/`, and its link points at `media_url/YYYY/MM/<file>`; the
toast lists what to upload. `media_dir` defaults to outside any site, since a
site repository may be public. With a bundle `path` (`.../{slug}/index.md`)
images go beside the post instead. A note is refused when an image link would
break: an image that is not one of its attachments, an svg or other type, or
images with neither `media_url` nor a bundle. A link to a non-image attachment
becomes its text.

**Kept off a public site**: the note's tags (tag lines, inline, in the title),
`bear://` links (reduced to their text), `[[wiki links]]` (reduced to their
text); a `file://` link refuses the publish. The same pass runs over the
title (before the slug is made from it) and every value in the note's own
front matter. Code blocks and inline code are
left exactly as written. Posts and images are written 0644, the ledger 0600,
and nothing is written through a symlink or outside `content/<section>`.

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

`a` opens the action menu — a search box over your actions, filtered as you
type, with the default marked ★ and the highlighted command shown in full;
`enter` runs. `!` skips the menu and runs the default straight away. The
menu's last row, **+ New action**, adds one, `ctrl+e` edits the highlighted one
and `ctrl+d` deletes it after asking; all three write the config for you,
comments and all. The note is rendered the way
export renders it, written to a temp file the command gets as
`$BJORN_NOTE_FILE` (and on stdin), with the title, id, tags and stamps in the
environment; the file goes away when the command ends. The first line the
command prints comes back as a toast, and a non-zero exit is reported with its
stderr. Full reference: [docs/actions.md](docs/actions.md).

## Development

```sh
cargo test                                 # unit, client, UI through a headless harness
cargo clippy --all-targets -- -D warnings
cargo run --release --bin bjorn-gate       # acceptance gate against the live library
cargo run --release --bin bjorn-gate -- --bench
cargo run --release --bin bjorn-gate -- --latency
cargo run --example shot -- --theme red-graphite --out /tmp/shot.json
uv run --with pillow --with fonttools python tools/shot.py /tmp/shot.json /tmp/shot.png
```

The screenshots above come from the same pipeline, drawn against the demo
library the fake bearcli seeds (`--height 34` for the main screen, `--screen
triage --height 22` for triage, then `tools/shot.py ... --scale 2`). A Nerd
Font in `~/Library/Fonts` is what draws the sidebar icons.

The plan and its status live in `docs/plans/bjorn-rust.md`; deferred work in
`docs/ROADMAP.md`.

## Acknowledgements

Bjorn exists because of [Shiny Frog](https://shinyfrog.net) and Bear, and
`bearcli` is what makes a terminal client possible at all. Thank you.
