//! Dated sections: `s` writing one into a note, and the `T` day screen over
//! the fake bearcli's sample library.

mod common;

use bjorn::app::{Msg, Pane};
use bjorn::bear::NoteContent;
use bjorn::config::Config;
use bjorn::harness::Harness;
use bjorn::model::View;
use bjorn::sections::{DayScan, InsertPosition, SectionsConfig};
use chrono::NaiveDate;
use common::Fake;
use std::sync::Arc;

fn heading_of(config: &SectionsConfig, date: NaiveDate) -> String {
    format!("## {}", config.heading_text(date))
}

/// Put the cursor on a note and wait for the reader to catch up.
async fn open_note(h: &mut Harness, id: &str) {
    assert!(h.app.notes.select_id(id), "{id} is in the list");
    let note = h.app.notes.current().cloned().unwrap();
    h.app.schedule_preview(note, true, false);
    let owned = id.to_string();
    h.until(move |app| app.reader.note.as_ref().is_some_and(|n| n.id == owned))
        .await;
}

async fn body(fake: &Fake, id: &str) -> String {
    fake.client().cat(id).await.unwrap().content
}

async fn open_day(h: &mut Harness) {
    h.press("T");
    h.until(|app| app.day.as_ref().is_some_and(|d| d.loaded))
        .await;
}

fn day_rows(h: &Harness) -> Vec<(String, String)> {
    h.app
        .day
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|i| {
            let row = &h.app.day.as_ref().unwrap().rows[*i];
            (row.note_title.clone(), row.header())
        })
        .collect()
}

#[tokio::test]
async fn s_inserts_todays_section_above_the_first_dated_one() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    // Trail Journal's only section is yesterday's, so today's goes above it.
    open_note(&mut h, "NOTE-TRAIL").await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.starts_with("Added “"))
    })
    .await;

    let config = SectionsConfig::default();
    let content = body(&fake, "NOTE-TRAIL").await;
    let new = content
        .find(&heading_of(&config, fake.today))
        .unwrap_or_else(|| panic!("today's section is missing:\n{content}"));
    let old = content
        .find(&heading_of(&config, fake.days_ago(1)))
        .unwrap();
    assert!(new < old, "the newest section is on top:\n{content}");
    assert!(
        content.contains(&format!(
            "{}\n{}\n* People:\n* Topic:\n\n---",
            heading_of(&config, fake.today),
            config.day_tag_display(fake.today)
        )),
        "the template was written as written:\n{content}"
    );
    assert!(
        content.starts_with("# Trail Journal\n#trail\n"),
        "{content}"
    );
}

#[tokio::test]
async fn s_appends_to_a_note_with_no_dated_sections() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_note(&mut h, "NOTE-GARDEN").await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.starts_with("Added “"))
    })
    .await;
    let content = body(&fake, "NOTE-GARDEN").await;
    let config = SectionsConfig::default();
    assert!(
        content.ends_with(&format!(
            "{}\n{}\n* People:\n* Topic:\n\n---\n",
            heading_of(&config, fake.today),
            config.day_tag_display(fake.today)
        )),
        "it goes at the end when there is nothing dated to go above:\n{content}"
    );
    assert!(
        content.contains("## Next spring"),
        "the note's own sections are untouched:\n{content}"
    );
}

#[tokio::test]
async fn a_second_section_for_today_is_not_written() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    // Field Notes already has a section for today.
    open_note(&mut h, "NOTE-FIELD").await;
    let before = body(&fake, "NOTE-FIELD").await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("already has a section for today"))
    })
    .await;
    h.settle().await;
    assert_eq!(
        before,
        body(&fake, "NOTE-FIELD").await,
        "nothing was written"
    );
    let config = SectionsConfig::default();
    assert_eq!(
        before.matches(&heading_of(&config, fake.today)).count(),
        1,
        "and there is still one section for today"
    );
    // It jumped to the section it found.
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-FIELD")
    })
    .await;
}

