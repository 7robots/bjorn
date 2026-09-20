//! The daily note: one per day, found by its title, made from the daily
//! template the first time it is asked for. `D` in the app opens it and
//! `bjorn capture` adds lines to it without the TUI.
//!
//! Both go through `bearcli create "<title>" --if-not-exists`, which finds a
//! note by that title before making one. Within one Bjorn process, writes are
//! also serialized, so asking twice never makes a second note. Two processes
//! asking in the same instant (a capture hook while `D` runs, say) rely on
//! Bear doing the check and the create together, which bearcli does not
//! promise; the worst case is a duplicate note, which Bjorn's duplicate-title
//! warning then points out. The template's first line is the title as a
//! heading (`## September 19, 2026 (Saturday)` by default); Bear reads that
//! line as the title and keeps it as it is.

use chrono::{DateTime, Local};

use crate::bear::{BearClient, BearError, Location};
use crate::config::{Config, DEFAULT_DAILY_TEMPLATE};
use crate::templates::{self, DAILY_TEMPLATE, LoadError, Template};

/// Today's note as it would be made: its title, its tag and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daily {
    pub title: String,
    /// The tag path (`log/2026/09/19`); empty for none.
    pub tag: String,
    pub content: String,
}

/// Work out today's note from the `[daily]` config. Fails on a format chrono
/// cannot read, a title or tag holding control characters, or a named
/// template that is missing or cannot be used, with a message fit for a toast
/// or stderr.
pub fn today(config: &Config, now: &DateTime<Local>) -> Result<Daily, String> {
    let daily = &config.daily;
    let title = templates::strftime(now, &daily.title)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("[daily] title {:?} is not a date format", daily.title))?;
    if title.chars().any(char::is_control) {
        return Err(format!(
            "[daily] title {:?} makes {title:?}, which holds a control character (%n, %t?)",
            daily.title
        ));
    }
    let tag = templates::strftime(now, &daily.tag)
        .ok_or_else(|| format!("[daily] tag {:?} is not a date format", daily.tag))?
        .trim()
        .trim_matches('#')
        .to_string();
    if tag.chars().any(char::is_control) {
        return Err(format!(
            "[daily] tag {:?} makes {tag:?}, which holds a control character (%n, %t?)",
            daily.tag
        ));
    }
    let template = match templates::load(&config.templates_dir, &daily.template) {
        Ok(template) => template,
        Err(LoadError::Missing) if daily.template == DEFAULT_DAILY_TEMPLATE => Template {
            name: DEFAULT_DAILY_TEMPLATE.into(),
            body: DAILY_TEMPLATE.into(),
            path: Default::default(),
        },
        Err(LoadError::Missing) => {
            return Err(format!(
                "[daily] template {:?} not found at {}",
                daily.template,
                templates::path_for(&config.templates_dir, &daily.template).display()
            ));
        }
        Err(LoadError::Unusable(why)) => {
            return Err(format!("[daily] template cannot be used: {why}"));
        }
    };
    let tags: Vec<String> = if tag.is_empty() {
        Vec::new()
    } else {
        vec![tag.clone()]
    };
    let content = template.content(now, &title, &config.workspace, &tags);
    Ok(Daily {
        title,
        tag,
        content,
    })
}

/// The id of today's note, made now if Bear has none by that title. A tag the
/// template does not write is passed to bearcli, which puts it where Bear's
/// settings say. A trashed or archived note with today's title is never
/// reused; Bear makes a fresh one, because bearcli's title lookup only
/// searches Notes. The location check below is a defense in case that
/// changes: a match outside Notes is refused, not used, and restoring it is
/// left to the user.
pub async fn ensure(client: &BearClient, daily: &Daily) -> Result<String, BearError> {
    let hashtag = templates::hashtag(&daily.tag);
    let tags = if hashtag.is_empty() || daily.content.contains(&hashtag) {
        Vec::new()
    } else {
        vec![daily.tag.clone()]
    };
    let (id, location) = client
        .create_if_missing(&daily.title, &tags, &daily.content)
        .await?;
    match location {
        Location::Notes => Ok(id),
        Location::Trash => Err(BearError::with_code(
            format!(
                "Today's note “{}” is in the Trash; restore it with u in the Trash view (7).",
                daily.title
            ),
            "not_in_notes",
        )),
        Location::Archive => Err(BearError::with_code(
            format!(
                "Today's note “{}” is in the Archive; restore it with u in the Archive view (6).",
                daily.title
            ),
            "not_in_notes",
        )),
    }
}

/// Is `section` a heading line bearcli can address (`## Inbox`)?
pub fn valid_section(section: &str) -> bool {
    let level = section.len() - section.trim_start_matches('#').len();
    (1..=6).contains(&level)
        && section[level..].starts_with(' ')
        && !section[level..].trim().is_empty()
}

/// Captured text without C0 control characters, tabs and newlines excepted:
/// an escape sequence has no business in a note.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\t' || *c == '\n' || (*c as u32) >= 0x20)
        .collect()
}

/// What one capture adds: `capture_format` with `{{text}}` filled in.
pub fn capture_line(config: &Config, text: &str, now: &DateTime<Local>) -> String {
    templates::render(
        &config.daily.capture_format,
        now,
        &[("text", text), ("workspace", &config.workspace)],
    )
}

