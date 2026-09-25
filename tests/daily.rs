//! Work notes: the daily note (`D`), templates (`N`) and `bjorn capture`.
//!
//! Tests that go through the clock use a daily title with no date in it, so a
//! run that crosses midnight still finds the note it made. A config file
//! refuses such a title, so the tests that run the binary use `DATED`.

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use bjorn::config::Config;
use bjorn::daily;
use bjorn::ui::modals::Overlay;
use chrono::{DateTime, Local, TimeZone};
use common::Fake;

const TITLE: &str = "Work log";

fn at() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 9, 19, 14, 5, 0).unwrap()
}

/// The fake's config with a daily title that does not change at midnight.
fn steady(fake: &Fake) -> Config {
    let mut config = fake.config();
    config.daily.title = TITLE.into();
    config.daily.tag = "log".into();
    config
}

/// How many notes in the fake carry `title`, in any location.
async fn count_titled(fake: &Fake, title: &str) -> usize {
    let snap = fake.client().snapshot().await.unwrap();
    snap.notes.iter().filter(|n| n.title == title).count()
}

async fn id_of(fake: &Fake, title: &str) -> String {
    let snap = fake.client().snapshot().await.unwrap();
    snap.notes
        .iter()
        .find(|n| n.title == title)
        .unwrap()
        .id
        .clone()
}

async fn body_of(fake: &Fake, title: &str) -> String {
    let id = id_of(fake, title).await;
    fake.client().cat(&id).await.unwrap().content
}

fn write_template(fake: &Fake, name: &str, body: &str) {
    std::fs::create_dir_all(fake.templates()).unwrap();
    std::fs::write(fake.templates().join(name), body).unwrap();
}

#[tokio::test]
async fn d_creates_todays_note_once_and_shows_it() {
    let fake = Fake::new();
    let mut h = fake.harness_with(steady(&fake), None);
    h.load().await;
    h.press("D");
    h.until(|app| {
        app.notes.current().is_some_and(|n| n.title == TITLE)
            && app.reader.plain_text().contains("People:")
    })
    .await;
    assert!(h.app.overlay.is_none(), "D opens no editor and no prompt");
    assert_eq!(
        body_of(&fake, TITLE).await,
        "## Work log\n#log\n* People:\n* Topic:\n\n---\n"
    );

    // Somewhere else, D comes back to the same note without making another.
    h.press("j");
    h.until(|app| app.notes.current().is_some_and(|n| n.title != TITLE))
        .await;
    h.press("D");
    h.until(|app| app.notes.current().is_some_and(|n| n.title == TITLE))
        .await;
    h.settle().await;
    assert_eq!(count_titled(&fake, TITLE).await, 1);
}

#[tokio::test]
async fn d_reuses_a_note_bear_already_has_and_leaves_the_workspace_for_it() {
    let fake = Fake::new();
    let config = steady(&fake);
    let today = daily::today(&config, &Local::now()).unwrap();
    let id = daily::ensure(&fake.client(), &today).await.unwrap();
    assert_eq!(
        daily::ensure(&fake.client(), &today).await.unwrap(),
        id,
        "--if-not-exists finds it by title"
    );
    let mut h = fake.harness_with(config, Some("#work"));
    h.load().await;
    assert!(!h.app.notes.titles().contains(&TITLE.to_string()));
    h.press("D");
    h.until(|app| app.notes.current().is_some_and(|n| n.id == id))
        .await;
    assert_eq!(h.app.selection.workspace, "", "the note is outside #work");
    h.until(|app| app.reader.plain_text().contains("Topic:"))
        .await;
    assert_eq!(count_titled(&fake, TITLE).await, 1);
}

