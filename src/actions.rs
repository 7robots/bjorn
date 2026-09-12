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
}
