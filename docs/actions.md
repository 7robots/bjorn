# Actions

An action is a shell command Bjorn runs against the note under the cursor.
Export writes the note to a file you chose; an action hands that same file to a
command of yours and reports what it said — `aws s3 cp`, `scp`, `curl`, `gh
gist create`, `pbcopy`, a script of your own.

- `!` runs the default action.
- `a` opens the action menu: a search box over your actions, filtered as you
  type. `↑`/`↓` (or `ctrl+p`/`ctrl+n`, or `tab`) picks, `enter` runs, `esc`
  closes. The default is marked ★, and under the list the highlighted action
  is shown in full: its command, format, timeout and whether it asks first.

`!` opens the menu instead when there is no default to run; with no actions at
all, the menu holds just **+ New action**. Locked notes are refused, as they
are for export.

## Adding one from the menu

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