#[tokio::test]
async fn a_trashed_or_archived_daily_note_is_left_alone_and_a_fresh_one_made() {
    let fake = Fake::new();
    let config = steady(&fake);
    let client = fake.client();
    let today = daily::today(&config, &at()).unwrap();
    let trashed = daily::ensure(&client, &today).await.unwrap();
    client.trash(&trashed).await.unwrap();
    let before = client.cat(&trashed).await.unwrap().content;

    // Like bearcli, a title only finds notes in Notes.
    let fresh = daily::ensure(&client, &today).await.unwrap();
    assert_ne!(fresh, trashed);
    daily::capture(&client, &config, "x", &at()).await.unwrap();
    assert!(client.cat(&fresh).await.unwrap().content.ends_with("x\n"));

    let archived = fresh;
    client.archive(&archived).await.unwrap();
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("D");
    h.until(|app| {
        app.notes
            .current()
            .is_some_and(|n| n.title == TITLE && n.id != trashed && n.id != archived)
    })
    .await;
    h.settle().await;
    let snap = client.snapshot().await.unwrap();
    use bjorn::bear::Location;
    assert_eq!(snap.by_id(&trashed).unwrap().location, Location::Trash);
    assert_eq!(snap.by_id(&archived).unwrap().location, Location::Archive);
    assert_eq!(
        client.cat(&trashed).await.unwrap().content,
        before,
        "the trashed note is untouched"
    );
    assert_eq!(count_titled(&fake, TITLE).await, 3);
    // The fake answers a title lookup outside Notes the way bearcli does.
    let gone = client.create("Gone", &[], "").await.unwrap();
    client.trash(&gone).await.unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_fake-bearcli"))
        .args(["cat", "--format", "json", "--title", "Gone"])
        .env("BJORN_FAKE_BEAR_STATE", fake.state())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("not_found") && stdout.contains("in notes"),
        "{stdout}"
    );
}

/// A bearcli that answers `create` with a note in the Trash: what `ensure`
/// must refuse if bearcli's title lookup ever reaches outside Notes.
struct TrashedCreate;

impl bjorn::bear::Runner for TrashedCreate {
    fn run<'a>(
        &'a self,
        _args: &'a [String],
        _stdin: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<bjorn::bear::RawOutput, bjorn::bear::BearError>>
    {
        Box::pin(async {
            Ok(bjorn::bear::RawOutput {
                status: 0,
                stdout: br#"[{"id": "OLD", "location": "trash"}]"#.to_vec(),
                stderr: String::new(),
            })
        })
    }
    fn describe(&self) -> String {
        "trashed-create".into()
    }
}

#[tokio::test]
async fn ensure_refuses_a_match_outside_notes() {
    let fake = Fake::new();
    let today = daily::today(&steady(&fake), &at()).unwrap();
    let client = bjorn::bear::BearClient::from_runner(Box::new(TrashedCreate));
    let err = daily::ensure(&client, &today).await.unwrap_err();
    assert!(err.message.contains("in the Trash"), "{}", err.message);
}

#[tokio::test]
async fn d_reports_a_bad_format_instead_of_writing() {
    let fake = Fake::new();
    let mut config = fake.config();
    config.daily.title = "%Q".into();
    let mut h = fake.harness_with(config, None);
    h.load().await;
    let before = fake.client().snapshot().await.unwrap().notes.len();
    h.press("D");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("not a date format")),
        "{:?}",
        h.app.toast_messages()
    );
    h.settle().await;
    assert_eq!(fake.client().snapshot().await.unwrap().notes.len(), before);
}

#[tokio::test]
async fn d_reports_an_unusable_daily_template() {
    let fake = Fake::new();
    write_template(&fake, "daily.md", "");
    std::fs::write(fake.templates().join("daily.md"), [0xff, 0xfe]).unwrap();
    let mut h = fake.harness_with(steady(&fake), None);
    h.load().await;
    h.press("D");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("cannot be used")),
        "{:?}",
        h.app.toast_messages()
    );
    h.settle().await;
    assert_eq!(count_titled(&fake, TITLE).await, 0);
}

