# Actions

An action is a shell command Bjorn runs against the note under the cursor.
Export writes the note to a file you chose; an action hands that same file to a
command of yours and reports what it said — `aws s3 cp`, `scp`, `curl`, `gh
gist create`, `pbcopy`, a script of your own.

- `!` runs the default action.
- `a` opens the action menu: a search box over your actions, filtered as you
  type. A query matches a name by its letters in order, so `pts3` finds
  "Publish to S3", and a command by plain substring, so `curl` finds an action
  by what it runs. `↑`/`↓` (or `ctrl+p`/`ctrl+n`, or `tab`) picks, `enter` runs, `esc`
  closes. The default is marked ★, and under the list the highlighted action
  is shown in full: its command, format, timeout and whether it asks first. `ctrl+e`
  edits it and `ctrl+d` deletes it, after asking.

`!` opens the menu instead when there is no default to run; with no actions at
all, the menu holds just **+ New action**. Locked notes are refused, as they
are for export.

## Adding, editing and deleting from the menu

The last row of the menu, **+ New action**, opens a form: the name, the
command, the format to render, whether it asks first, and whether it is the
default. Type in the search box first and the name is filled in from it. `tab`
or `↑`/`↓` moves between fields, `←`/`→` changes the format, `space` ticks a
box, `enter` saves and `esc` goes back to the menu.

Saving adds an `[[actions]]` entry to the end of the config file Bjorn read at
start-up (or the one `--config` named). The file is edited as text, not
rewritten, so your comments and layout stay as they were. Only one action is
the default: if the new one is, the old entry's `default = true` becomes
`default = false`, and the form says which one that is before you save.
Nothing is written if the file does not parse, and the new action is in the
menu straight away.

`ctrl+e` on the highlighted action opens the same form, filled in. Saving an
edit rewrites only the lines whose value changed: a comment on a line you left
alone, and keys the form does not show such as `timeout`, stay as they were.
The result is read back before it is written, and if the entry does not come
out exactly as edited (because the file changed since Bjorn read it, say),
nothing is written.

`ctrl+d` on the highlighted action asks first, the way quitting does: `y` or
`enter` deletes it, `n` or `esc` goes back to the menu. Deleting removes the
entry's lines from its `[[actions]]` header down to its last key. Comments and
blank lines after that key stay, so a block of commented-out templates below
your last action survives. As with an edit, the file is read back first and
every other action must come out unchanged, or nothing is written.

## Configuring

Actions live in the shared config file,
`${XDG_CONFIG_HOME:-~/.config}/bjorn/config.toml`, as an array of tables:

```toml
[[actions]]
name = "Publish to S3"
command = 'aws s3 cp "$BJORN_NOTE_FILE" "s3://notes/$BJORN_NOTE_TITLE.html"'
format = "html"
confirm = true
timeout = 300
default = true

[[actions]]
name = "Copy as plain text"
command = "pbcopy"
format = "txt"

[[actions]]
name = "Gist it"
command = 'gh gist create "$BJORN_NOTE_FILE" --desc "$BJORN_NOTE_TITLE"'

[[actions]]
name = "Post to the API"
command = 'curl -sf -X POST https://example.test/notes -H "Content-Type: text/markdown" --data-binary @-'
```

| Key | Default | |
|---|---|---|
| `name` | the command | what the palette shows and the toast reports |
| `command` | — | required; run through `sh -c`, so pipes, `&&` and redirection all work |
| `format` | `md` | how the note is rendered first: `md`, `html`, `txt`, `rtf`, `textbundle`. An unknown name falls back to `md` |
| `confirm` | `false` | ask before running. Worth setting on anything that publishes or deletes |
| `timeout` | `60` | seconds; a command that overruns is killed and reported |
| `default` | `false` | the action `!` runs. With exactly one action configured, that one is the default whether or not it says so |

An entry without a `command` is skipped rather than raised, so a half-written
action never stops the app.

## What the command gets

The note is rendered exactly as export renders it and written to a temp
directory, which is also the command's working directory. The file is named
after the note (`Sprint Planning.md`), and it is deleted as soon as the command
ends.

- **stdin** — the note's text in the chosen format. A TextBundle is a folder, so
  stdin gets its `text.md`.
- `BJORN_NOTE_FILE` — the full path to that file. Quote it; titles have spaces.
- `BJORN_NOTE_TITLE`, `BJORN_NOTE_ID`, `BJORN_NOTE_TAGS` (comma-separated),
  `BJORN_NOTE_CREATED`, `BJORN_NOTE_MODIFIED` (RFC 3339), `BJORN_NOTE_PINNED`
  (`0`/`1`), `BJORN_NOTE_FORMAT`, `BJORN_ACTION`.