#[tokio::test]
async fn the_insert_key_puts_the_section_where_the_config_says() {
    let fake = Fake::new();
    let config = Config {
        sections: SectionsConfig {
            insert: InsertPosition::Top,
            ..SectionsConfig::default()
        },
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    open_note(&mut h, "NOTE-TRAIL").await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.starts_with("Added “"))
    })
    .await;
    let content = body(&fake, "NOTE-TRAIL").await;
    let sections = SectionsConfig::default();
    assert!(
        content.starts_with(&format!(
            "# Trail Journal\n#trail\n\n{}\n",
            heading_of(&sections, fake.today)
        )),
        "top means straight under the title and its tags:\n{content}"
    );
}

#[tokio::test]
async fn t_lists_the_days_sections_grouped_by_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    let config = SectionsConfig::default();
    let heading = config.heading_text(fake.today);
    assert_eq!(
        day_rows(&h),
        vec![
            ("Field Notes".to_string(), heading.clone()),
            ("Ferry Timetable".to_string(), heading.clone()),
        ]
    );
    let day = h.app.day.as_ref().unwrap();
    assert_eq!(day.notes, 2);
    assert!(
        day.status.starts_with("2 sections · 2 notes"),
        "{}",
        day.status
    );
    assert!(day.header().contains(&heading), "{}", day.header());
    assert!(day.header().contains(&config.day_tag_display(fake.today)));
    let screen = h.text();
    assert!(screen.contains("Field Notes"), "{screen}");
    assert!(
        screen.contains("sensor drift at the weir station"),
        "the snippet shows the first lines of the section: {screen}"
    );
    h.press("escape");
    assert!(h.app.day.is_none(), "esc closes the screen");
}

#[tokio::test]
async fn left_and_right_step_days_and_t_returns_to_today() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    h.press("left");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.date == fake.days_ago(1))
    })
    .await;
    assert_eq!(
        day_rows(&h)
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        vec!["Field Notes", "Trail Journal"],
        "Field Notes by its day tag, Trail Journal by its date heading alone"
    );
    // `[` and `]` step too, and `t` comes home.
    h.press("[");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.date == fake.days_ago(2))
    })
    .await;
    assert_eq!(day_rows(&h).len(), 2);
    h.press("]");
    h.until(|app| app.day.as_ref().is_some_and(|d| d.date == fake.days_ago(1)))
        .await;
    h.press("t");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.date == fake.today)
    })
    .await;
    assert_eq!(day_rows(&h).len(), 2);
}

#[tokio::test]
async fn a_day_with_nothing_on_it_says_so() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    for _ in 0..6 {
        h.press("left");
    }
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.date == fake.days_ago(6))
    })
    .await;
    assert!(day_rows(&h).is_empty());
    let screen = h.text();
    assert!(screen.contains("Nothing written on"), "{screen}");
    assert!(
        screen.contains(&SectionsConfig::default().day_tag_display(fake.days_ago(6))),
        "{screen}"
    );
}

#[tokio::test]
async fn the_filter_narrows_the_day() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    h.press("slash");
    h.type_text("ferry");
    h.press("enter");
    assert_eq!(
        day_rows(&h)
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        vec!["Ferry Timetable"]
    );
    assert!(
        h.app
            .day
            .as_ref()
            .unwrap()
            .status
            .contains("filter “ferry”"),
        "{}",
        h.app.day.as_ref().unwrap().status
    );
    // The box opens on the filter in force, so typing extends it; a filter
    // that matches nothing says which day and which filter.
    h.press("slash");
    h.type_text("zzz");
    h.press("enter");
    assert!(day_rows(&h).is_empty());
    assert!(h.text().contains("matches “ferryzzz”"), "{}", h.text());
    // esc clears the filter before it closes the screen.
    h.press("escape");
    assert!(h.app.day.is_some());
    assert!(h.app.day.as_ref().unwrap().filter_text.is_empty());
    assert_eq!(day_rows(&h).len(), 2);
}