#[tokio::test]
async fn n_picks_a_template_then_asks_for_title_and_tags() {
    let fake = Fake::new();
    write_template(
        &fake,
        "meeting.md",
        "## Meeting\n{{tag}}\nWith: {{nope}}\n### Notes for {{title}}\n",
    );
    write_template(&fake, "decision.md", "Context\n");
    write_template(&fake, "daily.md", "## {{title}}\n");
    let environ = HashMap::from([("EDITOR".to_string(), "true".to_string())]);
    let mut h = fake.harness_env(environ);
    h.load().await;
    h.press("N");
    let text = h.text();
    assert!(text.contains("New note from a template"), "{text}");
    assert!(text.contains("decision") && text.contains("meeting"));
    assert!(!text.contains("daily"), "the daily template is D's: {text}");
    h.type_text("meet");
    match h.app.overlay.clone() {
        Some(Overlay::Templates { index, .. }) => assert_eq!(index, 0),
        other => panic!("{other:?}"),
    }
    h.press("enter");
    match h.app.overlay.clone() {
        Some(Overlay::NewNote {
            title, template, ..
        }) => {
            assert_eq!(title.value, "Meeting");
            assert_eq!(template.unwrap().name, "meeting");
        }
        other => panic!("{other:?}"),
    }
    assert!(h.text().contains("New note from “meeting”"));
    h.type_text(" with Ana");
    h.press("tab");
    h.type_text("work/meetings");
    h.press("enter");
    h.until(|app| app.notes.titles().contains(&"Meeting with Ana".to_string()))
        .await;
    assert_eq!(
        body_of(&fake, "Meeting with Ana").await,
        "## Meeting with Ana\n#work/meetings\nWith: {{nope}}\n### Notes for Meeting with Ana\n"
    );
}

#[tokio::test]
async fn the_template_picker_wraps_and_ignores_enter_without_a_match() {
    let fake = Fake::new();
    write_template(&fake, "a.md", "A\n");
    write_template(&fake, "b.md", "B\n");
    let mut h = fake.harness();
    h.load().await;
    h.press("N");
    let index = |h: &bjorn::harness::Harness| match &h.app.overlay {
        Some(Overlay::Templates { index, .. }) => *index,
        other => panic!("{other:?}"),
    };
    h.press("up");
    assert_eq!(index(&h), 1, "up from the top wraps to the bottom");
    h.press("down");
    assert_eq!(index(&h), 0, "down from the bottom wraps to the top");
    h.type_text("zzz");
    assert!(h.text().contains("no template matches"));
    h.press("enter");
    assert_eq!(index(&h), 0);
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Templates"));
}

#[tokio::test]
async fn a_front_matter_template_keeps_it_on_top() {
    let fake = Fake::new();
    write_template(&fake, "fm.md", "---\ntype: memo\n---\n## Memo\nbody\n");
    write_template(&fake, "bare.md", "---\ntype: bare\n---\nbody\n");
    let environ = HashMap::from([("EDITOR".to_string(), "true".to_string())]);
    let mut h = fake.harness_env(environ);
    h.load().await;
    h.press("N");
    h.type_text("fm");
    h.press("enter");
    h.type_text(" one");
    h.press("enter");
    h.press("enter");
    h.until(|app| app.notes.titles().contains(&"Memo one".to_string()))
        .await;
    assert_eq!(
        body_of(&fake, "Memo one").await,
        "---\ntype: memo\n---\n## Memo one\nbody\n"
    );
    h.press("N");
    h.type_text("bare");
    h.press("enter");
    h.type_text("Plain");
    h.press("enter");
    h.press("enter");
    h.until(|app| app.notes.titles().contains(&"Plain".to_string()))
        .await;
    assert_eq!(
        body_of(&fake, "Plain").await,
        "---\ntype: bare\n---\n# Plain\n\nbody\n",
        "the title goes after the front matter"
    );
}

#[tokio::test]
async fn n_says_when_no_template_is_usable() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("N");
    assert!(h.text().contains("no templates in"), "{}", h.text());
    h.press("enter");
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Templates"));
    h.press("escape");
    assert!(h.app.overlay.is_none());
    std::fs::create_dir_all(fake.templates()).unwrap();
    std::fs::write(fake.templates().join("bad.md"), [0xff]).unwrap();
    h.press("N");
    assert!(
        h.text().contains("no usable templates (1 skipped"),
        "{}",
        h.text()
    );
}