/// Add `text` to today's note, making the note (and the capture section) when
/// missing. `text` must not be blank.
pub async fn capture(
    client: &BearClient,
    config: &Config,
    text: &str,
    now: &DateTime<Local>,
) -> Result<(), String> {
    let text = clean(text);
    if text.trim().is_empty() {
        return Err("nothing to capture".into());
    }
    let section = config.daily.capture_section.trim();
    if !section.is_empty() && !valid_section(section) {
        return Err(format!(
            "[daily] capture_section {section:?} is not a heading line like \"## Inbox\""
        ));
    }
    let daily = today(config, now)?;
    let id = ensure(client, &daily).await.map_err(|e| e.message)?;
    let line = capture_line(config, text.trim_end_matches('\n'), now);
    if section.is_empty() {
        return client.append(&id, &line, "").await.map_err(|e| e.message);
    }
    // bearcli refuses a section that is not there, so look first (ignoring
    // case) and start the section at the end of the note when it is missing.
    let note = client.cat(&id).await.map_err(|e| e.message)?;
    let wanted = section.to_lowercase();
    let existing = note
        .content
        .lines()
        .map(str::trim)
        .find(|l| l.to_lowercase() == wanted)
        .map(str::to_string);
    match existing {
        Some(heading) => client.append(&id, &line, &heading).await,
        None => {
            client
                .append(&id, &format!("\n{section}\n{line}"), "")
                .await
        }
    }
    .map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 19, 14, 5, 0).unwrap()
    }

    fn config(dir: &std::path::Path) -> Config {
        Config {
            templates_dir: dir.to_path_buf(),
            ..Config::default()
        }
    }

    #[test]
    fn the_default_daily_note_is_the_log_layout() {
        let dir = tempfile::tempdir().unwrap();
        let daily = today(&config(dir.path()), &at()).unwrap();
        assert_eq!(daily.title, "September 19, 2026 (Saturday)");
        assert_eq!(daily.tag, "log/2026/09/19");
        assert_eq!(
            daily.content,
            "## September 19, 2026 (Saturday)\n#log/2026/09/19\n* People:\n* Topic:\n\n---\n"
        );
    }

    #[test]
    fn a_daily_template_in_the_directory_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("daily.md"),
            "# {{title}}\n{{tag}}\n{{date:%A}}\n",
        )
        .unwrap();
        let mut cfg = config(dir.path());
        cfg.daily.title = "%Y-%m-%d".into();
        cfg.daily.tag = String::new();
        let daily = today(&cfg, &at()).unwrap();
        assert_eq!(daily.content, "# 2026-09-19\nSaturday\n");
        cfg.daily.template = "standup".into();
        let err = today(&cfg, &at()).unwrap_err();
        assert!(err.contains("\"standup\" not found"), "{err}");
    }

    #[test]
    fn bad_formats_are_reported_not_panicked_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.daily.title = "%Q".into();
        assert!(today(&cfg, &at()).unwrap_err().contains("title"));
        cfg.daily.title = "%Y".into();
        cfg.daily.tag = "log/%Q".into();
        assert!(today(&cfg, &at()).unwrap_err().contains("tag"));
    }

    #[test]
    fn control_characters_are_refused_in_titles_and_stripped_from_captures() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.daily.title = "%Y%n%m".into();
        assert!(
            today(&cfg, &at())
                .unwrap_err()
                .contains("control character")
        );
        cfg.daily.title = "%Y".into();
        cfg.daily.tag = "log%t%Y".into();
        assert!(
            today(&cfg, &at())
                .unwrap_err()
                .contains("control character")
        );
        assert_eq!(clean("a\x1b[31mb\tc\r\nd\x07"), "a[31mb\tc\nd");
    }

    #[test]
    fn sections_must_be_heading_lines() {
        for ok in ["## Inbox", "# A", "###### Deep"] {
            assert!(valid_section(ok), "{ok}");
        }
        for bad in ["Inbox", "##Inbox", "## ", "####### Seven", "- ## x"] {
            assert!(!valid_section(bad), "{bad}");
        }
    }

    #[test]
    fn an_unusable_daily_template_is_reported_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("daily.md"), [0xff, 0xfe]).unwrap();
        let err = today(&config(dir.path()), &at()).unwrap_err();
        assert!(
            err.contains("cannot be used") && err.contains("UTF-8"),
            "{err}"
        );
        std::fs::remove_file(dir.path().join("daily.md")).unwrap();
        std::fs::create_dir(dir.path().join("daily.md")).unwrap();
        let err = today(&config(dir.path()), &at()).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn a_relative_template_path_is_inside_the_templates_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/day.md"), "day {{title}}\n").unwrap();
        let mut cfg = config(dir.path());
        cfg.daily.template = "sub/day".into();
        assert!(
            today(&cfg, &at())
                .unwrap()
                .content
                .starts_with("day September")
        );
        cfg.daily.template = "sub/day.md".into();
        assert!(today(&cfg, &at()).is_ok());
    }

    #[test]
    fn capture_lines_follow_the_format() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path());
        assert_eq!(capture_line(&cfg, "call Ana", &at()), "* 14:05 call Ana");
        cfg.daily.capture_format = "- [ ] {{text}}".into();
        assert_eq!(capture_line(&cfg, "call Ana", &at()), "- [ ] call Ana");
    }
}
