//! Actions: user commands run against the note under the cursor.
//!
//! An action is a shell command from the config file. Bjorn renders the note
//! the same way the exporter does, writes it to a temp file, then runs the
//! command with that file as `$BJORN_NOTE_FILE`, the note's text on stdin and
//! the note's metadata in the environment. What the command does with it —
//! `aws s3 cp`, `scp`, `curl`, `gh gist create` — is the user's business.
//! Bjorn reports the exit status and the first line of output, or, with
//! `output`, hands everything the command printed back to Bear: appended, as a
//! new note, or in place of the note. The temp file goes away when the command
//! ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use tokio::io::AsyncWriteExt;

use crate::bear::Note;
use crate::export::{export_note, extension_for, format_by_id, safe_filename};

/// A command that does not finish in this long is killed. Uploads are slower
/// than bearcli calls, so this is longer than `bear::COMMAND_TIMEOUT`.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
/// Characters of a command's output kept for the toast.
pub const OUTPUT_LIMIT: usize = 300;
/// Bytes of output an action may write into Bear. More than this is refused
/// rather than cut, since half a note is worse than none; the first
/// `WRITE_LIMIT` bytes are kept in a temp file instead (`keep_output`).
pub const WRITE_LIMIT: usize = 1024 * 1024;
/// Bytes of stderr kept; only its first line is ever shown.
const STDERR_LIMIT: usize = 64 * 1024;

/// Where an action's output goes once it exits 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionOutput {
    /// The first line, as a toast. Nothing is written to Bear.
    #[default]
    Toast,
    /// Added to the end of the note, or of one section of it.
    Append,
    /// A note of its own; the first line (or `# ` heading) is its title.
    NewNote,
    /// The note's whole text, hash-guarded against a change made meanwhile.
    Replace,
}

impl ActionOutput {
    pub const ALL: [ActionOutput; 4] = [
        ActionOutput::Toast,
        ActionOutput::Append,
        ActionOutput::NewNote,
        ActionOutput::Replace,
    ];

    /// The config value.
    pub fn id(self) -> &'static str {
        match self {
            ActionOutput::Toast => "toast",
            ActionOutput::Append => "append",
            ActionOutput::NewNote => "new-note",
            ActionOutput::Replace => "replace",
        }
    }

    /// A config value, case and `_`/`-` aside. `None` for anything else.
    pub fn parse(value: &str) -> Option<ActionOutput> {
        let wanted = value.trim().to_lowercase().replace('_', "-");
        Self::ALL.into_iter().find(|o| o.id() == wanted)
    }

    /// Whether the output is written into Bear.
    pub fn writes(self) -> bool {
        self != ActionOutput::Toast
    }

    /// The write into Bear this output asks for; `None` for a toast.
    pub fn write(self) -> Option<BearWrite> {
        match self {
            ActionOutput::Toast => None,
            ActionOutput::Append => Some(BearWrite::Append),
            ActionOutput::NewNote => Some(BearWrite::NewNote),
            ActionOutput::Replace => Some(BearWrite::Replace),
        }
    }

    /// Every config value, for an error that lists them.
    pub fn valid_values() -> String {
        Self::ALL.map(|o| o.id()).join(", ")
    }
}

/// The outputs that write into Bear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BearWrite {
    Append,
    NewNote,
    Replace,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ActionError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    /// Run through `sh -c`, so pipes and redirections work.
    pub command: String,
    /// The export format the note is rendered to first; an id from
    /// `export::FORMATS`.
    pub format: String,
    /// A `format` value that is not one of `export::FORMATS`, as written.
    /// `format` is then Markdown, and the action refuses to run, so a typo
    /// (`"pfd"`) does not hand a PDF command a Markdown file.
    pub format_error: Option<String>,
    /// Ask before running. For anything that publishes or deletes.
    pub confirm: bool,
    /// Ask for one line of text first, shown as the prompt's title, and pass it
    /// to the command as `$BJORN_ACTION_INPUT`. `None` runs straight away.
    pub prompt: Option<String>,
    /// Give the command the window and the keyboard, in a pty, instead of
    /// capturing its output. For anything that talks back: a session, a repl,
    /// a tool that asks its own questions.
    pub interactive: bool,
    /// What happens to stdout. Anything but `Toast` writes to Bear, and is
    /// refused for an `interactive` action, which has no captured output.
    pub output: ActionOutput,
    /// For `output = "append"`: the heading to append under (`## Summary`).
    /// `None` appends to the end of the note.
    pub section: Option<String>,
    /// An `output` value that is not one of `ActionOutput::ALL`, as written.
    /// `output` is then `Toast`, and the action refuses to run at all, so a
    /// typo neither writes nor spends a paid call on output that goes nowhere.
    pub output_error: Option<String>,
    pub timeout: Duration,
    /// Run by `!` without opening the palette.
    pub default: bool,
}

impl Default for Action {
    fn default() -> Self {
        Action {
            name: String::new(),
            command: String::new(),
            format: crate::export::DEFAULT_FORMAT.to_string(),
            format_error: None,
            confirm: false,
            prompt: None,
            interactive: false,
            output: ActionOutput::Toast,
            section: None,
            output_error: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            default: false,
        }
    }
}

impl Action {
    /// Case-insensitive: a subsequence of the name, so `pts3` finds
    /// "Publish to S3", or a substring of the command, so `curl` finds an
    /// action by what it runs.
    ///
    /// The command is matched as a substring and not as a subsequence on
    /// purpose. Commands are long, and scattered letters match almost
    /// anything inside one: `pub` matched a command whose only p, u and b
    /// were in `printf` and a temp path, so the box stopped filtering.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        subsequence(&self.name, query) || contains_ignore_case(&self.command, query)
    }

    /// Asks before running: `confirm = true`, or an output that replaces the
    /// note, which always asks.
    pub fn asks_first(&self) -> bool {
        self.confirm || self.output == ActionOutput::Replace
    }

    /// Why this action cannot run as configured, if it cannot. Short enough
    /// for the form's warning line; the caller names the action.
    pub fn misconfigured(&self) -> Option<String> {
        if let Some(value) = &self.format_error {
            return Some(format!(
                "format = {value} is not one of {}",
                crate::export::FORMATS.map(|f| f.id).join(", ")
            ));
        }
        if let Some(value) = &self.output_error {
            return Some(format!(
                "output = {value} is not one of {}",
                ActionOutput::valid_values()
            ));
        }
        if self.interactive && self.output.writes() {
            return Some(format!(
                "an interactive action has no output to {}; remove interactive or output",
                match self.output {
                    ActionOutput::Append => "append",
                    ActionOutput::NewNote => "make a note of",
                    _ => "write back",
                }
            ));
        }
        if self.output == ActionOutput::Replace && self.format != "md" {
            return Some(format!(
                "output = \"replace\" needs format = \"md\", or the note comes back as {}",
                format_by_id(&self.format).label
            ));
        }
        if self.section.is_some() && self.output != ActionOutput::Append {
            return Some("section is only used with output = \"append\"".into());
        }
        None
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .flat_map(char::to_lowercase)
        .all(|want| chars.any(|have| have == want))
}

/// The actions matching `query`, in config order.
pub fn filter<'a>(actions: &'a [Action], query: &str) -> Vec<&'a Action> {
    actions.iter().filter(|a| a.matches(query)).collect()
}

