//! Apple Reminders through `remctl`, and the link between a todo and its reminder.
//!
//! The link lives entirely on the Reminders side, exactly as remtui writes it:
//! a reminder created from a todo carries, in its notes, the note's `bear://`
//! link and a `bear-todo: <key>` line. Nothing is written into Bear on add. On
//! every triage load the reminders carrying a key are read back and joined to
//! the current todos on that key.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::todos::Todo;
use crate::util::{first_line, which};

pub const ENV_COMMAND: &str = "BJORN_REMCTL";
pub const DEFAULT_COMMAND: &str = "remctl";
pub const KEY_PREFIX: &str = "bear-todo:";
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

static KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^bear-todo:\s*([0-9a-f]{6,40})\s*$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RemctlError {
    pub message: String,
    pub exit_code: i32,
}

impl RemctlError {
    pub fn new(message: impl Into<String>) -> RemctlError {
        RemctlError {
            message: message.into(),
            exit_code: 1,
        }
    }
}

pub fn resolve_remctl(configured: &str) -> String {
    std::env::var(ENV_COMMAND)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            if configured.trim().is_empty() {
                DEFAULT_COMMAND.to_string()
            } else {
                configured.trim().to_string()
            }
        })
}

pub fn remctl_found(configured: &str) -> bool {
    let command = resolve_remctl(configured);
    which(&command).is_some() || std::path::Path::new(&command).is_file()
}

pub fn note_url(note_id: &str) -> String {
    format!("bear://x-callback-url/open-note?id={note_id}")
}

/// The notes block written into a reminder created from `todo`: a line for a
/// human, a clickable link, and the machine key `link_key` reads back.
pub fn link_notes(todo: &Todo) -> String {
    let first = if todo.note_title.is_empty() {
        "From Bear".to_string()
    } else {
        format!("From Bear: {}", todo.note_title)
    };
    format!(
        "{first}\n{}\n{KEY_PREFIX} {}",
        note_url(&todo.note_id),
        todo.key()
    )
}

pub fn link_key(notes: &str) -> String {
    KEY_RE
        .captures(notes)
        .map(|c| c[1].to_string())
        .unwrap_or_default()
}

/// The little we need of a reminder remctl serialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedReminder {
    pub id: i64,
    pub title: String,
    pub completed: bool,
    pub key: String,
    pub list_name: String,
}

