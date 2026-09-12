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
use std::path::PathBuf;
use std::time::Duration;

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
}