/// The action `!` runs: the one marked `default`, or the only one there is.
pub fn default_action(actions: &[Action]) -> Option<&Action> {
    actions
        .iter()
        .find(|a| a.default)
        .or(if actions.len() == 1 {
            actions.first()
        } else {
            None
        })
}

/// The environment an action's command sees, on top of Bjorn's own. `input` is
/// what the action's `prompt` collected, and is empty when it has none.
pub fn environment(
    action: &Action,
    note: &Note,
    file: &std::path::Path,
    input: &str,
) -> Vec<(String, String)> {
    let stamp =
        |t: Option<chrono::DateTime<chrono::Utc>>| t.map(|t| t.to_rfc3339()).unwrap_or_default();
    vec![
        ("BJORN_ACTION".to_string(), action.name.clone()),
        ("BJORN_ACTION_INPUT".to_string(), input.to_string()),
        ("BJORN_NOTE_ID".to_string(), note.id.clone()),
        ("BJORN_NOTE_TITLE".to_string(), note.title.clone()),
        ("BJORN_NOTE_FILE".to_string(), file.display().to_string()),
        ("BJORN_NOTE_FORMAT".to_string(), action.format.clone()),
        ("BJORN_NOTE_TAGS".to_string(), note.tags.join(",")),
        ("BJORN_NOTE_CREATED".to_string(), stamp(note.created)),
        ("BJORN_NOTE_MODIFIED".to_string(), stamp(note.modified)),
        (
            "BJORN_NOTE_PINNED".to_string(),
            if note.pins.is_empty() { "0" } else { "1" }.to_string(),
        ),
    ]
}

/// The first non-empty line of the command's output, for the toast: stdout if
/// it said anything, else stderr.
pub fn summarize(stdout: &str, stderr: &str) -> String {
    let pick = |text: &str| {
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .to_string()
    };
    let line = match pick(stdout) {
        empty if empty.is_empty() => pick(stderr),
        line => line,
    };
    if line.chars().count() > OUTPUT_LIMIT {
        format!("{}…", line.chars().take(OUTPUT_LIMIT).collect::<String>())
    } else {
        line
    }
}

/// The rendered note on disk, plus everything its command needs. Both the
/// captured path (`run`) and the interactive one start here, so an action sees
/// the same file, the same environment and the same working directory either way.
#[derive(Debug)]
pub struct Payload {
    /// Held for as long as the command may read it; dropping it removes the file.
    pub dir: tempfile::TempDir,
    pub file: PathBuf,
    pub env: Vec<(String, String)>,
    /// The note's text, for the commands that take it on stdin.
    pub stdin: Vec<u8>,
}

/// Render `note` the way export renders it and describe the command's world.
/// `images` is only read by the formats that want attachments.
pub async fn prepare(
    action: &Action,
    note: &Note,
    content: &str,
    images: &HashMap<String, Vec<u8>>,
    input: &str,
) -> Result<Payload, ActionError> {
    let fmt = format_by_id(&action.format);
    let dir = tempfile::Builder::new()
        .prefix("bjorn-action-")
        .tempdir()
        .map_err(|e| ActionError(e.to_string()))?;
    let file = dir.path().join(format!(
        "{}.{}",
        safe_filename(&note.title),
        extension_for(fmt, !images.is_empty())
    ));
    let (content, title, images) = (content.to_string(), note.title.clone(), images.clone());
    let target = file.clone();
    let file: PathBuf = tokio::task::spawn_blocking(move || {
        export_note(fmt, &content, &title, &target, &images).map_err(|e| ActionError(e.0))
    })
    .await
    .map_err(|e| ActionError(e.to_string()))??;

    // The text goes to stdin as well, so `curl --data-binary @-` works without
    // touching the file. A TextBundle is a folder; its markdown is inside.
    let stdin = if file.is_dir() {
        std::fs::read(file.join("text.md")).unwrap_or_default()
    } else {
        std::fs::read(&file).unwrap_or_default()
    };
    let env = environment(action, note, &file, input);
    Ok(Payload {
        dir,
        file,
        env,
        stdin,
    })
}

/// What a captured command printed, once it exited 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captured {
    /// Stdout as it came, for writing to Bear; checked, never converted.
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// Stdout ran past `WRITE_LIMIT`; `stdout` holds only the first part.
    pub truncated: bool,
}

impl Captured {
    /// The first line said, for the toast. Lossy is fine here.
    pub fn summary(&self) -> String {
        summarize(&String::from_utf8_lossy(&self.stdout), &self.stderr)
    }
}

/// Read up to `limit` bytes from `pipe` and discard the rest, so a command
/// that prints without end neither fills memory nor blocks on a full pipe.
async fn read_capped<R>(pipe: Option<R>, limit: usize) -> (Vec<u8>, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let Some(mut pipe) = pipe else {
        return (Vec::new(), false);
    };
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    let mut over = false;
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = limit.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                over |= n > room;
            }
        }
    }
    (kept, over)
}

/// Kill the command and everything it started. It runs as the leader of its
/// own process group, so `cat | claude -p` loses both halves, not just `sh`.
fn kill_group(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: killpg takes plain integers; the group is the child's own,
        // made by `process_group(0)` below, so nothing else is in it.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Render the note, run the command and collect what it printed. A non-zero
/// exit, a timeout or a failure to start is the error, carrying the exit code
/// and the first line of stderr.
///
/// The timeout covers everything the command is waited on for: feeding its
/// stdin, reading its output and its exit. A command that never reads a note
/// bigger than the pipe buffer, or that leaves a child holding stdout open,
/// is still stopped on time.
pub async fn execute(
    action: &Action,
    note: &Note,
    content: &str,
    images: &HashMap<String, Vec<u8>>,
    input: &str,
) -> Result<Captured, ActionError> {
    let Payload {
        dir,
        file: _,
        env,
        stdin: stdin_body,
    } = prepare(action, note, content, images, input).await?;

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&action.command)
        .current_dir(dir.path())
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ActionError(format!("could not run the command: {e}")))?;
    let (stdin_pipe, stdout_pipe, stderr_pipe) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let feed = async move {
        if let Some(mut handle) = stdin_pipe {
            let _ = handle.write_all(&stdin_body).await;
            let _ = handle.shutdown().await;
        }
    };
    let waited = tokio::time::timeout(action.timeout, async {
        let (_, out, err, status) = tokio::join!(
            feed,
            read_capped(stdout_pipe, WRITE_LIMIT),
            read_capped(stderr_pipe, STDERR_LIMIT),
            child.wait()
        );
        (out, err, status)
    })
    .await;
    let ((stdout, truncated), (stderr, _), status) = match waited {
        Ok((out, err, Ok(status))) => (out, err, status),
        Ok((_, _, Err(e))) => {
            kill_group(&mut child);
            return Err(ActionError(format!("the command failed to run: {e}")));
        }
        Err(_) => {
            kill_group(&mut child);
            let _ = child.wait().await;
            return Err(ActionError(format!(
                "“{}” did not finish within {} s and was stopped.",
                action.name,
                action.timeout.as_secs()
            )));
        }
    };
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    // The temp file lives exactly as long as the command does.
    drop(dir);
    if status.success() {
        Ok(Captured {
            stdout,
            stderr,
            truncated,
        })
    } else {
        let code = status
            .code()
            .map(|c| format!("exit {c}"))
            .unwrap_or_else(|| "killed by a signal".to_string());
        let detail = summarize(&stderr, &String::from_utf8_lossy(&stdout));
        Err(ActionError(if detail.is_empty() {
            code
        } else {
            format!("{code}: {detail}")
        }))
    }
}