#[tokio::test]
async fn capture_makes_the_note_then_adds_at_the_end() {
    let fake = Fake::new();
    let config = fake.config();
    let client = fake.client();
    daily::capture(&client, &config, "call Ana\n", &at())
        .await
        .unwrap();
    daily::capture(&client, &config, "send\x1b[2J notes\x07", &at())
        .await
        .unwrap();
    let title = "September 19, 2026 (Saturday)";
    assert_eq!(
        body_of(&fake, title).await,
        "## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n\n---\n\
         * 14:05 call Ana\n* 14:05 send[2J notes\n"
    );
    assert_eq!(count_titled(&fake, title).await, 1);
}

#[tokio::test]
async fn capture_starts_a_missing_section_then_grows_it() {
    let fake = Fake::new();
    let mut config = steady(&fake);
    config.daily.capture_section = "## Inbox".into();
    config.daily.capture_format = "- {{text}}".into();
    let client = fake.client();
    daily::capture(&client, &config, "one", &at())
        .await
        .unwrap();
    // A section already there is grown in place, above whatever follows it.
    let id = id_of(&fake, TITLE).await;
    client.append(&id, "\n## Later\nkeep me", "").await.unwrap();
    daily::capture(&client, &config, "two", &at())
        .await
        .unwrap();
    let body = client.cat(&id).await.unwrap().content;
    assert!(
        body.ends_with("---\n\n## Inbox\n- one\n- two\n\n## Later\nkeep me\n"),
        "{body:?}"
    );
    // The heading is matched ignoring case, so no second section appears.
    config.daily.capture_section = "## INBOX".into();
    daily::capture(&client, &config, "three", &at())
        .await
        .unwrap();
    let body = client.cat(&id).await.unwrap().content;
    assert_eq!(body.matches("## Inbox").count(), 1, "{body:?}");
    assert!(body.contains("- two\n- three\n"), "{body:?}");
}

#[tokio::test]
async fn a_multi_line_capture_stays_one_entry_in_its_section() {
    let fake = Fake::new();
    let mut config = steady(&fake);
    config.daily.capture_section = "## Inbox".into();
    config.daily.capture_format = "- {{text}}".into();
    let client = fake.client();
    daily::capture(&client, &config, "one\r\n## Foo\r\n\r\n---\r\n", &at())
        .await
        .unwrap();
    daily::capture(&client, &config, "two", &at())
        .await
        .unwrap();
    // The pasted heading and rule are text of the first entry, so they neither
    // end the section nor come between it and the next capture.
    let body = body_of(&fake, TITLE).await;
    assert!(
        body.ends_with("---\n\n## Inbox\n- one\n      ## Foo\n      ---\n- two\n"),
        "{body:?}"
    );
    let (_, headings) = bjorn::ui::markdown::render_with_headings(&body);
    let names: Vec<&str> = headings.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(names, ["Work log", "Inbox"]);
}

#[tokio::test]
async fn capture_refuses_blank_text_and_a_section_that_is_not_a_heading() {
    let fake = Fake::new();
    let mut config = steady(&fake);
    let err = daily::capture(&fake.client(), &config, " \n\x1b ", &at())
        .await
        .unwrap_err();
    assert!(err.contains("nothing to capture"));
    config.daily.capture_section = "Inbox".into();
    let err = daily::capture(&fake.client(), &config, "x", &at())
        .await
        .unwrap_err();
    assert!(err.contains("not a heading line"), "{err}");
    assert_eq!(count_titled(&fake, TITLE).await, 0);
    // The fake refuses a plain-text address the way bearcli does.
    let id = fake.client().create("N", &[], "text\n").await.unwrap();
    let err = fake.client().append(&id, "x", "text").await.unwrap_err();
    assert!(err.message.contains("Section not found"), "{}", err.message);
}

/// The daily title the binary runs with. A config file may not hold a title
/// without a date (`check_title_format`), and the binary reads the wall
/// clock, so these tests find their notes by this pattern rather than by one
/// day's title: a run that crosses midnight writes to two notes and still
/// passes.
const DATED: &str = "Log %Y-%m-%d";

/// Today's title under `DATED`, by the wall clock.
fn dated_today() -> String {
    Local::now().format(DATED).to_string()
}

/// The notes the binary made under `DATED`, in the fake's order.
async fn dated_notes(fake: &Fake) -> Vec<String> {
    let snap = fake.client().snapshot().await.unwrap();
    snap.notes
        .iter()
        .filter(|n| chrono::NaiveDate::parse_from_str(&n.title, DATED).is_ok())
        .map(|n| n.id.clone())
        .collect()
}

