//! A local stand-in for remctl.
//!
//! `search --json` prints a JSON array of reminders on stdout, `add --json`
//! prints a compact status object, `done <id>` completes one, errors are plain
//! text on stderr with exit 1. State lives at $BJORN_FAKE_REMCTL_STATE (default
//! ~/.cache/bjorn/demo-reminders.json).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::home_dir;

pub fn state_path() -> PathBuf {
    match std::env::var_os("BJORN_FAKE_REMCTL_STATE").filter(|v| !v.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => home_dir().join(".cache/bjorn/demo-reminders.json"),
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Reminder {
    id: i64,
    title: String,
    list: String,
    completed: bool,
    notes: String,
    #[serde(rename = "dueDate")]
    due_date: String,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct State {
    lists: Vec<String>,
    reminders: Vec<Reminder>,
    next_id: i64,
}

impl Default for State {
    fn default() -> Self {
        State {
            lists: vec!["Reminders".into(), "Work".into()],
            reminders: Vec::new(),
            next_id: 1,
        }
    }
}

fn load_state() -> State {
    let path = state_path();
    if path.exists() {
        let text = std::fs::read_to_string(&path).expect("readable state file");
        return serde_json::from_str(&text).expect("valid state file");
    }
    let state = State::default();
    save_state(&state);
    state
}

fn save_state(state: &State) {
    let text = serde_json::to_string_pretty(state).expect("serializable state");
    super::write_atomic(&state_path(), &text).expect("writable state file");
}

fn serialize(row: &Reminder) -> Value {
    let mut out = json!({
        "id": row.id, "title": row.title, "list": row.list, "completed": row.completed,
        "flagged": false, "urgent": false, "priority": "none", "subtaskCount": 0, "isSubtask": false,
    });
    if !row.notes.is_empty() {
        out["notes"] = Value::String(row.notes.clone());
    }
    if !row.due_date.is_empty() {
        out["dueDate"] = Value::String(row.due_date.clone());
    }
    out
}

fn fail(message: &str, code: i32) -> i32 {
    eprintln!("Error: {message}");
    code
}

#[derive(Parser)]
#[command(name = "remctl", disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Search {
        #[arg(allow_hyphen_values = true)]
        query: String,
        #[arg(long)]
        completed: bool,
        #[arg(long)]
        json: bool,
    },
    Add {
        #[arg(allow_hyphen_values = true)]
        title: String,
        #[arg(short = 'l', long = "list")]
        list_name: Option<String>,
        #[arg(short = 'n', long, default_value = "", allow_hyphen_values = true)]
        notes: String,
        #[arg(short = 'd', long)]
        due: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Done {
        id: i64,
    },
    Lists {
        #[arg(long)]
        json: bool,
    },
}

/// Run the fake with `argv` (program name first) and return the exit code.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(err) => {
            let code = if err.use_stderr() { 2 } else { 0 };
            let _ = err.print();
            return code;
        }
    };
    let mut state = load_state();
    match cli.command {
        Command::Search {
            query, completed, ..
        } => {
            let needle = query.to_lowercase();
            let hits: Vec<Value> = state
                .reminders
                .iter()
                .filter(|r| completed || !r.completed)
                .filter(|r| {
                    r.title.to_lowercase().contains(&needle)
                        || r.notes.to_lowercase().contains(&needle)
                })
                .map(serialize)
                .collect();
            println!("{}", Value::Array(hits));
            0
        }
        Command::Add {
            title,
            list_name,
            notes,
            due,
            ..
        } => {
            let list_name = list_name.unwrap_or_else(|| state.lists[0].clone());
            if !state.lists.contains(&list_name) {
                return fail(&format!("list '{list_name}' not found"), 1);
            }
            if let Some(due) = due.as_deref()
                && due != "today"
                && due != "tomorrow"
                && !(due.len() >= 4 && due[..4].chars().all(|c| c.is_ascii_digit()))
            {
                println!(
                    "{}",
                    json!({"status": "error", "message": format!("invalid due date '{due}'")})
                );
                return 2;
            }
            let row = Reminder {
                id: state.next_id,
                title,
                list: list_name,
                completed: false,
                notes,
                due_date: if due.is_some() {
                    chrono::Local::now().format("%Y-%m-%dT09:00:00").to_string()
                } else {
                    String::new()
                },
            };
            state.next_id += 1;
            let reply = json!({"status": "created", "id": format!("FAKE-CK-{}", row.id), "title": row.title, "numericId": row.id});
            state.reminders.push(row);
            save_state(&state);
            println!("{reply}");
            0
        }
        Command::Done { id } => {
            if let Some(row) = state.reminders.iter_mut().find(|r| r.id == id) {
                row.completed = true;
                save_state(&state);
                println!("{}", json!({"status": "completed", "numericId": id}));
                0
            } else {
                fail(&format!("#{id} not found"), 1)
            }
        }
        Command::Lists { .. } => {
            let rows: Vec<Value> = state
                .lists
                .iter()
                .enumerate()
                .map(|(i, t)| json!({"id": i + 1, "title": t}))
                .collect();
            println!("{}", Value::Array(rows));
            0
        }
    }
}