impl LinkedReminder {
    pub fn from_json(data: &Value) -> Option<LinkedReminder> {
        let key = link_key(data.get("notes").and_then(Value::as_str).unwrap_or(""));
        if key.is_empty() {
            return None;
        }
        let id = match data.get("id") {
            Some(Value::Number(n)) => n.as_i64()?,
            Some(Value::String(s)) => s.trim().parse().ok()?,
            _ => return None,
        };
        Some(LinkedReminder {
            id,
            title: data
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            completed: data
                .get("completed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            key,
            list_name: data
                .get("list")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// What became of a todo in Reminders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    New,
    Added,
    Done,
}

/// todo key -> (status, reminder id) for todos that have a reminder.
///
/// When several reminders share a key (the same todo added twice, say), an
/// active one wins over a completed one so the row does not read as finished
/// while work is still open.
pub fn join(todos: &[Todo], reminders: &[LinkedReminder]) -> HashMap<String, (Status, i64)> {
    let mut by_key: HashMap<&str, &LinkedReminder> = HashMap::new();
    for reminder in reminders {
        let takes_the_key = match by_key.get(reminder.key.as_str()) {
            None => true,
            // An active reminder displaces a completed one under the same key.
            Some(current) => current.completed && !reminder.completed,
        };
        if takes_the_key {
            by_key.insert(&reminder.key, reminder);
        }
    }
    let mut out = HashMap::new();
    for todo in todos {
        let key = todo.key();
        if let Some(reminder) = by_key.get(key.as_str()) {
            out.insert(
                key,
                (
                    if reminder.completed {
                        Status::Done
                    } else {
                        Status::Added
                    },
                    reminder.id,
                ),
            );
        }
    }
    out
}

/// Shells out to remctl. Reads parse `--json`; adds return the created id.
pub struct RemctlClient {
    command: Vec<String>,
    envs: Vec<(String, String)>,
    write_lock: tokio::sync::Mutex<()>,
}

impl RemctlClient {
    pub fn new(command: Vec<String>) -> RemctlClient {
        RemctlClient {
            command,
            envs: Vec::new(),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_env(command: Vec<String>, envs: Vec<(String, String)>) -> RemctlClient {
        RemctlClient {
            command,
            envs,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Run remctl once and return its stdout; any failure is a `RemctlError`.
    pub async fn run(&self, args: &[&str]) -> Result<String, RemctlError> {
        let (program, prefix) = self
            .command
            .split_first()
            .ok_or_else(|| RemctlError::new("no remctl command"))?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(prefix)
            .args(args)
            .envs(self.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                RemctlError::new(format!("remctl not found ({program})"))
            } else {
                RemctlError::new(format!("could not run {program}: {err}"))
            }
        })?;
        let output = match tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return Err(RemctlError::new(format!("remctl failed to run: {err}"))),
            Err(_) => {
                return Err(RemctlError::new(format!(
                    "remctl did not answer within {} s",
                    COMMAND_TIMEOUT.as_secs()
                )));
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut message = first_line(&stderr);
            if message.is_empty() {
                message = stdout.trim().to_string();
            }
            if let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(&stdout)
                && let Some(m) = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
            {
                message = m.to_string();
            }
            return Err(RemctlError {
                message: if message.is_empty() {
                    "remctl failed".into()
                } else {
                    message
                },
                exit_code: output.status.code().unwrap_or(1),
            });
        }
        Ok(stdout)
    }

    /// Every reminder, active or completed, created from a Bear todo.
    pub async fn linked_reminders(&self) -> Result<Vec<LinkedReminder>, RemctlError> {
        let out = self
            .run(&["search", KEY_PREFIX, "--completed", "--json"])
            .await?;
        if out.trim().is_empty() {
            return Ok(Vec::new());
        }
        let payload: Value = serde_json::from_str(&out)
            .map_err(|e| RemctlError::new(format!("remctl returned invalid JSON: {e}")))?;
        let rows = match payload {
            Value::Array(rows) => rows,
            Value::Object(map) => map
                .get("reminders")
                .or_else(|| map.get("results"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(rows
            .iter()
            .filter(|r| r.is_object())
            .filter_map(LinkedReminder::from_json)
            .collect())
    }

    /// Create the reminder for a todo; returns remctl's numeric id when given.
    pub async fn add(
        &self,
        todo: &Todo,
        list_title: &str,
        due: &str,
    ) -> Result<Option<i64>, RemctlError> {
        let notes = link_notes(todo);
        let mut args: Vec<&str> = vec!["add", "--json"];
        if !list_title.is_empty() {
            args.extend(["--list", list_title]);
        }
        if !due.is_empty() {
            args.extend(["--due", due]);
        }
        args.extend(["--notes", &notes, "--", &todo.text]);
        let out = {
            let _guard = self.write_lock.lock().await;
            self.run(&args).await?
        };
        if let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(&out) {
            if payload.get("status").and_then(Value::as_str) == Some("error") {
                let message = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("remctl add failed");
                return Err(RemctlError::new(message));
            }
            let rid = payload.get("numericId").or_else(|| payload.get("id"));
            if let Some(Value::Number(n)) = rid {
                return Ok(n.as_i64());
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t1() -> Todo {
        Todo::new(
            "N1",
            "Sprint",
            &["work"],
            "write the notes",
            "- [ ] write the notes",
            "## Tasks",
        )
    }

    fn t2() -> Todo {
        Todo::new(
            "N1",
            "Sprint",
            &["work"],
            "ship it",
            "- [ ] ship it",
            "## Tasks",
        )
    }

    #[test]
    fn link_notes_and_key_round_trip() {
        let notes = link_notes(&t1());
        assert_eq!(
            notes.lines().collect::<Vec<_>>(),
            vec![
                "From Bear: Sprint",
                &note_url("N1"),
                &format!("{KEY_PREFIX} {}", t1().key())
            ]
        );
        assert_eq!(link_key(&notes), t1().key());
        assert_eq!(link_key("no key here"), "");
        assert_eq!(link_key("bear-todo: 9afbe7c1dc95"), "9afbe7c1dc95");
    }

    #[test]
    fn join_prefers_an_active_duplicate() {
        let key = t1().key();
        let active = LinkedReminder {
            id: 1,
            title: "write the notes".into(),
            completed: false,
            key: key.clone(),
            list_name: String::new(),
        };
        let done = LinkedReminder {
            id: 2,
            title: "write the notes".into(),
            completed: true,
            key: key.clone(),
            list_name: String::new(),
        };
        let todos = vec![t1(), t2()];
        assert_eq!(
            join(&todos, std::slice::from_ref(&done)),
            HashMap::from([(key.clone(), (Status::Done, 2))])
        );
        assert_eq!(
            join(&todos, &[done.clone(), active.clone()]),
            HashMap::from([(key.clone(), (Status::Added, 1))])
        );
        assert_eq!(
            join(&todos, &[active.clone(), done]),
            HashMap::from([(key, (Status::Added, 1))])
        );
        assert!(join(&[t2()], &[active]).is_empty());
    }

    #[test]
    fn from_json_ignores_unlinked_rows() {
        assert!(
            LinkedReminder::from_json(
                &json!({"id": 5, "title": "x", "completed": false, "notes": "plain"})
            )
            .is_none()
        );
        let r = LinkedReminder::from_json(&json!({"id": "7", "title": "x", "completed": true, "list": "Work", "notes": link_notes(&t1())})).unwrap();
        assert_eq!(
            r,
            LinkedReminder {
                id: 7,
                title: "x".into(),
                completed: true,
                key: t1().key(),
                list_name: "Work".into()
            }
        );
    }

    #[tokio::test]
    async fn missing_remctl_is_a_remctl_error() {
        let err = RemctlClient::new(vec!["/nonexistent/remctl".into()])
            .linked_reminders()
            .await
            .unwrap_err();
        assert!(err.message.contains("not found"));
    }
}