#[tokio::test]
async fn enter_opens_the_note_at_that_section() {
    let fake = Fake::new();
    // Short enough that the note does not fit on one page, so landing on the
    // section means scrolling to it.
    let mut h = Harness::new(fake.config(), Arc::new(fake.client()), None, (100, 14));
    h.load().await;
    open_day(&mut h).await;
    h.press("j");
    assert_eq!(
        day_rows(&h)[h.app.day.as_ref().unwrap().cursor].0,
        "Ferry Timetable"
    );
    h.press("enter");
    assert!(h.app.day.is_none(), "the screen closes behind you");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-FERRY")
    })
    .await;
    h.until(|app| app.reader.scroll > 0).await;
    let heading = SectionsConfig::default().heading_text(fake.today);
    assert!(
        h.row(1).contains(&heading),
        "the section's heading is the first line in the reader: {}",
        h.text()
    );
}

#[tokio::test]
async fn b_opens_bear_at_the_section() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    h.press("b");
    let opened = fake.state().with_extension("json.opened");
    h.until(move |_| opened.exists()).await;
    let logged =
        std::fs::read_to_string(fake.state().with_extension("json.opened")).unwrap_or_default();
    let heading = SectionsConfig::default().heading_text(fake.today);
    assert!(logged.contains("NOTE-FIELD"), "{logged}");
    assert!(logged.contains(&heading), "{logged}");
}

#[tokio::test]
async fn another_day_tag_pattern_finds_its_own_sections() {
    let fake = Fake::new();
    let config = Config {
        sections: SectionsConfig {
            day_tag: "journal/%Y-%m-%d".into(),
            // Only the tag can decide: this heading format matches nothing the
            // sample notes write.
            heading_format: "%Y-%m-%d".into(),
            ..SectionsConfig::default()
        },
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    open_day(&mut h).await;
    assert!(day_rows(&h).is_empty(), "nothing journaled today");
    h.press("left");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.date == fake.days_ago(1))
    })
    .await;
    assert_eq!(
        day_rows(&h)
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        vec!["Trail Journal"],
        "the journal tag finds the section the log tag cannot"
    );
    assert!(
        h.app.day.as_ref().unwrap().header().contains("#journal/"),
        "{}",
        h.app.day.as_ref().unwrap().header()
    );
}

#[tokio::test]
async fn a_note_changed_in_bear_between_the_read_and_the_write_is_not_overwritten() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    // Two reads with a write in between: the hash from the first read is what
    // `s` would write against, and by then it is stale.
    let first = fake.client().cat("NOTE-TRAIL").await.unwrap();
    fake.client()
        .overwrite(
            "NOTE-TRAIL",
            &format!("{}\nchanged in Bear\n", first.content.trim_end()),
            &first.hash,
        )
        .await
        .unwrap();
    let after_write = fake.client().cat("NOTE-TRAIL").await.unwrap();
    assert_ne!(first.hash, after_write.hash);

    let note = h.app.snapshot.by_id("NOTE-TRAIL").unwrap().clone();
    h.app.handle_msg(Msg::SectionContent {
        note,
        result: Ok(first.clone()),
    });
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("changed in Bear"))
    })
    .await;
    assert_eq!(
        body(&fake, "NOTE-TRAIL").await,
        after_write.content,
        "the note in Bear is untouched"
    );
}

#[tokio::test]
async fn a_read_with_no_hash_is_never_written_back() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let before = body(&fake, "NOTE-TRAIL").await;
    let note = h.app.snapshot.by_id("NOTE-TRAIL").unwrap().clone();
    // `overwrite --base ""` is an unguarded write, so a read without a hash
    // must stop the write rather than fall back to one.
    h.app.handle_msg(Msg::SectionContent {
        note,
        result: Ok(NoteContent {
            id: "NOTE-TRAIL".into(),
            content: before.clone(),
            hash: String::new(),
        }),
    });
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("no content hash"))
    })
    .await;
    h.settle().await;
    assert_eq!(before, body(&fake, "NOTE-TRAIL").await);
}

