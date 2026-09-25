# Actions

An action is a shell command Bjorn runs against the note under the cursor.
Export writes the note to a file you chose; an action hands that same file to a
command of yours and reports what it said — `aws s3 cp`, `scp`, `curl`, `gh
gist create`, `pbcopy`, a script of your own. What the command prints can also
go back into Bear: appended to the note, as a new note, or in place of the note
(see [Sending the output to Bear](#sending-the-output-to-bear)).

- `!` runs the default action.
- `a` opens the action menu: a search box over your actions, filtered as you
  type. A query matches a name by its letters in order, so `pts3` finds
  "Publish to S3", and a command by plain substring, so `curl` finds an action
  by what it runs. `↑`/`↓` (or `ctrl+p`/`ctrl+n`, or `tab`) picks, `enter` runs, `esc`
  closes. The default is marked ★, and under the list the highlighted action
  is shown in full: its command, format, timeout, where its output goes and
  whether it asks first. `ctrl+e`
  edits it and `ctrl+d` deletes it, after asking.

`!` opens the menu instead when there is no default to run; with no actions at
all, the menu holds just **+ New action**. Locked notes are refused, as they
are for export.

## Adding, editing and deleting from the menu

The last row of the menu, **+ New action**, opens a form: the name, the
command, the format to render, whether it asks first, whether it is the
default, where its output goes and, for an append, the section heading. Type in
the search box first and the name is filled in from it. `tab` or `↑`/`↓` moves
between fields, `←`/`→` changes the format or the output, `space` ticks a box,
`enter` saves and `esc` goes back to the menu.

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
timeout = 900
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
| `interactive` | `false` | give the command the window and the keyboard in a pty, instead of capturing its output. For anything that talks back |
| `timeout` | `300` | seconds (five minutes); a command that overruns is killed and reported. Not applied to an `interactive` action, which runs until you quit it |
| `output` | `toast` | where stdout goes: `toast` (its first line, as today), `append` (to the end of the note), `new-note`, or `replace` (the note's whole text; always asks first, and needs `format = "md"`). Any other value stops the action before it runs, so a typo neither writes nor runs. See [Sending the output to Bear](#sending-the-output-to-bear) |
| `section` | — | with `output = "append"`: the heading to add under, written as it is in the note (`"## Summary"`). Blank or absent appends to the end of the note. Set on any other `output`, it stops the action from running |
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

## Commands that talk back

An ordinary action is a one-shot: Bjorn runs it, keeps the first line it printed
and shows that as a toast. A command that wants to *ask* something — a session,
a repl, an installer, a tool with its own prompts — has nowhere to ask it.
`interactive = true` gives it the window instead:

```toml
[[actions]]
name = "Sync my notes"
command = 'notes-sync "$BJORN_ACTION_INPUT"'
prompt = "Range: today, week, last-week, or a date"
interactive = true
```

The command runs in a pseudo-terminal that fills the window, titled with the
action's name, and every key goes to it until it exits — the same machinery the
editor uses, which is why `$EDITOR` works the way it does. Then Bjorn comes
back, and a non-zero exit is reported as a toast, since there is no captured
output to report instead.

It gets the same note and environment a captured action gets: the rendered
note as `$BJORN_NOTE_FILE`, the temp directory as its working directory, and
the whole `BJORN_*` environment including `$BJORN_ACTION_INPUT`. Two things
differ:

- **stdin** belongs to the terminal rather than to the note, so a command that
  wants the text reads `"$BJORN_NOTE_FILE"`.
- **`timeout` does not apply.** The command is waiting on you, not stuck, so it
  runs until it exits or you quit it. If Bjorn itself exits first, the command
  is killed.

One interactive action runs at a time. Between pressing the key and the
command starting, while the note is rendered, `esc` calls it off and every other
key is ignored: it is not passed on to the command once it starts.

The editor fills the reader pane so the note list stays beside it; an
interactive action fills the window, because its program owns its own screen and
there is nothing useful to keep next to it.

A captured action — anything not `interactive` — runs in a process group of its
own, away from the terminal, so a command that tries to prompt there (an ssh
passphrase, a login prompt) gets no answer and waits until the timeout stops
it. Give those `interactive = true`.

## Sending the output to Bear

By default the command's output is a toast: its first line, and the rest is
dropped. `output` sends all of it back into Bear instead, through the same
`bearcli` every other write uses:

```toml
# Summarize the note and add the summary under its own heading.
[[actions]]
name = "Summarize"
command = 'claude -p "Summarize this note in five bullet points."'
output = "append"
section = "## Summary"

# A GitHub issue as a note: the prompt asks which one.
[[actions]]
name = "Issue to note"
command = '''gh issue view "$BJORN_ACTION_INPUT" --json title,body --jq '"# \(.title)\n\n\(.body)"' '''
prompt = "Issue number"
output = "new-note"

# The pull request on this branch, as a note.
[[actions]]
name = "PR to note"
command = '''cd ~/src/app && gh pr view --json title,url,body --jq '"# \(.title)\n\n\(.url)\n\n\(.body)"' '''
output = "new-note"

# Tidy the note in place.
[[actions]]
name = "Tidy"
command = 'llm -s "Fix spelling and grammar. Keep the Markdown and every link as it is."'
output = "replace"
```

- **`append`** adds the output to the end of the note, or with `section` to
  the end of that section: after anything nested under it, ahead of the blank
  lines before the next heading. Without a section, Bear puts it ahead of tags
  placed at the bottom of the note and of footnote definitions. A section the
  note does not have is an error, and nothing is written.
- **`new-note`** makes a note of the output. Bear takes the title from its
  first line, after any YAML front matter, so a command that prints `# Title`
  first gets that title. The note is tagged the way `n` tags one — the tag
  you are in, else the workspace — and selected once it is in the list.
- **`replace`** writes the output over the note's whole text. It always asks
  first, whether or not the action says `confirm = true`, and it is
  hash-guarded the way the editor is: if the note changed in Bear while the
  command ran, nothing is written. Bear derives the title and tags from the
  new text, so a command that rewrites a note should keep its `# Title` line
  and its tags. Bear also refuses a replacement that would drop an attachment
  the note has. It needs `format = "md"`, since the command's output is what
  the note becomes: an HTML or RTF rendering would come back as the note's
  text. Bear cannot undo it, so the text the note had is saved to a temp file
  first, and the toast says where.

The output goes to Bear as it was printed, without the blank lines around it,
with `\r\n` turned into `\n` and control characters other than tab and
newline (terminal color codes, bells) taken out. Some things never write:

- **A non-zero exit, a timeout or a failure to start.** The toast reports it,
  as for any action, and says nothing was written.
- **Empty output.** A command that printed nothing (or only blank lines) never
  makes an empty note or wipes one; a toast says it ran and wrote nothing.
- **More than 1 MB.** The output is refused rather than cut short; the first
  megabyte is kept in a temp file.
- **Output that is not UTF-8 text.** It is kept in a temp file as it came.
- **A note that went to the trash** while the command ran. Bjorn checks just
  before it writes, and keeps the output.

When Bear refuses a write — the section is missing, the note changed, an
attachment would go — the output is kept in a temp file and the toast says
where (`…/bjorn-output-XXXX/Summarize.md`), so an answer that took a minute
and cost money does not have to be asked for again. The folder is readable
only by you. Bjorn does not delete it, but it lives under `$TMPDIR`, which
macOS clears of files left untouched for a few days, so move anything you
want to keep.

An action that could not do what its entry asks does not run, and a toast
says why: an `interactive` action with an `output` (it has no captured output
to send), `replace` with a format other than `md`, a `section` on anything but
`append`, or an `output` value Bjorn does not know. The add and edit form shows
the same warning and will not save such an action.

An LLM command reads the note, and a note can hold text written to steer it
(pasted from a web page, say). Run such commands with their tools turned off,
using whatever option your CLI has for allowing no tools or shell access, so
the worst a steered answer can do is be wrong, which the backup and the
confirm dialog of `replace` let you catch. A tool-enabled agent that reads an
injected note can act on it, with your credentials.

## What the command gets

The note is rendered exactly as export renders it and written to a temp
directory, which is also the command's working directory. The file is named
after the note (`Sprint Planning.md`), and it is deleted as soon as the command
ends.

- **stdin** — the note's text in the chosen format. A TextBundle is a folder, so
  stdin gets its `text.md`. An `interactive` action is the exception: its stdin
  is the terminal, so it reads the note from `"$BJORN_NOTE_FILE"`.
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
first line the command printed (`Done.` when it printed nothing). With an
`output` that writes to Bear, the toast says what was written instead. Anything else
is an error toast with the exit code and the first line of stderr —
`exit 3: no credentials`.

Nothing blocks: the action runs on the tokio runtime like every bearcli call,
and the three columns stay live while it does. Unless its `output` wrote to
Bear, Bjorn never inspects a note after an action; if the command changed the
note in Bear itself, the next poll picks it up.

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
