//! BearClient against the fake bearcli, driven as a real subprocess.

use bjorn::bear::{BearClient, Location};

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn state(&self) -> std::path::PathBuf {
        self.dir.path().join("bear.json")
    }

    fn client(&self) -> BearClient {
        BearClient::with_env(
            vec![env!("CARGO_BIN_EXE_fake-bearcli").to_string()],
            vec![(
                "BJORN_FAKE_BEAR_STATE".to_string(),
                self.state().to_string_lossy().into_owned(),
            )],
        )
    }
}

#[tokio::test]
async fn snapshot_covers_every_location() {
    let fake = Fake::new();
    let snap = fake.client().snapshot().await.unwrap();
    let mut locations: Vec<Location> = snap.notes.iter().map(|n| n.location).collect();
    locations.sort_by_key(|l| l.as_str());
    locations.dedup();
    assert_eq!(
        locations,
        vec![Location::Archive, Location::Notes, Location::Trash]
    );
    let planning = snap.by_id("NOTE-PLANNING").unwrap();
    assert_eq!((planning.todos, planning.done), (3, 1));
    assert!(planning.pinned_globally());
    assert_eq!(
        snap.by_id("NOTE-DESIGN").unwrap().tags,
        vec!["work", "work/CAD and Design"]
    );
    assert_eq!(snap.by_id("NOTE-GARDEN").unwrap().attachments, 1);
    assert!(
        snap.by_id("NOTE-PLANNING")
            .unwrap()
            .preview
            .starts_with("Tasks")
    );
}

#[tokio::test]
async fn probe_changes_after_a_write() {
    let fake = Fake::new();
    let client = fake.client();
    let before = client.probe().await.unwrap();
    assert_eq!(before.count, 7);
    client.pin("NOTE-READING", "global").await.unwrap();
    client
        .create("Fresh", &["home".to_string()], "")
        .await
        .unwrap();
    let after = client.probe().await.unwrap();
    assert_ne!(after, before);
    assert_eq!(after.count, 8);
}

#[tokio::test]
async fn cat_returns_hash_and_conflict_is_typed() {
    let fake = Fake::new();
    let client = fake.client();
    let content = client.cat("NOTE-GARDEN").await.unwrap();
    assert!(content.content.starts_with("# Garden Plan"));
    assert!(!content.hash.is_empty());
    let err = client
        .overwrite(
            "NOTE-GARDEN",
            "# Garden Plan\n#home/garden\n\nx\n",
            "0000000",
        )
        .await
        .unwrap_err();
    assert!(err.is_conflict(), "{err:?}");
    client
        .overwrite(
            "NOTE-GARDEN",
            "# Garden Plan\n#home/garden\n\nx\n",
            &content.hash,
        )
        .await
        .unwrap();
    assert!(
        client
            .cat("NOTE-GARDEN")
            .await
            .unwrap()
            .content
            .ends_with("x\n")
    );
}

#[tokio::test]
async fn search_ids_and_tags() {
    let fake = Fake::new();
    let client = fake.client();
    assert_eq!(
        client.search_ids("@todo", "all").await.unwrap(),
        vec!["NOTE-PLANNING", "NOTE-GARDEN"]
    );
    assert_eq!(
        client.search_ids("@untagged", "all").await.unwrap(),
        vec!["NOTE-UNTAGGED"]
    );
    assert!(client.search_ids("   ", "all").await.unwrap().is_empty());
    assert!(
        client
            .tags()
            .await
            .unwrap()
            .contains(&"work/CAD and Design".to_string())
    );
    let rows = client.todo_rows("").await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].get("content").is_some());
}

#[tokio::test]
async fn create_trash_restore_pin() {
    let fake = Fake::new();
    let client = fake.client();
    let nid = client
        .create("Brand New", &["work/new".to_string()], "body\n")
        .await
        .unwrap();
    let snap = client.snapshot().await.unwrap();
    let note = snap.by_id(&nid).unwrap();
    assert_eq!(note.title, "Brand New");
    assert_eq!(note.tags, vec!["work", "work/new"]);
    client.trash(&nid).await.unwrap();
    assert_eq!(
        client
            .snapshot()
            .await
            .unwrap()
            .by_id(&nid)
            .unwrap()
            .location,
        Location::Trash
    );
    client.restore(&nid).await.unwrap();
    assert_eq!(
        client
            .snapshot()
            .await
            .unwrap()
            .by_id(&nid)
            .unwrap()
            .location,
        Location::Notes
    );
    client.pin(&nid, "global").await.unwrap();
    assert!(
        client
            .snapshot()
            .await
            .unwrap()
            .by_id(&nid)
            .unwrap()
            .pinned_globally()
    );
    client.unpin(&nid, "global").await.unwrap();
    assert!(
        client
            .snapshot()
            .await
            .unwrap()
            .by_id(&nid)
            .unwrap()
            .pins
            .is_empty()
    );
    client.archive(&nid).await.unwrap();
    assert_eq!(
        client
            .snapshot()
            .await
            .unwrap()
            .by_id(&nid)
            .unwrap()
            .location,
        Location::Archive
    );
}