#[tokio::test]
async fn s_refuses_a_locked_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake.state()).unwrap()).unwrap();
    state["notes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "NOTE-LOCKED",
            "title": "Sealed",
            "tags": ["survey"],
            "locked": true,
            "location": "notes",
            "created": "2026-08-01T09:00:00Z",
            "modified": "2026-08-01T09:00:00Z",
            "content": "# Sealed\n#survey\n\nnothing to see\n",
        }));
    std::fs::write(fake.state(), serde_json::to_string(&state).unwrap()).unwrap();
    h.press("r");
    h.until(|app| app.snapshot.by_id("NOTE-LOCKED").is_some())
        .await;
    assert!(h.app.notes.select_id("NOTE-LOCKED"));
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("Locked notes cannot take a section"))
    })
    .await;
    h.settle().await;
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake.state()).unwrap()).unwrap();
    let locked = state["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "NOTE-LOCKED")
        .unwrap();
    assert_eq!(
        locked["content"], "# Sealed\n#survey\n\nnothing to see\n",
        "nothing was written into the locked note"
    );
}

/// Put a note with exactly this body into the fake's library and reload.
async fn add_note(
    fake: &Fake,
    h: &mut Harness,
    id: &str,
    title: &str,
    location: &str,
    content: &str,
) {
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake.state()).unwrap()).unwrap();
    state["notes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": id,
            "title": title,
            "tags": ["survey"],
            "location": location,
            "created": "2026-08-01T09:00:00Z",
            "modified": "2026-08-01T09:00:00Z",
            "content": content,
        }));
    std::fs::write(fake.state(), serde_json::to_string(&state).unwrap()).unwrap();
    h.press("r");
    let owned = id.to_string();
    h.until(move |app| app.snapshot.by_id(&owned).is_some())
        .await;
}

#[tokio::test]
async fn s_on_a_daily_note_goes_after_the_whole_title_section() {
    // A daily note's title is its own dated heading. Today's section must not
    // land between that heading and its tag line, or the old day's tag and
    // body would move under today's date.
    let fake = Fake::new();
    let config = SectionsConfig::default();
    let mut h = fake.harness();
    h.load().await;
    let day = fake.days_ago(3);
    let original = format!(
        "{}\n{}\n* People: Ada\n* Topic: the rollout\n\n---\n",
        heading_of(&config, day),
        config.day_tag_display(day)
    );
    let title = config.heading_text(day);
    add_note(&fake, &mut h, "NOTE-DAILY", &title, "notes", &original).await;
    open_note(&mut h, "NOTE-DAILY").await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.starts_with("Added “"))
    })
    .await;
    let content = body(&fake, "NOTE-DAILY").await;
    assert_eq!(
        content,
        format!(
            "{original}\n{}\n{}\n* People:\n* Topic:\n\n---\n",
            heading_of(&config, fake.today),
            config.day_tag_display(fake.today)
        ),
        "the old day stays whole and first, today's follows it"
    );
    let found = bjorn::sections::find_sections(&config, &content);
    assert_eq!(
        found.iter().map(|s| s.date).collect::<Vec<_>>(),
        vec![day, fake.today]
    );
    assert_eq!(found[0].snippet, "People: Ada · Topic: the rollout");
}

#[tokio::test]
async fn s_refuses_a_trashed_note() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let original = "# Discarded\n#survey\n\nold plans\n";
    add_note(&fake, &mut h, "NOTE-BINNED", "Discarded", "trash", original).await;
    h.app.action_view(View::Trash);
    assert!(h.app.notes.select_id("NOTE-BINNED"));
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("Notes in the Trash cannot take a section"))
    })
    .await;
    h.settle().await;
    assert_eq!(
        body(&fake, "NOTE-BINNED").await,
        original,
        "nothing was written into the trashed note"
    );
}

#[tokio::test]
async fn s_works_on_an_empty_note() {
    let fake = Fake::new();
    let config = SectionsConfig::default();
    let empty = fake
        .client()
        .create("Empty", &["survey".to_string()], "")
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_note(&mut h, &empty).await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("to “Empty”"))
    })
    .await;
    let content = body(&fake, &empty).await;
    assert!(
        content.starts_with("# Empty\n"),
        "the title stays first, or Bear renames the note: {content}"
    );
    assert!(
        content.contains(&heading_of(&config, fake.today)),
        "{content}"
    );
    assert_eq!(
        bjorn::sections::find_sections(&config, &content).len(),
        1,
        "and it reads back as exactly one dated section: {content}"
    );
}