/// Every body the binary wrote under `DATED`, joined.
async fn dated_bodies(fake: &Fake) -> String {
    let mut out = String::new();
    for id in dated_notes(fake).await {
        out.push_str(&fake.client().cat(&id).await.unwrap().content);
    }
    out
}

/// Run the real binary against the fake. `BJORN_BEARCLI` points at the fake
/// too, so even a run that lost `--demo` never reaches Bear, and the config
/// keeps templates in the temp directory.
fn bjorn(fake: &Fake, args: &[&str], stdin: &[u8]) -> std::process::Output {
    let config = fake.dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[daily]\ntitle = \"{DATED}\"\ntag = \"daily\"\n[templates]\ndir = \"{}\"\n",
            fake.templates().display()
        ),
    )
    .unwrap();
    let args: Vec<String> = args
        .iter()
        .map(|a| a.replace("CONFIG", &config.to_string_lossy()))
        .collect();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bjorn"))
        .args(&args)
        .env("BJORN_FAKE_BEAR_STATE", fake.state())
        .env("BJORN_BEARCLI", env!("CARGO_BIN_EXE_fake-bearcli"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A refused run may exit before reading all of stdin.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    child.wait_with_output().unwrap()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[tokio::test]
async fn the_capture_command_reads_stdin_and_stays_quiet() {
    let fake = Fake::new();
    let started = dated_today();
    let base = ["--demo", "--config", "CONFIG"];
    let run = |extra: &[&str], stdin: &[u8]| {
        let args: Vec<&str> = base.iter().chain(extra).copied().collect();
        bjorn(&fake, &args, stdin)
    };
    let out = run(&["capture"], b"from a pipe\n");
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    let out = run(&["capture", "from", "args"], b"");
    assert!(out.status.success(), "{out:?}");
    let body = dated_bodies(&fake).await;
    assert!(body.contains("from a pipe\n"), "{body:?}");
    assert!(body.contains(" from args\n"), "{body:?}");
    assert!(body.contains("#daily"), "{body:?}");

    let out = run(&["capture"], b"  \n");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("nothing to capture"));

    // Over 1 MiB is refused whole, not cut short.
    let big = vec![b'x'; 1024 * 1024 + 1];
    let out = run(&["capture"], &big);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("over 1 MiB"), "{}", stderr(&out));
    assert!(!dated_bodies(&fake).await.contains("xxxx"));

    let out = run(&["today"], b"");
    assert!(out.status.success(), "{out:?}");
    let line = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let (id, title) = line.split_once('\t').unwrap();
    assert!(
        dated_notes(&fake).await.contains(&id.to_string()),
        "{line:?}"
    );
    // `today` found the note the captures made, unless midnight fell between.
    if dated_today() == started {
        assert_eq!(title, started);
        assert_eq!(dated_notes(&fake).await.len(), 1);
    }
}