The command inherits Bjorn's own environment too, so `PATH`, `AWS_PROFILE` and
anything else your shell exported when you launched Bjorn is there. It does not
read your `~/.zshrc`: it is `sh -c`, not a login shell. If an action needs a
shell function or an alias, put it in a script and call the script.

## A PDF

`bearcli` does not export, and Bjorn has no PDF writer: nothing in it draws a
page, and a converter that did would be the first thing it could not do on its
own. The HTML export is the way there. It carries a print stylesheet — A4 (what
Bear's own PDF export uses), a white page whatever the theme's background is,
headings kept with the text under them, and the theme's colors on the links
and the list markers, the way Bear puts its own on its PDF — so an HTML-to-PDF
converter is all an action needs. Neither of these ships with macOS.

```toml
[[actions]]
name = "Save as PDF to Desktop"
command = 'out="$HOME/Desktop/$(basename "$BJORN_NOTE_FILE" .html).pdf"; weasyprint "$BJORN_NOTE_FILE" "$out" 2>/dev/null && echo "saved $out"'
format = "html"
timeout = 120
```

```toml
[[actions]]
name = "Save as PDF to Desktop (Chrome)"
command = 'out="$HOME/Desktop/$(basename "$BJORN_NOTE_FILE" .html).pdf"; profile="$(mktemp -d)"; cp "$BJORN_NOTE_FILE" note.html && "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --user-data-dir="$profile" --host-resolver-rules="MAP * ~NOTFOUND" --proxy-server="127.0.0.1:1" --proxy-bypass-list="<-loopback>" --disable-remote-fonts --no-pdf-header-footer --print-to-pdf="$out" "file://$PWD/note.html" >/dev/null 2>&1; rm -rf "$profile"; [ -s "$out" ] && echo "saved $out"'
format = "html"
timeout = 120
```

`weasyprint` (`pipx install weasyprint`, or `uv tool install weasyprint`) reads
the page directly; headless Chrome is already on most Macs and renders the CSS
the way the browser you preview in does. Attachments are embedded in the HTML
as `data:` URIs, so the PDF carries the note's images either way.

What comes across: Bear's colored highlights (it writes the color as a
circle at the front of the run, which becomes the highlight's color rather
than a character in it), its `> [!NOTE]` callouts as panels, note links
(`[[Another note]]`) as the target in the accent color, ticked tasks grayed
out, tables, code, quotes with the accent bar, and images, which are embedded
in the HTML as `data:` URIs. Two things do not: `$math$`, which needs a math
engine Bjorn does not carry, and the icon Bear draws inside a callout. And
WeasyPrint paints form controls itself, so a ticked task's box is its own
square there, where Chrome draws Bear's rounded one.

Three details in those commands are load-bearing:

- **The name comes from the file, not the title.** `$BJORN_NOTE_TITLE` is the
  title verbatim — a note called `Q1/Q2 plan` would send the PDF to a directory
  that does not exist. The temp file's name is already sanitized, so
  `basename "$BJORN_NOTE_FILE" .html` is the safe stem.
- **Chrome gets a copy at a plain path.** `#` and `%` survive the filename
  sanitizing and mean something else inside a URL, so `file://$BJORN_NOTE_FILE`
  can quietly print the wrong page — and Chrome still exits 0. The temp
  directory is the command's working directory; copying to `note.html` first
  sidesteps it.
- **Chrome gets its own throwaway profile, and nowhere to go.** A note can
  contain inline HTML, and a converter renders it. `--user-data-dir` keeps the
  render away from your logged-in profile and its cookies (and stops the action
  failing when Chrome is already open), and it is deleted afterwards.
  `--host-resolver-rules` stops any hostname resolving; `--proxy-server` points
  what is left at a dead port, because a URL written as a bare IP address never
  goes near the resolver. Between them a note that carries a tracking pixel or
  a script has nowhere to send anything.

WeasyPrint runs no scripts, but it fetches what the page points at, and there
is no flag to stop it: a remote URL is fetched, and so is a local file — a note
saying `<img src="/Users/you/…">` bakes that file into the PDF you then send
on. Print a note you did not write yourself with Chrome.

## What comes back

Exit status 0 is success: a toast titled with the action's name, carrying the
first line the command printed (`Done.` when it printed nothing). Anything else
is an error toast with the exit code and the first line of stderr —
`exit 3: no credentials`.

Nothing blocks: the action runs on the tokio runtime like every bearcli call,
and the three columns stay live while it does. Bjorn never inspects a note
after an action; if the command changed the note in Bear, the next poll picks
it up.

## Safety

An action is a shell command you wrote, run with your credentials, on demand.
Bjorn does not sandbox it and does not parse it. Two habits are worth keeping:
set `confirm = true` on anything that publishes, deletes or costs money, and
quote `"$BJORN_NOTE_FILE"` and `"$BJORN_NOTE_TITLE"` — note titles carry
spaces, quotes and slashes, and only the filename is sanitized.