/// Why output was not written to Bear, and whether it is worth keeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: String,
    /// The raw output should go to a temp file: there is something in it.
    pub keep: bool,
}

/// The text an action with a writing `output` hands to Bear: stdout without
/// the blank lines around it, with `\r\n` as `\n` and control characters
/// other than tab and newline taken out, ending on one newline.
///
/// Refused when there is nothing to write, so an empty run never makes an
/// empty note or wipes one; when it ran past `WRITE_LIMIT`, since a cut-off
/// note is worse than none; and when it is not UTF-8, which Bear would store
/// mangled. The last two are worth keeping.
pub fn text_for_bear(captured: &Captured) -> Result<String, Refusal> {
    if captured.truncated {
        return Err(Refusal {
            reason: format!(
                "it printed more than {} MB, the most an action may write",
                WRITE_LIMIT / (1024 * 1024)
            ),
            keep: true,
        });
    }
    let Ok(raw) = std::str::from_utf8(&captured.stdout) else {
        return Err(Refusal {
            reason: "what it printed is not UTF-8 text".into(),
            keep: true,
        });
    };
    let cleaned: String = raw
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect();
    let text = cleaned.trim_end();
    // Whole blank lines only: leading spaces may be indentation that matters.
    let text = match text.find(|c: char| c != '\n' && c != ' ' && c != '\t') {
        Some(first) => {
            let line_start = text[..first].rfind('\n').map_or(0, |i| i + 1);
            &text[line_start..]
        }
        None => "",
    };
    if text.is_empty() {
        return Err(Refusal {
            reason: "it printed nothing".into(),
            keep: false,
        });
    }
    Ok(format!("{text}\n"))
}

/// Keep bytes that could not go where they were meant to — output Bear
/// refused, or a note's text before `replace` wrote over it — so a slow or
/// costly command does not have to run again:
/// `<tmpdir>/bjorn-output-XXXX/<name>.md`. The directory is 0700 and the file
/// 0600 and new: an LLM's answer about a private note is as private as the note.
pub fn keep_output(name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    keep_output_in(&std::env::temp_dir(), name, bytes)
}

/// `keep_output` under `parent` instead of the temp dir, so a test can make it
/// fail without changing the environment every other test reads.
pub fn keep_output_in(parent: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = tempfile::Builder::new()
        .prefix("bjorn-output-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)?
        .keep();
    let path = dir.join(format!("{}.md", safe_filename(name)));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(bytes)?;
    Ok(path)
}

/// An existing `default = true` inside an `[[actions]]` entry, keeping what
/// follows it on the line (a comment, the newline).
static DEFAULT_TRUE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*default\s*=\s*)true\b(.*\n?)$").unwrap());

/// A TOML basic string: quoted, with backslashes, quotes and control
/// characters escaped.
fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `action` as an `[[actions]]` entry, writing only what differs from the
/// defaults apart from the format, which is worth seeing.
fn entry(action: &Action) -> String {
    let mut text = format!(
        "[[actions]]\nname = {}\ncommand = {}\nformat = {}\n",
        toml_string(&action.name),
        toml_string(&action.command),
        toml_string(&action.format)
    );
    if action.confirm {
        text.push_str("confirm = true\n");
    }
    if action.default {
        text.push_str("default = true\n");
    }
    if action.timeout.as_secs() != DEFAULT_TIMEOUT_SECONDS {
        text.push_str(&format!("timeout = {}\n", action.timeout.as_secs()));
    }
    if action.output.writes() {
        text.push_str(&format!("output = {}\n", toml_string(action.output.id())));
    }
    if let Some(section) = &action.section {
        text.push_str(&format!("section = {}\n", toml_string(section)));
    }
    text
}

/// `text` with every `default = true` inside an `[[actions]]` entry turned to
/// `false`. Other tables are left alone.
fn clear_defaults(text: &str) -> String {
    let mut in_actions = false;
    let mut out = String::with_capacity(text.len());
    let lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let quoted = inside_strings(&lines);
    for (line, quoted) in lines.iter().zip(quoted) {
        // A line inside a multi-line string is text, whatever it looks like.
        if quoted {
            out.push_str(line);
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let header: String = trimmed
                .split('#')
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            in_actions = header == "[[actions]]";
        } else if in_actions && let Some(caps) = DEFAULT_TRUE_RE.captures(line) {
            out.push_str(&caps[1]);
            out.push_str("false");
            out.push_str(&caps[2]);
            continue;
        }
        out.push_str(line);
    }
    out
}

