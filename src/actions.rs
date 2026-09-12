//! Actions: user commands run against the note under the cursor.
//!
//! An action is a shell command from the config file. Bjorn renders the note
//! the same way the exporter does, writes it to a temp file, then runs the
//! command with that file as `$BJORN_NOTE_FILE`, the note's text on stdin and
//! the note's metadata in the environment. What the command does with it —
//! `aws s3 cp`, `scp`, `curl`, `gh gist create` — is the user's business; the
//! only thing Bjorn reports back is the exit status and the last line of
//! output. The temp file goes away when the command ends.

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
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
/// Characters of a command's output kept for the toast.
pub const OUTPUT_LIMIT: usize = 300;

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
    /// Ask before running. For anything that publishes or deletes.
    pub confirm: bool,
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
            confirm: false,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
            default: false,
        }
    }
}

impl Action {
    /// Case-insensitive subsequence match on the name, then on the command, so
    /// `s3` finds "Publish to S3" and `curl` finds an action by what it runs.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }
        subsequence(&self.name, query) || subsequence(&self.command, query)
    }
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

/// The environment an action's command sees, on top of Bjorn's own.
pub fn environment(action: &Action, note: &Note, file: &std::path::Path) -> Vec<(String, String)> {
    let stamp =
        |t: Option<chrono::DateTime<chrono::Utc>>| t.map(|t| t.to_rfc3339()).unwrap_or_default();
    vec![
        ("BJORN_ACTION".to_string(), action.name.clone()),
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

/// Render the note and run the command. Returns whatever the command said, or
/// the failure. `images` is only read by the formats that want attachments.
pub async fn run(
    action: &Action,
    note: &Note,
    content: &str,
    images: &HashMap<String, Vec<u8>>,
) -> Result<String, ActionError> {
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
    let stdin_body = if file.is_dir() {
        std::fs::read(file.join("text.md")).unwrap_or_default()
    } else {
        std::fs::read(&file).unwrap_or_default()
    };

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&action.command)
        .current_dir(dir.path())
        .envs(environment(action, note, &file))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ActionError(format!("could not run the command: {e}")))?;
    let mut handle = child.stdin.take();
    let feed = async {
        if let Some(mut handle) = handle.take() {
            let _ = handle.write_all(&stdin_body).await;
            let _ = handle.shutdown().await;
        }
    };
    let (_, waited) = tokio::join!(
        feed,
        tokio::time::timeout(action.timeout, child.wait_with_output())
    );
    let output = match waited {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(ActionError(format!("the command failed to run: {e}"))),
        Err(_) => {
            return Err(ActionError(format!(
                "“{}” did not finish within {} s and was stopped.",
                action.name,
                action.timeout.as_secs()
            )));
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // The temp file lives exactly as long as the command does.
    drop(dir);
    if output.status.success() {
        Ok(summarize(&stdout, &stderr))
    } else {
        let code = output
            .status
            .code()
            .map(|c| format!("exit {c}"))
            .unwrap_or_else(|| "killed by a signal".to_string());
        let detail = summarize(&stderr, &stdout);
        Err(ActionError(if detail.is_empty() {
            code
        } else {
            format!("{code}: {detail}")
        }))
    }
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
    text
}

/// `text` with every `default = true` inside an `[[actions]]` entry turned to
/// `false`. Other tables are left alone.
fn clear_defaults(text: &str) -> String {
    let mut in_actions = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
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
    if let Err(e) = original.parse::<toml::Table>() {
        return Err(ActionError(format!(
            "{} does not parse, so nothing was written: {}",
            path.display(),
            first_line(e)
        )));
    }
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ActionError(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(path, text).map_err(|e| ActionError(format!("{}: {e}", path.display())))
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

/// Where the table opened on line `start` ends: the next header, or the end.
fn block_end(lines: &[String], start: usize) -> usize {
    (start + 1..lines.len())
        .find(|&i| table_header(&lines[i]).is_some())
        .unwrap_or(lines.len())
}

/// The lines holding `key` in `lines[from..to]`: the key line, and for a
/// multi-line string every line down to the one that closes it.
fn key_lines(lines: &[String], from: usize, to: usize, key: &str) -> Option<(usize, usize)> {
    let pattern = Regex::new(&format!(r"^\s*{}\s*=", regex::escape(key))).unwrap();
    let start = (from..to).find(|&i| pattern.is_match(&lines[i]))?;
    let value = lines[start]
        .split_once('=')
        .map(|(_, v)| v)
        .unwrap_or("")
        .trim_start();
    for delim in ["\"\"\"", "'''"] {
        if let Some(rest) = value.strip_prefix(delim) {
            if rest.contains(delim) {
                return Some((start, start));
            }
            let end = (start + 1..to)
                .find(|&i| lines[i].contains(delim))
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
    let headers: Vec<usize> = (0..lines.len())
        .filter(|&i| table_header(&lines[i]).as_deref() == Some("[[actions]]"))
        .collect();
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
        (
            "timeout",
            seconds.to_string(),
            seconds != DEFAULT_TIMEOUT_SECONDS,
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
    let reread = result
        .parse::<toml::Table>()
        .map(|t| read_entries(&t))
        .unwrap_or_default();
    if reread.len() != entries.len() || reread[position].as_ref() != Some(updated) {
        return Err(ActionError(
            "the edited entry did not read back as written, so nothing was written; edit this one by hand."
                .into(),
        ));
    }
    std::fs::write(path, result).map_err(|e| ActionError(format!("{}: {e}", path.display())))
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
    let headers: Vec<usize> = (0..lines.len())
        .filter(|&i| table_header(&lines[i]).as_deref() == Some("[[actions]]"))
        .collect();
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
    let reread = result
        .parse::<toml::Table>()
        .map(|t| read_entries(&t))
        .unwrap_or_default();
    let mut expected = entries;
    expected.remove(position);
    if reread != expected {
        return Err(ActionError(
            "the config did not read back as expected, so nothing was written; delete this one by hand."
                .into(),
        ));
    }
    std::fs::write(path, result).map_err(|e| ActionError(format!("{}: {e}", path.display())))
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

    #[test]
    fn matching_is_a_case_insensitive_subsequence() {
        let a = action(
            "Publish to S3",
            "aws s3 cp \"$BJORN_NOTE_FILE\" s3://notes/",
        );
        assert!(a.matches(""));
        assert!(a.matches("s3"));
        assert!(a.matches("PUB"));
        assert!(a.matches("pblsh"), "letters in order, gaps allowed");
        assert!(a.matches("aws"), "the command is searched too");
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
        )
        .await
        .unwrap();
        assert!(!std::path::Path::new(&path).exists(), "{path}");
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
            confirm: true,
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
}