#[tokio::test]
async fn s_keeps_a_note_that_ends_without_a_newline() {
    let fake = Fake::new();
    let ragged = fake
        .client()
        .create("Ragged", &["survey".to_string()], "")
        .await
        .unwrap();
    let base = fake.client().cat(&ragged).await.unwrap();
    fake.client()
        .overwrite(&ragged, "# Ragged\n#survey\n\nlast line", &base.hash)
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_note(&mut h, &ragged).await;
    h.press("s");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("to “Ragged”"))
    })
    .await;
    let content = body(&fake, &ragged).await;
    assert!(
        content.ends_with("---"),
        "a note without a final newline does not gain one: {content:?}"
    );
    assert!(
        content.starts_with("# Ragged\n#survey\n\nlast line\n"),
        "{content}"
    );
}

#[tokio::test]
async fn an_older_day_load_never_lands_on_a_newer_day() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    let rows = day_rows(&h);
    assert_eq!(rows.len(), 2);
    // A load from a day the screen has already stepped away from.
    h.app.handle_msg(Msg::DayLoaded {
        generation: 0,
        scan: DayScan::default(),
        error: "from a day ago".into(),
    });
    h.draw();
    assert_eq!(day_rows(&h), rows, "the stale answer is dropped");
    assert!(
        !h.app
            .day
            .as_ref()
            .unwrap()
            .status
            .contains("from a day ago")
    );
}

#[tokio::test]
async fn stepping_crosses_months_and_years() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.app.open_day(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    h.until(|app| app.day.as_ref().is_some_and(|d| d.loaded))
        .await;
    h.press("left");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.date == NaiveDate::from_ymd_opt(2025, 12, 31).unwrap())
    })
    .await;
    assert!(h.app.day.as_ref().unwrap().header().contains("2025"));
    h.press("right");
    h.press("right");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.date == NaiveDate::from_ymd_opt(2026, 1, 2).unwrap())
    })
    .await;
    h.app.open_day(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
    h.until(|app| app.day.as_ref().is_some_and(|d| d.loaded))
        .await;
    h.press("[");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.date == NaiveDate::from_ymd_opt(2026, 2, 28).unwrap())
    })
    .await;
}

#[tokio::test]
async fn a_note_that_only_heads_its_sections_by_date_is_found() {
    let fake = Fake::new();
    let config = SectionsConfig::default();
    fake.client()
        .create(
            "Minutes",
            &["survey".to_string()],
            &format!(
                "# Minutes\n#survey\n\n{}\nNo day tag anywhere in this note.\n",
                heading_of(&config, fake.today)
            ),
        )
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    assert!(
        day_rows(&h).iter().any(|(title, _)| title == "Minutes"),
        "the heading phrase finds it even with no day tag: {:?}",
        day_rows(&h)
    );
    assert!(h.text().contains("No day tag anywhere"), "{}", h.text());
}

#[tokio::test]
async fn an_archived_note_keeps_its_history_and_says_so() {
    let fake = Fake::new();
    fake.client().archive("NOTE-FERRY").await.unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    assert!(
        day_rows(&h)
            .iter()
            .any(|(title, _)| title == "Ferry Timetable"),
        "an archived topic note was still written in that day: {:?}",
        day_rows(&h)
    );
    assert!(
        h.app
            .day
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.archived && r.note_title == "Ferry Timetable")
    );
    assert!(h.text().contains("(archived)"), "{}", h.text());
    // The trash is not history.
    fake.client().trash("NOTE-FIELD").await.unwrap();
    h.press("r");
    h.until(|app| {
        app.day
            .as_ref()
            .is_some_and(|d| d.loaded && d.rows.iter().all(|r| r.note_title != "Field Notes"))
    })
    .await;
}

#[tokio::test]
async fn enter_opens_an_archived_note_at_its_section() {
    let fake = Fake::new();
    fake.client().archive("NOTE-FERRY").await.unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    h.press("j");
    assert_eq!(
        day_rows(&h)[h.app.day.as_ref().unwrap().cursor].0,
        "Ferry Timetable"
    );
    h.press("enter");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-FERRY")
    })
    .await;
    assert_eq!(
        h.app.selection.view,
        View::Archive,
        "the list follows the note into the archive"
    );
    assert!(
        !h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("no longer in the list")),
        "{:?}",
        h.app.toast_messages()
    );
    assert!(h.app.reader.plain_text().contains("two-boat schedule"));
}