#[tokio::test]
async fn flags_after_the_subcommand_are_errors_not_text() {
    let fake = Fake::new();
    // Flags go before the subcommand; after it they are refused, never
    // captured as text and never acted on.
    for args in [
        &["--demo", "--config", "CONFIG", "capture", "try", "--demo"][..],
        &[
            "--demo",
            "--config",
            "CONFIG",
            "capture",
            "--config=/tmp/evil.toml",
        ],
        &["--demo", "--config", "CONFIG", "capture", "-h"],
        &["--demo", "--config", "CONFIG", "capture", "--help"],
        &["--config", "CONFIG", "capture", "--demo", "x"],
        &[
            "--demo", "--config", "CONFIG", "today", "--config", "CONFIG",
        ],
    ] {
        let out = bjorn(&fake, args, b"");
        assert!(!out.status.success(), "{args:?} ran: {out:?}");
        assert!(out.stdout.is_empty(), "{args:?}: {out:?}");
        assert!(
            stderr(&out).contains("unexpected argument"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    assert!(dated_notes(&fake).await.is_empty());

    // After `--`, anything is text.
    let out = bjorn(
        &fake,
        &[
            "--demo",
            "--config",
            "CONFIG",
            "capture",
            "--",
            "--config=x",
            "-h",
        ],
        b"",
    );
    assert!(out.status.success(), "{out:?}");
    assert!(dated_bodies(&fake).await.contains(" --config=x -h\n"));
}

#[tokio::test]
async fn a_pasted_capture_keeps_its_lines_under_the_bullet() {
    let fake = Fake::new();
    let out = bjorn(
        &fake,
        &["--demo", "--config", "CONFIG", "capture"],
        b"first\r\n## Foo\r\n---\r\n\r\nlast\r\n",
    );
    assert!(out.status.success(), "{out:?}");
    let body = dated_bodies(&fake).await;
    assert!(
        body.contains(" first\n      ## Foo\n      ---\n      last\n"),
        "{body:?}"
    );
}

#[tokio::test]
async fn the_binary_refuses_a_daily_title_that_names_no_day() {
    let fake = Fake::new();
    let before = fake.client().snapshot().await.unwrap().notes.len();
    let config = fake.dir.path().join("weekday.toml");
    let path = config.to_string_lossy().into_owned();
    for (title, why) in [
        ("%A", "no year"),
        ("%B %-d", "no year"),
        ("%Y (%A)", "no single day"),
    ] {
        std::fs::write(&config, format!("[daily]\ntitle = \"{title}\"\n")).unwrap();
        for command in [&["capture", "hi"][..], &["today"]] {
            let args: Vec<&str> = ["--demo", "--config", &path]
                .into_iter()
                .chain(command.iter().copied())
                .collect();
            let out = bjorn(&fake, &args, b"");
            assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
            let err = stderr(&out);
            assert!(
                err.contains(&path) && err.contains("[daily] title") && err.contains(why),
                "{args:?}: {err}"
            );
        }
    }
    assert_eq!(fake.client().snapshot().await.unwrap().notes.len(), before);
}

#[tokio::test]
async fn capture_honors_tag_before_the_subcommand_for_workspace() {
    let fake = Fake::new();
    let config = fake.dir.path().join("ws.toml");
    std::fs::write(
        &config,
        format!(
            "[daily]\ntitle = \"{DATED}\"\ncapture_format = \"{{{{workspace}}}}: {{{{text}}}}\"\n\
             [templates]\ndir = \"{}\"\n",
            fake.templates().display()
        ),
    )
    .unwrap();
    let path = config.to_string_lossy().into_owned();
    let out = bjorn(
        &fake,
        &[
            "--demo", "--config", &path, "--tag", "#work", "capture", "hi",
        ],
        b"",
    );
    assert!(out.status.success(), "{out:?}");
    assert!(dated_bodies(&fake).await.contains("work: hi\n"));
}

#[tokio::test]
async fn a_failed_reload_after_d_forgets_the_note_it_was_to_show() {
    let fake = Fake::new();
    let flag = fake.dir.path().join("fail-next-read");
    let client = bjorn::bear::BearClient::with_env(
        vec![env!("CARGO_BIN_EXE_fake-bearcli").to_string()],
        vec![
            (
                "BJORN_FAKE_BEAR_STATE".to_string(),
                fake.state().to_string_lossy().into_owned(),
            ),
            (
                "BJORN_FAKE_BEAR_FAIL_READ".to_string(),
                flag.to_string_lossy().into_owned(),
            ),
        ],
    );
    let mut h =
        bjorn::harness::Harness::new(steady(&fake), std::sync::Arc::new(client), None, (120, 40));
    h.load().await;
    let first = h.app.notes.current().unwrap().id.clone();
    // `create` is not a read, so every listing fails until the flag goes.
    std::fs::write(&flag, "").unwrap();
    h.press("D");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("Injected failure"))
    })
    .await;
    assert_eq!(h.app.notes.current().unwrap().id, first);
    // Reads work again; the next reload shows the new note but stays put.
    std::fs::remove_file(&flag).unwrap();
    h.press("r");
    h.until(|app| app.notes.titles().contains(&TITLE.to_string()))
        .await;
    h.settle().await;
    assert_eq!(h.app.notes.current().unwrap().id, first);
}