/// Add `action` to the config file at `path` as a new `[[actions]]` entry at
/// the end. The file is edited as text, not rewritten, so comments and layout
/// survive. When the new action is the default, an existing entry's
/// `default = true` becomes `false`, because the first default wins. Nothing
/// is written unless the file parses before and after.
pub fn add_to_config(path: &Path, action: &Action) -> Result<(), ActionError> {
    let original = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ActionError(format!("{}: {e}", path.display()))),
    };
    let first_line = |e: toml::de::Error| e.to_string().lines().next().unwrap_or("").to_string();
    let before = match original.parse::<toml::Table>() {
        Ok(table) => table,
        Err(e) => {
            return Err(ActionError(format!(
                "{} does not parse, so nothing was written: {}",
                path.display(),
                first_line(e)
            )));
        }
    };
    let mut expected = read_entries(&before);
    if action.default {
        expected = cleared(expected);
    }
    expected.push(Some(action.clone()));
    let mut untouched: Vec<Option<toml::Table>> =
        raw_entries(&before).into_iter().map(Some).collect();
    untouched.push(None);
    let mut text = if action.default {
        clear_defaults(&original)
    } else {
        original
    };
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&entry(action));
    if let Err(e) = text.parse::<toml::Table>() {
        return Err(ActionError(format!(
            "the new entry did not parse, so nothing was written: {}",
            first_line(e)
        )));
    }
    if !reads_back(&before, &text, &expected, &untouched) {
        return Err(ActionError(
            "the config did not read back as expected, so nothing was written; add this one by hand."
                .into(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ActionError(format!("{}: {e}", parent.display())))?;
    }
    write_config(path, &text)
}

/// Put `text` in place of the config at `path` all at once: written to a temp
/// file beside it, flushed to disk, then renamed over it. A crash, a full disk
/// or a killed Bjorn mid-write leaves the old file whole, never half of a
/// hand-commented config. The temp file takes the old one's permissions, so a
/// config kept private stays private.
///
/// A config that is a symlink (a dotfile manager's) is written where it
/// points: the temp file goes beside the target and replaces that, so the
/// link survives and the file it names gets the change.
fn write_config(path: &Path, text: &str) -> Result<(), ActionError> {
    use std::io::Write;
    let failed = |at: &Path, e: std::io::Error| ActionError(format!("{}: {e}", at.display()));
    let target = match std::fs::canonicalize(path) {
        Ok(real) => real,
        // A link to nothing: renaming over it would swap the link for a file,
        // and guessing where its target should be made is worse.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && path.symlink_metadata().is_ok() => {
            return Err(ActionError(format!(
                "{} is a link to a file that does not exist, so nothing was written.",
                path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(failed(path, e)),
    };
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut temp = tempfile::Builder::new()
        .prefix(".bjorn-config-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|e| failed(dir, e))?;
    // A new config keeps the temp file's own 0600.
    if let Ok(meta) = std::fs::metadata(&target) {
        temp.as_file()
            .set_permissions(meta.permissions())
            .map_err(|e| failed(temp.path(), e))?;
    }
    temp.write_all(text.as_bytes())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|e| failed(temp.path(), e))?;
    temp.persist(&target)
        .map_err(|e| failed(&target, e.error))?;
    // The rename itself is only durable once the directory is flushed. It has
    // already happened, so a failure here is not worth reporting.
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

/// The first line of a TOML parse error; the rest is a drawing of the spot.
fn first_error_line(e: &toml::de::Error) -> String {
    e.to_string().lines().next().unwrap_or("").to_string()
}

/// A line that opens a table, `[name]` or `[[name]]`, with spaces and any
/// comment taken out.
fn table_header(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    Some(
        trimmed
            .split('#')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect(),
    )
}

/// Which lines sit inside a multi-line string (`'''` or `"""`), after the
/// line that opens it. Such a line is part of a value: `[ -f x ]` in a shell
/// command is not a table header, and `output=$(…)` is not the `output` key.
fn inside_strings(lines: &[String]) -> Vec<bool> {
    let mut open: Option<&str> = None;
    lines
        .iter()
        .map(|line| {
            if let Some(delim) = open {
                if closes(line, delim) {
                    open = None;
                }
                return true;
            }
            let code = line.trim_start();
            if !code.starts_with('#')
                && let Some((_, value)) = code.split_once('=')
            {
                let value = value.trim_start();
                for delim in ["\"\"\"", "'''"] {
                    if let Some(rest) = value.strip_prefix(delim) {
                        if !closes(rest, delim) {
                            open = Some(delim);
                        }
                        break;
                    }
                }
            }
            false
        })
        .collect()
}

/// Whether `text`, inside a multi-line string opened by `delim`, closes it.
/// A basic string (`"""`) honors backslash escapes, so `\"""` is a quote
/// followed by two more, not the end; a literal one (`'''`) has no escapes.
fn closes(text: &str, delim: &str) -> bool {
    if delim == "'''" {
        return text.contains(delim);
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' if bytes[i..].starts_with(b"\"\"\"") => return true,
            _ => i += 1,
        }
    }
    false
}

/// The `[[actions]]` headers in `lines`, leaving out look-alikes inside strings.
fn action_headers(lines: &[String]) -> Vec<usize> {
    let quoted = inside_strings(lines);
    (0..lines.len())
        .filter(|&i| !quoted[i] && table_header(&lines[i]).as_deref() == Some("[[actions]]"))
        .collect()
}

/// Where the table opened on line `start` ends: the next header, or the end.
fn block_end(lines: &[String], start: usize) -> usize {
    let quoted = inside_strings(lines);
    (start + 1..lines.len())
        .find(|&i| !quoted[i] && table_header(&lines[i]).is_some())
        .unwrap_or(lines.len())
}

/// The lines holding `key` in `lines[from..to]`: the key line, and for a
/// multi-line string every line down to the one that closes it. Lines inside
/// another key's multi-line string are never taken for the key.
fn key_lines(lines: &[String], from: usize, to: usize, key: &str) -> Option<(usize, usize)> {
    let pattern = Regex::new(&format!(r"^\s*{}\s*=", regex::escape(key))).unwrap();
    let quoted = inside_strings(lines);
    let start = (from..to).find(|&i| !quoted[i] && pattern.is_match(&lines[i]))?;
    let value = lines[start]
        .split_once('=')
        .map(|(_, v)| v)
        .unwrap_or("")
        .trim_start();
    for delim in ["\"\"\"", "'''"] {
        if let Some(rest) = value.strip_prefix(delim) {
            if closes(rest, delim) {
                return Some((start, start));
            }
            let end = (start + 1..to)
                .find(|&i| closes(&lines[i], delim))
                .unwrap_or(to.saturating_sub(1).max(start));
            return Some((start, end));
        }
    }
    Some((start, start))
}

/// Whether two `key = value` snippets hold the same value, whatever their
/// spacing, quoting or trailing comment.
fn same_value(current: &str, wanted: &str, key: &str) -> bool {
    let read = |text: &str| {
        text.trim_start()
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| t.get(key).cloned())
    };
    let now = read(current);
    now.is_some() && now == read(wanted)
}

/// Every action the config text holds, in order, as the app reads them.
fn read_entries(table: &toml::Table) -> Vec<Option<Action>> {
    match table.get("actions") {
        Some(toml::Value::Array(entries)) => entries
            .iter()
            .map(|e| e.as_table().and_then(crate::config::parse_action))
            .collect(),
        _ => Vec::new(),
    }
}

/// Every `[[actions]]` entry as TOML, the way it is written.
fn raw_entries(table: &toml::Table) -> Vec<toml::Table> {
    match table.get("actions") {
        Some(toml::Value::Array(entries)) => entries
            .iter()
            .map(|e| e.as_table().cloned().unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    }
}

/// `actions` as they read after their `default` flags are cleared, which is
/// all a write that sets a new default may change about them.
fn cleared(actions: Vec<Option<Action>>) -> Vec<Option<Action>> {
    actions
        .into_iter()
        .map(|a| {
            a.map(|a| Action {
                default: false,
                ..a
            })
        })
        .collect()
}

/// The read-back every config write passes before it is written: `result`
/// parses, every table but `[[actions]]` is as it was in `before`, the actions
/// read (through `config::parse_action`, as the app reads them) exactly as
/// `expected`, and every entry the write did not mean to touch — `untouched`,
/// in the new order, `None` for the one it did — keeps each key it had apart
/// from `default`. That catches a line edited where it should not have been,
/// such as a `default = true` inside a multi-line command.
fn reads_back(
    before: &toml::Table,
    result: &str,
    expected: &[Option<Action>],
    untouched: &[Option<toml::Table>],
) -> bool {
    let Ok(after) = result.parse::<toml::Table>() else {
        return false;
    };
    let others = |t: &toml::Table| {
        let mut t = t.clone();
        t.remove("actions");
        t
    };
    let without_default = |t: &toml::Table| {
        let mut t = t.clone();
        t.remove("default");
        t
    };
    let raw = raw_entries(&after);
    others(before) == others(&after)
        && read_entries(&after) == expected
        && raw.len() == untouched.len()
        && raw.iter().zip(untouched).all(|(now, was)| match was {
            Some(was) => without_default(now) == without_default(was),
            None => true,
        })
}

/// Rewrite the `[[actions]]` entry that reads as `original` so it reads as
/// `updated`. Only lines whose value changes are touched, so comments and keys
/// the form does not show stay as they were, and a missing key is added after
/// the entry's last one. If `updated` is the default, other entries lose
/// theirs. The result is read back and must hold `updated` where `original`
/// was, or nothing is written.
pub fn update_in_config(
    path: &Path,
    original: &Action,
    updated: &Action,
) -> Result<(), ActionError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ActionError(format!("{}: {e}", path.display())))?;
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        ActionError(format!(
            "{} does not parse, so nothing was written: {}",
            path.display(),
            first_error_line(&e)
        ))
    })?;
    let entries = read_entries(&table);
    let position = entries
        .iter()
        .position(|e| e.as_ref() == Some(original))
        .ok_or_else(|| {
            ActionError(format!(
                "“{}” is no longer in {} as it was read, so nothing was written.",
                original.name,
                path.display()
            ))
        })?;

    let text = if updated.default {
        clear_defaults(&text)
    } else {
        text
    };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    let headers = action_headers(&lines);
    if headers.len() != entries.len() {
        return Err(ActionError(format!(
            "{} does not write every action as an [[actions]] block, so edit this one by hand.",
            path.display()
        )));
    }
    let header = headers[position];
    let seconds = updated.timeout.as_secs();
    let wanted = [
        ("name", toml_string(&updated.name), true),
        ("command", toml_string(&updated.command), true),
        ("format", toml_string(&updated.format), true),
        ("confirm", updated.confirm.to_string(), updated.confirm),
        ("default", updated.default.to_string(), updated.default),
        // Written as "" when cleared: `parse_action` reads a blank one back as
        // None, so the read-back below still matches.
        (
            "prompt",
            toml_string(updated.prompt.as_deref().unwrap_or("")),
            updated.prompt.is_some(),
        ),
        (
            "timeout",
            seconds.to_string(),
            seconds != DEFAULT_TIMEOUT_SECONDS,
        ),
        (
            "output",
            toml_string(updated.output.id()),
            updated.output.writes(),
        ),
        // Blank when cleared, which reads back as None, like `prompt`.
        (
            "section",
            toml_string(updated.section.as_deref().unwrap_or("")),
            updated.section.is_some(),
        ),
    ];
    for (key, value, needed) in wanted {
        let end = block_end(&lines, header);
        let line = format!("{key} = {value}\n");
        match key_lines(&lines, header + 1, end, key) {
            Some((first, last)) => {
                if same_value(&lines[first..=last].concat(), &line, key) {
                    continue;
                }
                let indent: String = lines[first]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                lines.splice(first..=last, [format!("{indent}{line}")]);
            }
            None if needed => {
                // After the entry's last key, ahead of blank lines and comments
                // that separate it from what follows.
                let at = (header + 1..end)
                    .rev()
                    .find(|&i| {
                        let t = lines[i].trim();
                        !t.is_empty() && !t.starts_with('#')
                    })
                    .map_or(header + 1, |i| i + 1);
                lines.insert(at, line);
            }
            None => {}
        }
    }

    let result = lines.concat();
    // Every entry, not just this one: the others may lose `default` when this
    // one takes it, and nothing else.
    let mut expected = if updated.default {
        cleared(entries)
    } else {
        entries
    };
    expected[position] = Some(updated.clone());
    let mut untouched: Vec<Option<toml::Table>> =
        raw_entries(&table).into_iter().map(Some).collect();
    untouched[position] = None;
    if !reads_back(&table, &result, &expected, &untouched) {
        return Err(ActionError(
            "the edited entry did not read back as written, so nothing was written; edit this one by hand."
                .into(),
        ));
    }
    write_config(path, &result)
}