#[tokio::test]
async fn enter_on_a_note_that_is_gone_says_so_and_changes_nothing() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    // A tag view, cursor off the first row: a list rebuilt behind the toast
    // would move both the cursor and the reader to another note.
    h.app.set_focus(Pane::Sidebar);
    assert!(h.app.sidebar.move_to_tag("home"));
    h.press("enter");
    h.until(|app| app.selection.tag == "home").await;
    h.app.set_focus(Pane::Notes);
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-READING")
    })
    .await;
    let cursor = h.app.notes.cursor;
    assert_eq!(cursor, Some(1));

    open_day(&mut h).await;
    // A row for a note no snapshot has ever held.
    h.app.day.as_mut().unwrap().rows[0].note_id = "NOTE-VANISHED".into();
    h.press("enter");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("no longer in the list — press r to refresh"))
    })
    .await;
    h.settle().await;
    assert_eq!(
        h.app.reader.note.as_ref().map(|n| n.id.clone()),
        Some("NOTE-READING".to_string()),
        "the reader stayed where it was"
    );
    assert_eq!(h.app.notes.cursor, cursor, "and so did the cursor");
    assert_eq!(h.app.selection.tag, "home", "and the selection");
    assert_eq!(h.app.notes.titles(), vec!["Garden Plan", "Reading Queue"]);
}

#[tokio::test]
async fn b_quotes_a_section_headed_like_a_flag() {
    let fake = Fake::new();
    let config = SectionsConfig::default();
    // `## --version` must reach bearcli as a value; as a bare argument it
    // would be read as the version flag and the note would never open.
    let id = fake
        .client()
        .create("Flagged", &["survey".to_string()], "")
        .await
        .unwrap();
    let base = fake.client().cat(&id).await.unwrap();
    fake.client()
        .overwrite(
            &id,
            &format!(
                "# Flagged\n#survey\n\n## --version\n{}\nA heading that looks like a flag.\n",
                config.day_tag_display(fake.today)
            ),
            &base.hash,
        )
        .await
        .unwrap();

    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    let index = day_rows(&h)
        .iter()
        .position(|(title, heading)| title == "Flagged" && heading == "--version")
        .unwrap_or_else(|| panic!("{:?}", day_rows(&h)));
    for _ in 0..index {
        h.press("j");
    }
    h.press("b");
    let opened = fake.state().with_extension("json.opened");
    h.until(move |_| opened.exists()).await;
    let logged =
        std::fs::read_to_string(fake.state().with_extension("json.opened")).unwrap_or_default();
    assert!(
        logged.contains("\"header\":\"--version\""),
        "the heading arrived as a value: {logged}"
    );
    assert!(
        !h.app.toast_messages().iter().any(|m| m.contains("failed")),
        "{:?}",
        h.app.toast_messages()
    );
}

#[tokio::test]
async fn a_day_tag_that_is_not_a_date_says_so_at_start_up() {
    let fake = Fake::new();
    let config = Config {
        sections: SectionsConfig {
            day_tag: "log/daily".into(),
            ..SectionsConfig::default()
        },
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("day_tag") && m.contains("day screen (T) will stay empty"))
    })
    .await;
    // The sample config says nothing, because there is nothing to say.
    let mut clean = fake.harness();
    clean.load().await;
    assert!(
        !clean
            .app
            .toast_messages()
            .iter()
            .any(|m| m.contains("day_tag")),
        "{:?}",
        clean.app.toast_messages()
    );
}

