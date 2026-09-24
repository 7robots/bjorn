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
| `prompt` | — | ask for one line of text first and pass it as `$BJORN_ACTION_INPUT`. The value is the prompt's title (`prompt = "Which bucket"`). A blank one is no prompt at all |
| `timeout` | `60` | seconds; a command that overruns is killed and reported |
| `default` | `false` | the action `!` runs. With exactly one action configured, that one is the default whether or not it says so |

An entry without a `command` is skipped rather than raised, so a half-written
action never stops the app.

## Asking for something first

An action with a `prompt` asks for one line of text before it runs, and hands
it to the command as `$BJORN_ACTION_INPUT`. It is one action instead of five
when the only difference between them is an argument:

```toml
[[actions]]
name = "Sync my notes"
command = 'notes-sync "$BJORN_ACTION_INPUT"'
prompt = "Range: today, week, last-week, 2w, or a date"
```

The menu's detail line says `asks: <title>` for an action that has one. The
add and edit form does not show `prompt`, so set it by hand in the config;
editing an action through the form keeps the prompt it already has, the way it
keeps a `timeout`.

`enter` runs it, `esc` cancels. An empty answer still runs: a command that has
a sensible default ("today", here) can treat "nothing typed" as asking for it,
so the quick path stays two keys. With `confirm = true` as well the prompt comes
first and the dialog quotes the answer — *Run "Sync my notes" on "last-week"?* —
so a fat-fingered range is caught before the command sees it.

## What the command gets

The note is rendered exactly as export renders it and written to a temp
directory, which is also the command's working directory. The file is named
after the note (`Sprint Planning.md`), and it is deleted as soon as the command
ends.

- **stdin** — the note's text in the chosen format. A TextBundle is a folder, so
  stdin gets its `text.md`.
- `BJORN_NOTE_FILE` — the full path to that file. Quote it; titles have spaces.
- `BJORN_ACTION_INPUT` — what `prompt` collected; empty when the action has no
  prompt, and empty when the prompt was answered with nothing.
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
Bjorn does not sandbox it and does not parse it. Three habits are worth keeping:
set `confirm = true` on anything that publishes, deletes or costs money; quote
`"$BJORN_NOTE_FILE"` and `"$BJORN_NOTE_TITLE"` — note titles carry spaces,
quotes and slashes, and only the filename is sanitized; and quote
`"$BJORN_ACTION_INPUT"` as well.

The variables reach the command through its environment, never spliced into the
command text, so a `;` or a `$(…)` inside one is inert. Two things undo that.
Leaving a variable unquoted lets the shell split it into words, so a typed
`--force` arrives as an *option* rather than as text and a `*` is expanded
against the working directory. And handing one to something that evaluates its
input — `eval`, `sh -c "$VAR"`, an arithmetic context like `$(( VAR ))` or
`[[ $VAR -eq 1 ]]` — turns the text back into code. An action that cannot avoid
that should carry `confirm = true`, which shows the answer back to you, quoted,
before the command sees it.

## Hugo recipes

`P` writes a post into a Hugo site and stops there: Bjorn makes no network
calls, so uploading images, previewing and committing are yours. These are
examples to adapt, not built-ins; they assume `[hugo] site = "~/sites/blog"`
and the default `media_dir`. See the README's
[Publishing to Hugo](../README.md#publishing-to-hugo) for the `[hugo]` keys.

Upload what `P` staged, with a site script that takes a key prefix (this one
is shaped like an R2 upload script: `-p` sets the prefix, it wants absolute
paths because it changes directory, and for each file it prints the public
URL on success, `EXISTS ...` when the key is already there, or `FAILED`/`SKIP`,
exiting 0 either way). Staged files sit in `media_dir/YYYY/MM/`, the tail of
the URL `P` linked, so the prefix is the folder. A file that uploaded, or was
already there, is moved out of staging, so running it again only retries
what failed. Names are `<slug>-<name>`, so `EXISTS` means this post's image
went up before; if you changed the image, upload it again with `-f` by hand:

```toml
[[actions]]
name = "Upload staged images"
command = '''
staged=~/Downloads/bjorn-media; done_dir=~/Downloads/bjorn-media-uploaded
cd "$staged" 2>/dev/null || { echo "Nothing staged."; exit 0; }
ok=0; left=0
for file in "$staged"/*/*/*; do
  [ -f "$file" ] || continue
  month=${file#"$staged"/}; month=${month%/*}
  result=$(~/sites/blog/r2-upload.sh -p "blog/$month" "$file" 2>&1) || result="FAILED"
  case "$result" in
    https://*|EXISTS*) mkdir -p "$done_dir/$month" && mv "$file" "$done_dir/$month/" && ok=$((ok + 1)) ;;
    *) left=$((left + 1)) ;;
  esac
done
echo "Uploaded $ok; $left left in staging."
'''
confirm = true
timeout = 300
```

Preview with drafts. `hugo server` runs until stopped, so it is started in
the background with its output going to a file; otherwise the action would
wait for it and be killed at the timeout:

```toml
[[actions]]
name = "Preview the site"
command = '''
cd ~/sites/blog || exit 1
pgrep -qf "hugo server" || nohup hugo server -D </dev/null >/tmp/hugo-preview.log 2>&1 &
sleep 2 && open http://localhost:1313/ && echo "http://localhost:1313/ (log: /tmp/hugo-preview.log)"
'''
```

Commit just this note's post. The ledger `P` keeps beside the config file
maps the note's id to the file it wrote, and `plutil` (part of macOS) reads
it; only that file (or a bundle's folder) is staged and committed. Pushing is
left out on purpose, since a public site repository publishes whatever it is
given:

```toml
[[actions]]
name = "Commit the post"
command = '''
post=$(plutil -extract "posts.$BJORN_NOTE_ID.path" raw -o - ~/.config/bjorn/hugo-published.json) || { echo "Not published yet."; exit 1; }
[ "${post##*/}" = index.md ] && post=${post%/index.md}
cd ~/sites/blog && git add -- "$post" && git commit -q -m "Post: $BJORN_NOTE_TITLE" -- "$post" && git log -1 --format="%h %s"
'''
confirm = true
```
