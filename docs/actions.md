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

## Publish to Hugo

Bjorn has no Hugo code of its own. `contrib/hugo-publish` in this repository
is a script you run as an action: it takes the note Bjorn hands over and
writes it as a post into a [Hugo](https://gohugo.io) site on your disk, then
stops. Building, previewing, committing and pushing stay with you. It needs
`python3` (standard library only); macOS has it once the Command Line Tools
are installed (`xcode-select --install`). Copy the script somewhere on your
`PATH`, or point the action at it where it is.

Two actions, so the choice is made by which one you pick: a draft by
default, and going live only through an entry that asks first.

```toml
[[actions]]
name = "Hugo: save draft"
command = 'hugo-publish --site ~/sites/blog --tag-prefix blog/ --bundle'
format = "textbundle"

[[actions]]
name = "Hugo: publish live"
command = 'hugo-publish --site ~/sites/blog --tag-prefix blog/ --bundle --live'
format = "textbundle"
confirm = true
```

The toast says which happened: `Draft saved: content/posts/<slug>/index.md`
or `Published (live): content/posts/<slug>/index.md`, with `(updated)` when
the post was there already, and `(was live; now a draft)` when a draft run
takes a live post down.

If you would rather have one entry, `--ask` reads the answer to the action's
`prompt`: `yes` publishes live, anything else (including nothing) saves a
draft.

```toml
[[actions]]
name = "Hugo"
command = 'hugo-publish --site ~/sites/blog --tag-prefix blog/ --bundle --ask'
format = "textbundle"
prompt = "Publish live? Type yes; anything else saves a draft"
```

| Flag | |
|---|---|
| `--site DIR` | required: the site root, the folder with `content/` |
| `--section NAME` | the folder under `content/`; default `posts` |
| `--draft` / `--live` / `--ask` | `draft: true` (the default), `draft: false`, or ask through `prompt` |
| `--tag-prefix TAG` | publish only the note's tags under this one, prefix removed: with `blog/`, `#blog/rust` is `rust`. Without it no tags are published |
| `--bundle` | a new post is `<slug>/index.md` with its images beside it. A note with images needs it |
| `--allow-html` | publish raw HTML and Hugo shortcodes instead of refusing the note |

What a post gets: `title` (the note's `# ` title), `slug` (the title in
lower-case ASCII, accents folded, other runs a hyphen), `date` (now), `draft`,
`tags`, and `bjorn_note`, a hash of the note's id that marks the post as this
note's. A note can start with its own front matter between `---` fences, and
`title`, `slug`, `date`, `description`, `summary`, `showtoc`, `tags` (a list)
and `cover` (`image`, `alt`, `caption`, `relative`, `hidden`) are taken from
it; every other key, and the note's own `draft:`, is left out and listed after
the toast's line.

**Publishing again** finds the post by its mark, wherever it is in the
section, even after the note's title changed, and rewrites it from the note:
it keeps its `date`, `slug` and place, gains a `lastmod`, and keeps keys you
added to the post by hand. Its `description`, `summary`, `showtoc`, `cover`
and `tags` stay as you set them when the note gives none. The note is the
source for everything else, the body included.

**What it guards:**

- **Never another file.** A file at the post's path without this note's mark
  (a post you wrote by hand, or one written for another note) is refused, not
  replaced; so is a `<slug>.md` beside a `<slug>/` bundle, which Hugo treats
  as the same page. An image already in the bundle is never overwritten: the
  same bytes are reused, different ones get a new name (`photo-2.png`), and
  the output says so.
- **Only inside `content/<section>`.** The slug is `[a-z0-9-]` and cannot
  climb out; every folder under the site is opened without following
  symlinks, and a symlink anywhere on the way is refused. Files are created
  with `O_EXCL|O_NOFOLLOW` under a temporary name and moved into place, so a
  post is written whole or not at all; a refusal writes nothing, and a
  failure part way removes what that run added.
- **Front matter it writes itself.** Keys come from the fixed list above and
  every string is JSON-quoted (a valid YAML double-quoted scalar), so a value
  holding a newline, a colon or `url: /x/` stays one line of text. The
  note's own front matter is read by a small `key: value` reader, not a YAML
  library: anchors, aliases, tags and block scalars are never read (the key is
  dropped), and front matter over 64 KB is refused.
- **Private things stay home.** Tags: the tag lines, the note's tags inline in
  the text, and every tag not under `--tag-prefix`. Wiki links become their
  text (`[[Plan [v2]]]` is `Plan [v2]`, `[[Note|shown]]` is `shown`). Links
  to `bear://`, `file://`, other apps (`things:`, `obsidian:`, anything but
  `http`, `https` and `mailto`), `~/...` and paths on this Mac (`/Users/...`,
  `/Volumes/...`) keep their text and lose the link. A note that still
  mentions a `/Users/...` path, or has one or a `bear://` link in code (code is
  published as written), is refused.
- **Nothing that runs on the site.** Hugo shortcodes (`{{<`, `{{%`, even in
  code blocks, where Hugo still runs them) and raw HTML are refused unless
  `--allow-html` is given. `<!--more-->` is fine.
- **Images that would break.** Only png, jpg, jpeg, gif, webp and avif
  attachments are published, renamed web-safe beside the post. An image that
  is not one of the note's attachments, an svg, an inline `data:` image, or
  images without `--bundle` are refused. With `format = "md"` the action gets
  no attachments at all, so use `format = "textbundle"` for a note with
  images.

A refusal exits non-zero with one line on stderr, which Bjorn shows as the
error toast (`exit 1: content/posts/hello.md exists and was not written by
this script; refusing to replace it.`). To see it work outside Bjorn:

```sh
BJORN_NOTE_FILE=note.md BJORN_NOTE_ID=test hugo-publish --site ~/sites/blog
```

Previewing is another action. `hugo server` runs until stopped, so start it
in the background with its output going to a file, or the action waits for
it and is killed at the timeout:

```toml
[[actions]]
name = "Preview the site"
command = '''
cd ~/sites/blog || exit 1
log="${TMPDIR:-$HOME}/hugo-preview.log"
pgrep -qf "hugo server" || nohup hugo server -D </dev/null >"$log" 2>&1 &
sleep 2 && open http://localhost:1313/ && echo "http://localhost:1313/ (log: $log)"
'''
```