#[tokio::test]
async fn a_scroll_waiting_on_a_note_that_cannot_be_read_is_dropped() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    // The note goes out from under the day screen between its load and enter.
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake.state()).unwrap()).unwrap();
    state["notes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["id"] != "NOTE-FIELD");
    std::fs::write(fake.state(), serde_json::to_string(&state).unwrap()).unwrap();
    // The body is in the reader's cache from the first snapshot; without
    // dropping it nothing would be read at all.
    h.app.forget_content(None);
    h.press("enter");
    h.until(|app| {
        app.reader
            .message()
            .is_some_and(|m| m.contains("Could not read note"))
    })
    .await;
    assert!(
        h.app.pending_scroll.is_none(),
        "a scroll that will never happen must not fire on the next note"
    );
}

#[tokio::test]
async fn the_filter_box_says_what_esc_does() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    open_day(&mut h).await;
    h.press("slash");
    assert!(h.text().contains("esc to cancel"), "{}", h.text());
    // And that is what it does: the box closes, the list is untouched.
    h.type_text("ferry");
    h.press("escape");
    assert!(h.app.day.as_ref().unwrap().filter.is_none());
    assert!(h.app.day.as_ref().unwrap().filter_text.is_empty());
    assert_eq!(day_rows(&h).len(), 2);
}

#[tokio::test]
async fn a_noisy_note_title_never_reaches_a_toast_as_written() {
    let fake = Fake::new();
    let id = fake
        .client()
        .create("Bell\u{7}Note", &["survey".to_string()], "")
        .await
        .unwrap();
    let mut h = fake.harness();
    h.load().await;
    open_note(&mut h, &id).await;
    h.press("s");
    h.until(|app| app.toast_messages().iter().any(|m| m.contains("Added “")))
        .await;
    let toasts = h.app.toast_messages();
    assert!(
        toasts.iter().any(|m| m.contains("to “BellNote”")),
        "{toasts:?}"
    );
    assert!(
        toasts.iter().all(|m| !m.chars().any(char::is_control)),
        "{toasts:?}"
    );
}

#[tokio::test]
async fn enter_leaves_a_workspace_the_note_is_outside_of() {
    let fake = Fake::new();
    // The day screen searches every note, so a workspace that does not hold
    // the note is the common case, not the odd one.
    let mut h = fake.harness_with(fake.config(), Some("home"));
    h.load().await;
    assert_eq!(h.app.selection.workspace, "home");
    open_day(&mut h).await;
    assert!(
        day_rows(&h).iter().any(|(title, _)| title == "Field Notes"),
        "the day screen ignores the workspace: {:?}",
        day_rows(&h)
    );
    h.press("enter");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-FIELD")
    })
    .await;
    assert!(
        h.app.selection.workspace.is_empty(),
        "the workspace is left behind, or the note could not be listed at all"
    );
    assert_eq!(h.app.sidebar.header(), "BJORN");
    assert!(
        h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("Left the workspace #home")),
        "{:?}",
        h.app.toast_messages()
    );
    assert!(
        !h.app
            .toast_messages()
            .iter()
            .any(|m| m.contains("no longer in the list")),
        "{:?}",
        h.app.toast_messages()
    );
}

#[tokio::test]
async fn a_scroll_waiting_on_a_locked_note_is_dropped() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake.state()).unwrap()).unwrap();
    state["notes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "NOTE-SEALED",
            "title": "Sealed",
            "tags": ["survey"],
            "locked": true,
            "location": "notes",
            "created": "2026-08-01T09:00:00Z",
            "modified": "2026-08-01T09:00:00Z",
            "content": "# Sealed\n#survey\n\nnothing to see\n",
        }));
    std::fs::write(fake.state(), serde_json::to_string(&state).unwrap()).unwrap();
    h.press("r");
    h.until(|app| app.snapshot.by_id("NOTE-SEALED").is_some())
        .await;

    // A locked note is never read, so the scroll waiting on it would sit there
    // and fire on whichever note is drawn next.
    h.app.pending_scroll = Some(("NOTE-SEALED".to_string(), "## Anything".to_string()));
    let note = h.app.snapshot.by_id("NOTE-SEALED").unwrap().clone();
    h.app.schedule_preview(note, true, true);
    h.settle().await;
    assert!(
        h.app.reader.message().is_some_and(|m| m.contains("locked")),
        "{:?}",
        h.app.reader.message()
    );
    assert!(h.app.pending_scroll.is_none());
}