/// Remove the `[[actions]]` entry that reads as `original`: its header and
/// every line down to its last key. Comments and blank lines after that key
/// stay, since they often introduce what follows (commented-out templates, the
/// next section). The result is read back and must hold every other action
/// unchanged, or nothing is written.
pub fn remove_from_config(path: &Path, original: &Action) -> Result<(), ActionError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ActionError(format!("{}: {e}", path.display())))?;
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        ActionError(format!(
            "{} does not parse, so nothing was written: {}",
            path.display(),
            first_error_line(&e)
        ))
    })?;
    let entries = read_entries(&table);
    let position = entries
        .iter()
        .position(|e| e.as_ref() == Some(original))
        .ok_or_else(|| {
            ActionError(format!(
                "“{}” is no longer in {} as it was read, so nothing was written.",
                original.name,
                path.display()
            ))
        })?;

    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    let headers = action_headers(&lines);
    if headers.len() != entries.len() {
        return Err(ActionError(format!(
            "{} does not write every action as an [[actions]] block, so delete this one by hand.",
            path.display()
        )));
    }
    let header = headers[position];
    let end = block_end(&lines, header);
    let last = (header + 1..end)
        .rev()
        .find(|&i| {
            let t = lines[i].trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .unwrap_or(header);
    lines.drain(header..=last);
    // No double blank line where the entry was.
    let blank = |line: Option<&String>| line.is_some_and(|l| l.trim().is_empty());
    if blank(lines.get(header)) && (header == 0 || blank(lines.get(header - 1))) {
        lines.remove(header);
    }

    let result = lines.concat();
    let mut expected = entries;
    expected.remove(position);
    let mut untouched: Vec<Option<toml::Table>> =
        raw_entries(&table).into_iter().map(Some).collect();
    untouched.remove(position);
    if !reads_back(&table, &result, &expected, &untouched) {
        return Err(ActionError(
            "the config did not read back as expected, so nothing was written; delete this one by hand."
                .into(),
        ));
    }
    write_config(path, &result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(name: &str, command: &str) -> Action {
        Action {
            name: name.into(),
            command: command.into(),
            ..Action::default()
        }
    }

    /// Scattered letters inside a command must not match: a long command
    /// contains almost any short query as a subsequence, which made the menu
    /// keep rows the query had nothing to do with.
    #[test]
    fn scattered_letters_in_a_command_do_not_match() {
        let a = action(
            "Copy",
            "{ printf '%s' \"$BJORN_ACTION\"; cat; } > '/tmp/.tmpu0b1/Copy.receipt'; echo sent",
        );
        assert!(
            !a.matches("pub"),
            "p, u and b appear in order but 'pub' does not"
        );
        assert!(a.matches("copy"), "its own name still matches");
        assert!(
            a.matches("printf"),
            "a real substring of the command matches"
        );
    }

    #[test]
    fn an_empty_query_keeps_everything() {
        let actions = vec![action("Copy", "pbcopy"), action("Publish", "scp x y")];
        assert_eq!(filter(&actions, "   ").len(), 2);
    }

    #[test]
    fn matching_is_the_name_by_subsequence_and_the_command_by_substring() {
        let a = action(
            "Publish to S3",
            "aws s3 cp \"$BJORN_NOTE_FILE\" s3://notes/",
        );
        assert!(a.matches(""));
        assert!(a.matches("s3"));
        assert!(a.matches("PUB"));
        assert!(a.matches("pblsh"), "letters in order, gaps allowed");
        assert!(a.matches("aws"), "the command is searched too");
        assert!(a.matches("S3://NOTES"), "and case-insensitively");
        assert!(!a.matches("zebra"));
    }

    #[test]
    fn default_is_the_marked_one_or_the_only_one() {
        assert!(default_action(&[]).is_none());
        let one = vec![action("Only", "true")];
        assert_eq!(default_action(&one).unwrap().name, "Only");
        let two = vec![action("A", "true"), action("B", "true")];
        assert!(default_action(&two).is_none(), "two, neither marked");
        let marked = vec![
            action("A", "true"),
            Action {
                default: true,
                ..action("B", "true")
            },
        ];
        assert_eq!(default_action(&marked).unwrap().name, "B");
    }

    #[test]
    fn summary_takes_the_first_line_and_falls_back_to_stderr() {
        assert_eq!(summarize("\n  done: s3://x  \nmore\n", ""), "done: s3://x");
        assert_eq!(summarize("", "boom"), "boom");
        assert_eq!(summarize("", ""), "");
        let long = "x".repeat(OUTPUT_LIMIT + 10);
        assert_eq!(summarize(&long, "").chars().count(), OUTPUT_LIMIT + 1);
    }

    /// Run the command and keep its first line, as a toast action does.
    async fn run(
        action: &Action,
        note: &Note,
        content: &str,
        images: &HashMap<String, Vec<u8>>,
        input: &str,
    ) -> Result<String, ActionError> {
        execute(action, note, content, images, input)
            .await
            .map(|c| c.summary())
    }

    fn note() -> Note {
        Note {
            id: "NOTE-1".into(),
            title: "Sprint Planning".into(),
            tags: vec!["work/sprint".into()],
            ..Note::default()
        }
    }

    #[tokio::test]
    async fn the_command_sees_the_file_the_metadata_and_stdin() {
        let out = run(
            &action(
                "Echo",
                "printf '%s|%s|%s|' \"$BJORN_NOTE_TITLE\" \"$BJORN_NOTE_TAGS\" \"$BJORN_ACTION\"; \
                 cat \"$BJORN_NOTE_FILE\" | head -1; cat > piped; wc -c < piped",
            ),
            &note(),
            "# Sprint Planning\n\nbody",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();
        assert_eq!(out, "Sprint Planning|work/sprint|Echo|# Sprint Planning");
    }

    #[tokio::test]
    async fn the_file_is_named_after_the_note_and_the_format() {
        let out = run(
            &Action {
                format: "txt".into(),
                ..action("Name", "basename \"$BJORN_NOTE_FILE\"")
            },
            &note(),
            "# Sprint Planning\n\nbody",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();
        assert_eq!(out, "Sprint Planning.txt");
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_exit_code_and_message() {
        let err = run(
            &action("Nope", "echo 'no credentials' >&2; exit 3"),
            &note(),
            "body",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, "exit 3: no credentials");
    }

    #[tokio::test]
    async fn a_hung_command_is_stopped_at_the_timeout() {
        let err = run(
            &Action {
                timeout: Duration::from_millis(120),
                ..action("Sleep", "sleep 30")
            },
            &note(),
            "body",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("did not finish within"), "{}", err.0);
    }

    #[tokio::test]
    async fn the_temp_file_is_gone_once_the_command_ends() {
        let path = run(
            &action("Where", "printf '%s' \"$BJORN_NOTE_FILE\""),
            &note(),
            "body",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();
        assert!(!std::path::Path::new(&path).exists(), "{path}");
    }
    #[test]
    fn a_prompt_can_be_added_changed_and_cleared_by_an_edit() {
        // `update_in_config` compares the whole Action on read-back, so a key it
        // cannot write is a key that makes every edit fail.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bjorn/config.toml");
        let plain = Action {
            name: "Sync".into(),
            command: "sync".into(),
            ..Action::default()
        };
        add_to_config(&path, &plain).unwrap();

        let asking = Action {
            prompt: Some("Range".into()),
            ..plain.clone()
        };
        update_in_config(&path, &plain, &asking).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("prompt = \"Range\"\n"),
            "{}",
            std::fs::read_to_string(&path).unwrap()
        );
        assert_eq!(
            crate::config::Config::load(Some(&path)).unwrap().actions[0],
            asking
        );

        let reworded = Action {
            prompt: Some("Which range".into()),
            ..plain.clone()
        };
        update_in_config(&path, &asking, &reworded).unwrap();
        assert_eq!(
            crate::config::Config::load(Some(&path)).unwrap().actions[0],
            reworded
        );

        update_in_config(&path, &reworded, &plain).unwrap();
        assert_eq!(
            crate::config::Config::load(Some(&path)).unwrap().actions[0],
            plain
        );
    }

    #[test]
    fn output_values_parse_loosely_and_nothing_else_writes() {
        assert_eq!(ActionOutput::parse("append"), Some(ActionOutput::Append));
        assert_eq!(
            ActionOutput::parse(" New_Note "),
            Some(ActionOutput::NewNote)
        );
        assert_eq!(ActionOutput::parse("REPLACE"), Some(ActionOutput::Replace));
        assert_eq!(ActionOutput::parse("apend"), None);
        for output in ActionOutput::ALL {
            assert_eq!(ActionOutput::parse(output.id()), Some(output));
        }
        assert!(!ActionOutput::Toast.writes());
    }

    fn captured(stdout: &str) -> Captured {
        Captured {
            stdout: stdout.into(),
            ..Captured::default()
        }
    }

    #[test]
    fn text_for_bear_drops_blank_lines_around_the_output_only() {
        assert_eq!(
            text_for_bear(&captured("\n\n  - item\n  more\n\n\n")).unwrap(),
            "  - item\n  more\n",
            "leading indentation on the first real line stays"
        );
        assert_eq!(text_for_bear(&captured("# T")).unwrap(), "# T\n");
        let empty = text_for_bear(&captured(" \n\t\n")).unwrap_err();
        assert_eq!(empty.reason, "it printed nothing");
        assert!(!empty.keep);
        let big = Captured {
            truncated: true,
            ..captured("# Big")
        };
        let refused = text_for_bear(&big).unwrap_err();
        assert!(refused.reason.contains("more than 1 MB"), "{refused:?}");
        assert!(refused.keep, "the first megabyte is worth keeping");
    }

    #[test]
    fn text_for_bear_takes_out_control_characters_and_carriage_returns() {
        assert_eq!(
            text_for_bear(&captured("# T\r\n\x1b[31mred\x1b[0m\x07\tok\x7f\u{85}\r\n")).unwrap(),
            "# T\n[31mred[0m\tok\n"
        );
        let bytes = Captured {
            stdout: b"# T\n\xff\xfe".to_vec(),
            ..Captured::default()
        };
        let refused = text_for_bear(&bytes).unwrap_err();
        assert!(refused.reason.contains("not UTF-8"), "{refused:?}");
        assert!(refused.keep);
    }

    #[test]
    fn kept_output_is_private_and_its_name_fits_the_file_system() {
        use std::os::unix::fs::PermissionsExt;
        let name = "é".repeat(200);
        let path = keep_output(&name, b"\xffraw").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert!(path.file_name().unwrap().len() <= 255);
        assert_eq!(std::fs::read(&path).unwrap(), b"\xffraw");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    /// A command that never reads a note bigger than the pipe buffer used to
    /// leave the feed blocked outside the timeout, so it was never stopped.
    #[tokio::test]
    async fn a_command_that_ignores_a_big_note_is_still_stopped_on_time() {
        let big = "x".repeat(400 * 1024);
        let started = std::time::Instant::now();
        let err = execute(
            &Action {
                timeout: Duration::from_secs(1),
                ..action("Sleep", "sleep 20")
            },
            &note(),
            &big,
            &HashMap::new(),
            "",
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("did not finish within 1 s"), "{}", err.0);
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_timeout_stops_the_whole_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let err = execute(
            &Action {
                timeout: Duration::from_millis(500),
                ..action(
                    "Pipe",
                    &format!(
                        "sh -c 'echo $$ > \"$0\"; exec sleep 30' '{}' | cat",
                        pidfile.display()
                    ),
                )
            },
            &note(),
            "body",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap_err();
        assert!(err.0.contains("did not finish"), "{}", err.0);
        let pid: libc::pid_t = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the grandchild {pid} outlived the timeout"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[tokio::test]
    async fn execute_keeps_only_the_first_part_of_a_flood() {
        let got = execute(
            &action(
                "Flood",
                "head -c 1200000 /dev/zero | tr '\\0' 'a'; echo done >&2",
            ),
            &note(),
            "body",
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();
        assert!(got.truncated);
        assert_eq!(got.stdout.len(), WRITE_LIMIT);
        assert_eq!(got.stderr, "done\n", "stderr is read alongside");
    }

    #[test]
    fn output_and_section_are_written_changed_and_cleared_by_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bjorn/config.toml");
        let plain = action("Sum", "llm");
        add_to_config(&path, &plain).unwrap();
        let load = || crate::config::Config::load(Some(&path)).unwrap().actions[0].clone();

        let appending = Action {
            output: ActionOutput::Append,
            section: Some("## Summary".into()),
            ..plain.clone()
        };
        update_in_config(&path, &plain, &appending).unwrap();
        assert_eq!(load(), appending);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("output = \"append\"\nsection = \"## Summary\"\n"),
            "{text}"
        );

        let replacing = Action {
            output: ActionOutput::Replace,
            section: None,
            ..plain.clone()
        };
        update_in_config(&path, &appending, &replacing).unwrap();
        assert_eq!(load(), replacing);

        update_in_config(&path, &replacing, &plain).unwrap();
        assert_eq!(load(), plain);
    }

    #[test]
    fn keys_and_headers_inside_a_multi_line_command_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // The script's own `output=`, `[ … ]` and `default = true` lines look
        // like keys and a header, and are none of them.
        let body = "[[actions]]\nname = \"Stamp\"\ncommand = \'\'\'output=$(date)\n[ -n \"$output\" ] && echo \"$output\"\ndefault = true\'\'\'\n\n\
                    [[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n";
        std::fs::write(&path, body).unwrap();
        let load = || crate::config::Config::load(Some(&path)).unwrap().actions;
        let before = load();
        assert_eq!(before.len(), 2);
        assert!(!before[0].default);
        let edited = Action {
            output: ActionOutput::NewNote,
            ..before[0].clone()
        };
        update_in_config(&path, &before[0], &edited).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("[[actions]]\nname = \"Stamp\"\ncommand = \'\'\'output=$(date)\n[ -n")
                && text.contains("default = true\'\'\'\nformat = \"md\"\noutput = \"new-note\"\n"),
            "{text}"
        );
        assert_eq!(load()[0], edited);

        // Making the other one the default leaves the script's line alone.
        let copy = Action {
            default: true,
            ..before[1].clone()
        };
        update_in_config(&path, &before[1], &copy).unwrap();
        assert_eq!(load()[0], edited);

        remove_from_config(&path, &copy).unwrap();
        assert_eq!(load(), vec![edited]);
    }

    /// `\"""` in a basic multi-line string is an escaped quote and two more,
    /// not its end. Taken for the end, the `default = true` line after it
    /// looked like a key, and making another action the default flipped it.
    const ESCAPED_QUOTES: &str = "[[actions]]\nname = \"Quote\"\ncommand = \"\"\"\necho \\\"\"\"\ndefault = true\n\"\"\"\n\n\
                                  [[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n";

    #[test]
    fn an_escaped_quote_does_not_end_a_multi_line_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, ESCAPED_QUOTES).unwrap();
        let load = || crate::config::Config::load(Some(&path)).unwrap().actions;
        let before = load();
        assert_eq!(before[0].command, "echo \"\"\"\ndefault = true");
        assert!(!before[0].default);

        let copy = Action {
            default: true,
            ..before[1].clone()
        };
        update_in_config(&path, &before[1], &copy).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("echo \\\"\"\"\ndefault = true\n\"\"\""),
            "the command's own line is untouched"
        );
        assert_eq!(load(), vec![before[0].clone(), copy.clone()]);

        let new = Action {
            default: true,
            ..action("New", "true")
        };
        add_to_config(&path, &new).unwrap();
        let after = load();
        assert_eq!(after[0], before[0]);
        assert!(!after[1].default && after[2].default);
    }

    /// The read-back is what stops a stray edit when the scanner is wrong:
    /// flipping the command's line by hand must fail it.
    #[test]
    fn the_read_back_refuses_a_change_to_an_entry_it_did_not_mean_to_touch() {
        let before: toml::Table = ESCAPED_QUOTES.parse().unwrap();
        let entries = read_entries(&before);
        let untouched: Vec<Option<toml::Table>> =
            raw_entries(&before).into_iter().map(Some).collect();
        assert!(reads_back(&before, ESCAPED_QUOTES, &entries, &untouched));
        let flipped = ESCAPED_QUOTES.replace("default = true\n\"\"\"", "default = false\n\"\"\"");
        assert!(!reads_back(&before, &flipped, &entries, &untouched));
        let other_table = format!("theme = \"x\"\n{ESCAPED_QUOTES}");
        assert!(!reads_back(&before, &other_table, &entries, &untouched));
        let mut renamed = entries.clone();
        renamed[1] = Some(action("Copy", "pbcopy"));
        assert!(reads_back(&before, ESCAPED_QUOTES, &renamed, &untouched));
        renamed[1] = Some(action("Paste", "pbcopy"));
        assert!(!reads_back(&before, ESCAPED_QUOTES, &renamed, &untouched));
    }

    #[test]
    fn replace_always_asks_and_interactive_output_is_refused() {
        let replace = Action {
            output: ActionOutput::Replace,
            ..action("R", "true")
        };
        assert!(replace.asks_first());
        assert!(!action("T", "true").asks_first());
        assert!(replace.misconfigured().is_none());
        let chat = Action {
            interactive: true,
            output: ActionOutput::NewNote,
            ..action("Chat", "true")
        };
        assert!(
            chat.misconfigured()
                .unwrap()
                .contains("an interactive action has no output")
        );
        let html_replace = Action {
            format: "html".into(),
            ..replace.clone()
        };
        assert!(
            html_replace
                .misconfigured()
                .unwrap()
                .contains("needs format = \"md\"")
        );
        let stray_section = Action {
            section: Some("## S".into()),
            ..action("S", "true")
        };
        assert!(
            stray_section
                .misconfigured()
                .unwrap()
                .contains("only used with")
        );
        let session = Action {
            interactive: true,
            ..action("Session", "true")
        };
        assert!(session.misconfigured().is_none());
    }

    #[test]
    fn a_new_action_is_appended_and_the_file_around_it_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bjorn/config.toml");
        let first = Action {
            name: "Copy".into(),
            command: "pbcopy".into(),
            default: true,
            ..Action::default()
        };
        add_to_config(&path, &first).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("[[actions]]\nname = \"Copy\""), "{body}");

        std::fs::write(&path, format!("# mine\n[other]\ndefault = true\n\n{body}")).unwrap();
        let second = Action {
            name: "Quote \"it\"".into(),
            command: "printf '%s\\n' \"$BJORN_NOTE_TITLE\" C:\\path".into(),
            format: "html".into(),
            format_error: None,
            confirm: true,
            prompt: None,
            interactive: false,
            output: ActionOutput::Append,
            section: Some("## Summary \"now\"".into()),
            output_error: None,
            default: true,
            timeout: Duration::from_secs(90),
        };
        add_to_config(&path, &second).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.starts_with("# mine\n[other]\ndefault = true\n"),
            "only [[actions]] entries lose their default: {body}"
        );
        let cfg = crate::config::Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.actions.len(), 2);
        assert!(!cfg.actions[0].default, "{body}");
        assert_eq!(cfg.actions[1], second, "round trip: {body}");
        assert_eq!(default_action(&cfg.actions).unwrap().name, "Quote \"it\"");
    }

    #[test]
    fn a_config_that_does_not_parse_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \n").unwrap();
        let err = add_to_config(&path, &action("X", "true")).unwrap_err();
        assert!(err.0.contains("nothing was written"), "{}", err.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \n");
    }

    #[test]
    fn an_edit_changes_only_the_lines_it_must() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let body = "# mine\n\n[[actions]]\nname = \"Copy\"   # short\ncommand = \"pbcopy\"\ndefault = true\n\n\
                    [[actions]]\n# uploads\nname = \"Upload\"\ncommand = '''\nscp \"$BJORN_NOTE_FILE\" host:notes/\n'''\ntimeout = 300\n\n\
                    [reminders]\nenabled = false\n";
        std::fs::write(&path, body).unwrap();
        let before = crate::config::Config::load(Some(&path)).unwrap().actions;
        let upload = before[1].clone();
        let edited = Action {
            name: "Upload \"notes\"".into(),
            command: "rsync -a \"$BJORN_NOTE_FILE\" host:notes/".into(),
            format: "html".into(),
            confirm: true,
            default: true,
            ..upload.clone()
        };
        update_in_config(&path, &upload, &edited).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after,
            "# mine\n\n[[actions]]\nname = \"Copy\"   # short\ncommand = \"pbcopy\"\ndefault = false\n\n\
             [[actions]]\n# uploads\nname = \"Upload \\\"notes\\\"\"\ncommand = \"rsync -a \\\"$BJORN_NOTE_FILE\\\" host:notes/\"\n\
             timeout = 300\nformat = \"html\"\nconfirm = true\ndefault = true\n\n\
             [reminders]\nenabled = false\n"
        );
        let cfg = crate::config::Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.actions[1], edited);
        assert_eq!(
            default_action(&cfg.actions).unwrap().name,
            "Upload \"notes\""
        );
    }

    #[test]
    fn an_edit_of_an_action_that_changed_on_disk_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let body = "[[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n";
        std::fs::write(&path, body).unwrap();
        let stale = action("Copy", "pbcopy -Prefer txt");
        let err = update_in_config(&path, &stale, &action("Copy", "true")).unwrap_err();
        assert!(err.0.contains("nothing was written"), "{}", err.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn deleting_an_action_keeps_its_neighbours_and_the_comments_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let body = "theme = \"red-graphite\"\n\n[[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n\n\
                    [[actions]]\n# uploads\nname = \"Upload\"\ncommand = \"scp x y\"\nconfirm = true\n\n\
                    # --- templates ---\n# [[actions]]\n# name = \"S3\"\n";
        std::fs::write(&path, body).unwrap();
        let actions = crate::config::Config::load(Some(&path)).unwrap().actions;
        remove_from_config(&path, &actions[1]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theme = \"red-graphite\"\n\n[[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n\n\
             # --- templates ---\n# [[actions]]\n# name = \"S3\"\n"
        );
        remove_from_config(&path, &actions[0]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "theme = \"red-graphite\"\n\n# --- templates ---\n# [[actions]]\n# name = \"S3\"\n"
        );
        assert!(
            crate::config::Config::load(Some(&path))
                .unwrap()
                .actions
                .is_empty()
        );
    }

    #[test]
    fn deleting_an_action_that_changed_on_disk_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let body = "[[actions]]\nname = \"Copy\"\ncommand = \"pbcopy\"\n";
        std::fs::write(&path, body).unwrap();
        let err = remove_from_config(&path, &action("Copy", "pbcopy -Prefer txt")).unwrap_err();
        assert!(err.0.contains("nothing was written"), "{}", err.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn config_writes_keep_the_file_mode_and_leave_no_temp_file_behind() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# mine\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        add_to_config(&path, &action("Copy", "pbcopy")).unwrap();
        assert_eq!(mode(&path), 0o640, "an add");
        let copy = crate::config::Config::load(Some(&path)).unwrap().actions[0].clone();
        update_in_config(&path, &copy, &action("Copy", "pbcopy -Prefer txt")).unwrap();
        assert_eq!(mode(&path), 0o640, "an edit");
        let copy = crate::config::Config::load(Some(&path)).unwrap().actions[0].clone();
        assert_eq!(copy.command, "pbcopy -Prefer txt");
        remove_from_config(&path, &copy).unwrap();
        assert_eq!(mode(&path), 0o640, "a delete");
        // The blank line the add put ahead of the entry stays.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n\n");
        assert_eq!(names_in(dir.path()), ["config.toml"]);
    }

    /// A dotfile manager's link must still be a link afterwards, pointing where
    /// it did, with the change in the file it names.
    #[test]
    fn a_symlinked_config_stays_a_link_and_its_target_gets_the_change() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles");
        let config_dir = dir.path().join("bjorn");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        let target = dotfiles.join("bjorn.toml");
        std::fs::write(&target, "# managed\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = config_dir.join("config.toml");
        let points_to = Path::new("../dotfiles/bjorn.toml");
        std::os::unix::fs::symlink(points_to, &link).unwrap();
        let still_linked = |what: &str| {
            assert!(
                link.symlink_metadata().unwrap().file_type().is_symlink(),
                "{what}"
            );
            assert_eq!(std::fs::read_link(&link).unwrap(), points_to, "{what}");
        };

        add_to_config(&link, &action("Copy", "pbcopy")).unwrap();
        still_linked("an add");
        let written = std::fs::read_to_string(&target).unwrap();
        assert!(
            written.starts_with("# managed\n") && written.contains("name = \"Copy\""),
            "{written}"
        );
        let copy = crate::config::Config::load(Some(&link)).unwrap().actions[0].clone();
        update_in_config(&link, &copy, &action("Copy", "pbcopy -Prefer txt")).unwrap();
        still_linked("an edit");
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("pbcopy -Prefer txt")
        );
        let copy = crate::config::Config::load(Some(&link)).unwrap().actions[0].clone();
        remove_from_config(&link, &copy).unwrap();
        still_linked("a delete");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "# managed\n\n");
        assert_eq!(mode(&target), 0o600);
        assert_eq!(names_in(&dotfiles), ["bjorn.toml"]);
        assert_eq!(names_in(&config_dir), ["config.toml"]);
    }

    #[test]
    fn a_link_to_a_missing_config_is_not_swapped_for_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(dir.path().join("gone.toml"), &link).unwrap();
        let err = add_to_config(&link, &action("Copy", "pbcopy")).unwrap_err();
        assert!(err.0.contains("does not exist"), "{}", err.0);
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(names_in(dir.path()), ["config.toml"]);
    }

    #[test]
    fn an_unknown_format_refuses_the_action_and_names_the_valid_ones() {
        let typo = Action {
            format_error: Some("\"pfd\"".into()),
            ..action("Print", "weasyprint - out.pdf")
        };
        assert_eq!(
            typo.misconfigured().as_deref(),
            Some("format = \"pfd\" is not one of md, html, txt, rtf, textbundle")
        );
    }
}