#[tokio::test]
async fn edit_tick_and_open_in_app() {
    let fake = Fake::new();
    let client = fake.client();
    client
        .edit(
            "NOTE-GARDEN",
            "- [ ] move the hydrangea",
            "- [x] move the hydrangea",
            "## Next spring",
        )
        .await
        .unwrap();
    assert!(
        client
            .cat("NOTE-GARDEN")
            .await
            .unwrap()
            .content
            .contains("- [x] move the hydrangea")
    );
    client
        .tick_todo(
            "NOTE-GARDEN",
            "- [ ] order bulbs for the front bed",
            "- [x] order bulbs for the front bed",
            "",
        )
        .await
        .unwrap();
    assert!(
        client
            .cat("NOTE-GARDEN")
            .await
            .unwrap()
            .content
            .contains("- [x] order bulbs for the front bed")
    );
    let gone = client
        .tick_todo(
            "NOTE-GARDEN",
            "- [ ] order bulbs for the front bed",
            "- [x] x",
            "",
        )
        .await
        .unwrap_err();
    assert!(gone.is_conflict());
    client
        .open_in_app("NOTE-GARDEN", "Next spring")
        .await
        .unwrap();
    let log = std::fs::read_to_string(fake.dir.path().join("bear.json.opened")).unwrap();
    let last: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(
        last,
        serde_json::json!({"id": "NOTE-GARDEN", "header": "Next spring"})
    );
}

#[tokio::test]
async fn missing_binary_is_a_bear_error() {
    let err = BearClient::new(vec!["/nonexistent/bearcli".into()])
        .snapshot()
        .await
        .unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn attachments_list_and_save() {
    let fake = Fake::new();
    let client = fake.client();
    assert_eq!(
        client.attachments("NOTE-GARDEN").await.unwrap(),
        vec!["Front bed.png"]
    );
    assert!(
        client
            .attachments("NOTE-PLANNING")
            .await
            .unwrap()
            .is_empty()
    );
    let data = client
        .attachment("NOTE-GARDEN", "Front bed.png")
        .await
        .unwrap();
    assert!(data.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(data.len(), 70);
    assert!(
        client
            .attachment("NOTE-GARDEN", "missing.png")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn read_errors_come_back_typed() {
    let fake = Fake::new();
    let client = fake.client();
    let err = client.cat("NO-SUCH").await.unwrap_err();
    assert_eq!(err.code, "not_found");
    assert_eq!(err.message, "Note not found");
    let err = client.trash("NO-SUCH").await.unwrap_err();
    assert_eq!(err.message, "Error: Note not found"); // the first stderr line, prefix included, as in Python
    assert_eq!(err.exit_code, 1);
}

#[test]
fn fake_remctl_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("reminders.json");
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_fake-remctl"))
            .args(args)
            .env("BJORN_FAKE_REMCTL_STATE", &state)
            .output()
            .unwrap()
    };
    let out = run(&[
        "add",
        "--json",
        "--list",
        "Work",
        "--due",
        "today",
        "--notes",
        "From Bear: T\nbear-todo: abcdef123456",
        "--",
        "- do it",
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["status"], "created");
    assert_eq!(reply["numericId"], 1);
    let out = run(&["search", "bear-todo:", "--completed", "--json"]);
    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rows[0]["title"], "- do it");
    assert_eq!(rows[0]["completed"], false);
    assert!(rows[0]["dueDate"].as_str().unwrap().ends_with("T09:00:00"));
    let out = run(&["done", "1"]);
    assert_eq!(out.status.code(), Some(0));
    let out = run(&["search", "bear-todo:", "--json"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let out = run(&["add", "--json", "--due", "whenever", "--", "x"]);
    assert_eq!(out.status.code(), Some(2));
    let out = run(&["add", "--list", "Nope", "--", "x"]);
    assert_eq!(out.status.code(), Some(1));
}
