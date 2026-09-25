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
//!
//! All of it is off until the config has a `[daily]` table: Bear has no daily
//! notes of its own, so Bjorn does not make any unless asked to.

use chrono::{DateTime, Local};

use crate::bear::{BearClient, BearError, Location, normalize_tag};
use crate::config::{Config, DEFAULT_DAILY_TEMPLATE, DailyConfig};
use crate::render;
use crate::templates::{self, DAILY_TEMPLATE, LoadError, Template};

/// What `D`, `bjorn capture` and `bjorn today` say when the config has no
/// `[daily]` table.
pub const DAILY_OFF: &str = "Daily notes are off — add a [daily] section to config.toml";

/// The `[daily]` settings, or `DAILY_OFF` when daily notes are not turned on.
pub fn settings(config: &Config) -> Result<&DailyConfig, String> {
    config.daily.as_ref().ok_or_else(|| DAILY_OFF.to_string())
}

/// Today's note as it would be made: its title, its tag and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daily {
    pub title: String,
    /// The tag path (`log/2026/09/19`); empty for none.
    pub tag: String,
    pub content: String,
}

/// Work out today's note from the `[daily]` config. Fails without one
/// (`DAILY_OFF`), on a format chrono cannot read, a title or tag holding
/// control characters, or a named template that is missing or cannot be
/// used, with a message fit for a toast or stderr. Whether the title names
/// one day is checked when the config is loaded (`check_title_format`), not
/// here.
pub fn today(config: &Config, now: &DateTime<Local>) -> Result<Daily, String> {
    let daily = settings(config)?;
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

/// Refuse a `[daily] title` format that does not name exactly one day.
/// Today's note is found by its title alone, so a title that repeats (`%A`,
/// `%B %-d`) would quietly reuse last week's or last year's note, and one
/// that changes during the day (`%H`) would make a new note every hour.
///
/// The format is read as chrono's items, not searched as text, so `%%d` is a
/// literal and `%F`, `%D`, `%x` and `%v` count for the fields they expand to.
/// A day is named by one of three sets of fields:
///
/// - a year, a month and a day of the month (`%Y-%m-%d`, `%B %-d, %Y`);
/// - a year and a day of the year (`%Y-%j`), or a year, a week (`%U`, `%W`)
///   and a weekday;
/// - an ISO week-based year, an ISO week and a weekday (`%G-W%V-%u`).
///
/// The years do not mix: `%G` is not the calendar year in the days around New
/// Year, so `%G-%m-%d` and `%Y-W%V-%u` each give two days the same title. A
/// two-digit year (`%y`, `%g`) is accepted; it repeats only after a century.
/// Any time-of-day or time-zone field is refused, `%c` and `%+` included, and
/// so is a newline or a tab (`%n`, `%t`), which no one-line title can carry.
pub fn check_title_format(format: &str) -> Result<(), String> {
    use chrono::format::{Fixed, Item, Numeric, StrftimeItems};

    let mut year = false;
    let mut iso_year = false;
    let mut month = false;
    let mut day = false;
    let mut ordinal = false;
    let mut week = false;
    let mut iso_week = false;
    let mut weekday = false;
    let mut not_a_date = false;
    for item in StrftimeItems::new(format) {
        match item {
            Item::Error => {
                return Err(format!(
                    "[daily] title {format:?} is not a date format chrono can read"
                ));
            }
            // `%n` and `%t` are spaces to chrono, but a newline or a tab has
            // no place in a title: the title would never match the note.
            Item::Literal(text) | Item::Space(text) if text.chars().any(char::is_control) => {
                return Err(control_in_title(format));
            }
            Item::OwnedLiteral(ref text) | Item::OwnedSpace(ref text)
                if text.chars().any(char::is_control) =>
            {
                return Err(control_in_title(format));
            }
            Item::Literal(_) | Item::OwnedLiteral(_) | Item::Space(_) | Item::OwnedSpace(_) => {}
            Item::Numeric(numeric, _) => match numeric {
                Numeric::Year | Numeric::YearMod100 => year = true,
                Numeric::IsoYear | Numeric::IsoYearMod100 => iso_year = true,
                // A century alone names no year; with `%y` the `%y` does.
                Numeric::YearDiv100 | Numeric::IsoYearDiv100 | Numeric::Quarter => {}
                Numeric::Month => month = true,
                Numeric::Day => day = true,
                Numeric::Ordinal => ordinal = true,
                Numeric::WeekFromSun | Numeric::WeekFromMon => week = true,
                Numeric::IsoWeek => iso_week = true,
                Numeric::NumDaysFromSun | Numeric::WeekdayFromMon => weekday = true,
                _ => not_a_date = true,
            },
            Item::Fixed(fixed) => match fixed {
                Fixed::ShortMonthName | Fixed::LongMonthName => month = true,
                Fixed::ShortWeekdayName | Fixed::LongWeekdayName => weekday = true,
                _ => not_a_date = true,
            },
        }
    }
    let why = if not_a_date {
        "holds a time of day or a time zone, so it can change during the day \
         and a capture could make a new note"
    } else if (year && ((month && day) || ordinal || (week && weekday)))
        || (iso_year && iso_week && weekday)
    {
        return Ok(());
    } else if !year && !iso_year {
        "has no year, so it repeats and would bring back an older note"
    } else if iso_year && !year {
        "has an ISO week-based year (%G, %g), which names a day only with an ISO \
         week (%V) and a weekday; around New Year it is not the calendar year"
    } else if iso_week && !iso_year {
        "has an ISO week (%V) but the calendar year; around New Year they \
         disagree, so use %G with %V"
    } else {
        "names no single day of the year (that takes a month and a day of the \
         month, a day of the year, or a week and a weekday)"
    };
    Err(format!(
        "[daily] title {format:?} does not name one day: it {why}. Today's note is \
         found by its title, so the title must change every day and only then. \
         Use a year, month and day (%Y-%m-%d or %B %-d, %Y), a year and day of \
         the year (%Y-%j), or an ISO week date (%G-W%V-%u)"
    ))
}

fn control_in_title(format: &str) -> String {
    format!(
        "[daily] title {format:?} holds a control character (%n, %t or a typed one); \
         a title is one line of text"
    )
}

/// The id of today's note, made now if Bear has none by that title. A tag the
/// template does not write is passed to bearcli, which puts it where Bear's
/// settings say. A trashed or archived note with today's title is never
/// reused; Bear makes a fresh one, because bearcli's title lookup only
/// searches Notes. The location check below is a defense in case that
/// changes: a match outside Notes is refused, not used, and restoring it is
/// left to the user.
pub async fn ensure(client: &BearClient, daily: &Daily) -> Result<String, BearError> {
    let tags = if normalize_tag(&daily.tag).is_empty() || writes_tag(&daily.content, &daily.tag) {
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

/// Does `content` carry `tag` as a whole tag? Each tag is read the way the
/// reader reads one (`#nested/tag`, `#two words#`) and compared entire, so a
/// longer tag that starts with the same text (`#log/2026/09/25` for
/// `log/2026/09/2`) does not count. A fenced code block holds no tags.
fn writes_tag(content: &str, tag: &str) -> bool {
    let wanted = normalize_tag(tag);
    let mut fenced = false;
    content.lines().any(|line| {
        if render::is_fence(line) {
            fenced = !fenced;
            return false;
        }
        !fenced
            && render::tags_in_line(line)
                .iter()
                .any(|found| normalize_tag(found) == wanted)
    })
}

/// Is `section` a heading line bearcli can address (`## Inbox`)?
pub fn valid_section(section: &str) -> bool {
    let level = section.len() - section.trim_start_matches('#').len();
    (1..=6).contains(&level)
        && section[level..].starts_with(' ')
        && !section[level..].trim().is_empty()
}

/// Captured text without control characters (C0, DEL and C1), tabs and
/// newlines excepted: an escape sequence has no business in a note, and a C1
/// CSI (U+009B) is one a terminal honors as surely as `ESC [`. A carriage
/// return is a line break, whether alone or before `\n`, so a CRLF or
/// old-Mac paste keeps its lines instead of running them together.
pub fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\t' || *c == '\n' || !c.is_control())
        .collect()
}

/// What one capture adds: `format` (`[daily] capture_format`) with
/// `{{text}}` filled in, and `{{workspace}}` with `workspace`.
///
/// A multi-line capture (`pbpaste | bjorn capture`) stays one entry. Its
/// blank lines are dropped and every line after the first is indented four
/// spaces past the content column of the line `{{text}}` sits on (six for
/// `* {{time}} {{text}}`). Indenting only to the content column would keep a
/// line inside the list item but still let it start a block there: a pasted
/// `## Foo` would be a heading and `---` a rule or a setext underline. Four
/// more spaces is the indentation no heading, rule, fence, quote or list
/// marker allows, and an indented code block cannot interrupt a paragraph,
/// so each such line is only more text of the entry's paragraph. A blank
/// line would end that paragraph and make what follows a code block, which
/// is why blank lines go.
pub fn capture_line(format: &str, workspace: &str, text: &str, now: &DateTime<Local>) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let text = match lines.as_slice() {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first, rest @ ..] => {
            let pad = " ".repeat(content_column(&text_prefix(format, workspace, now)) + 4);
            let mut joined = (*first).to_string();
            for line in rest {
                joined.push('\n');
                joined.push_str(&pad);
                joined.push_str(line);
            }
            joined
        }
    };
    templates::render(format, now, &[("text", &text), ("workspace", workspace)])
}

/// What `capture_format` puts before `{{text}}` on the text's own line. NUL
/// stands in for the text: `clean` removes it from anything captured.
fn text_prefix(format: &str, workspace: &str, now: &DateTime<Local>) -> String {
    let probe = templates::render(format, now, &[("text", "\0"), ("workspace", workspace)]);
    let before = probe.split('\0').next().unwrap_or_default();
    before.rsplit('\n').next().unwrap_or_default().to_string()
}

/// The column a line's content starts at, as CommonMark counts it: past the
/// indentation, and past a list marker (`*`, `-`, `+`, `1.`, `1)`) with the
/// one to four spaces after it. A line with no marker is a paragraph, whose
/// content starts after its indentation. Tabs stop every four columns.
fn content_column(line: &str) -> usize {
    let width = |s: &str, from: usize| {
        s.chars().fold(from, |col, c| {
            if c == '\t' {
                col + 4 - col % 4
            } else {
                col + 1
            }
        })
    };
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = width(&line[..line.len() - rest.len()], 0);
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let marker = match rest[digits..].chars().next() {
        Some('*' | '-' | '+') if digits == 0 => 1,
        Some('.' | ')') if (1..=9).contains(&digits) => digits + 1,
        _ => return indent,
    };
    let after = &rest[marker..];
    let spaces = after.len() - after.trim_start_matches([' ', '\t']).len();
    let marker_end = indent + marker;
    let gap = width(&after[..spaces], marker_end) - marker_end;
    // Five or more spaces after the marker make the content an indented code
    // block that starts one column past the marker. A marker with no space
    // after it (`*{{text}}`) is no list item, but counting it as one only
    // overshoots, and indenting further than needed is still safe.
    if (1..=4).contains(&gap) {
        marker_end + gap
    } else {
        marker_end + 1
    }
}

/// The heading line in `content` that `section` names, compared ignoring
/// case, as the note writes it. A line indented four or more columns is not
/// a heading, so a pasted `## Inbox` inside an earlier capture is never taken
/// for the section.
fn find_section(content: &str, section: &str) -> Option<String> {
    let wanted = section.trim().to_lowercase();
    content
        .lines()
        .filter(|l| {
            let rest = l.trim_start_matches(' ');
            l.len() - rest.len() <= 3 && !rest.starts_with('\t')
        })
        .map(str::trim)
        .find(|l| l.to_lowercase() == wanted)
        .map(str::to_string)
}

/// Add `text` to today's note, making the note (and the capture section) when
/// missing. `text` must not be blank. Refused, before anything reaches
/// bearcli, when daily notes are off.
pub async fn capture(
    client: &BearClient,
    config: &Config,
    text: &str,
    now: &DateTime<Local>,
) -> Result<(), String> {
    let daily_cfg = settings(config)?;
    let text = clean(text);
    if text.trim().is_empty() {
        return Err("nothing to capture".into());
    }
    let section = daily_cfg.capture_section.trim();
    if !section.is_empty() && !valid_section(section) {
        return Err(format!(
            "[daily] capture_section {section:?} is not a heading line like \"## Inbox\""
        ));
    }
    let daily = today(config, now)?;
    let id = ensure(client, &daily).await.map_err(|e| e.message)?;
    let line = capture_line(&daily_cfg.capture_format, &config.workspace, &text, now);
    if section.is_empty() {
        return client.append(&id, &line, None).await.map_err(|e| e.message);
    }
    // bearcli refuses a section that is not there, so look first (ignoring
    // case) and start the section at the end of the note when it is missing.
    let note = client.cat(&id).await.map_err(|e| e.message)?;
    match find_section(&note.content, section) {
        Some(heading) => client.append(&id, &line, Some(&heading)).await,
        None => {
            client
                .append(&id, &format!("\n{section}\n{line}"), None)
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

    /// Daily notes on, as an empty `[daily]` table turns them on.
    fn config(dir: &std::path::Path) -> Config {
        Config {
            templates_dir: dir.to_path_buf(),
            daily: Some(DailyConfig::default()),
            ..Config::default()
        }
    }

    fn daily_mut(cfg: &mut Config) -> &mut DailyConfig {
        cfg.daily.as_mut().expect("daily notes are on")
    }

    #[test]
    fn nothing_is_worked_out_while_daily_notes_are_off() {
        let dir = tempfile::tempdir().unwrap();
        let off = Config {
            daily: None,
            ..config(dir.path())
        };
        assert_eq!(today(&off, &at()).unwrap_err(), DAILY_OFF);
        assert_eq!(settings(&off).unwrap_err(), DAILY_OFF);
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
        daily_mut(&mut cfg).title = "%Y-%m-%d".into();
        daily_mut(&mut cfg).tag = String::new();
        let daily = today(&cfg, &at()).unwrap();
        assert_eq!(daily.content, "# 2026-09-19\nSaturday\n");
        daily_mut(&mut cfg).template = "standup".into();
        let err = today(&cfg, &at()).unwrap_err();
        assert!(err.contains("\"standup\" not found"), "{err}");
    }

    #[test]
    fn bad_formats_are_reported_not_panicked_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path());
        daily_mut(&mut cfg).title = "%Q".into();
        assert!(today(&cfg, &at()).unwrap_err().contains("title"));
        daily_mut(&mut cfg).title = "%Y".into();
        daily_mut(&mut cfg).tag = "log/%Q".into();
        assert!(today(&cfg, &at()).unwrap_err().contains("tag"));
    }

    #[test]
    fn control_characters_are_refused_in_titles_and_stripped_from_captures() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path());
        daily_mut(&mut cfg).title = "%Y%n%m".into();
        assert!(
            today(&cfg, &at())
                .unwrap_err()
                .contains("control character")
        );
        daily_mut(&mut cfg).title = "%Y".into();
        daily_mut(&mut cfg).tag = "log%t%Y".into();
        assert!(
            today(&cfg, &at())
                .unwrap_err()
                .contains("control character")
        );
        assert_eq!(clean("a\x1b[31mb\tc\r\nd\x07"), "a[31mb\tc\nd");
        assert_eq!(clean("old\rmac\r\r\nend"), "old\nmac\n\nend");
    }

    #[test]
    fn captures_lose_del_and_c1_controls_too() {
        assert_eq!(clean("a\x7fb\u{9b}31mc\u{85}d\u{9f}"), "ab31mcd");
        assert_eq!(
            clean("tab\tand\nline, é and — stay"),
            "tab\tand\nline, é and — stay"
        );
    }

    #[test]
    fn a_title_format_with_a_newline_or_tab_is_refused_when_loaded() {
        for bad in ["%Y-%m-%d%n", "%F%t(%A)", "%F\u{7}", "%F\n"] {
            let err = check_title_format(bad).unwrap_err();
            assert!(err.contains("control character"), "{bad:?}: {err}");
            assert!(err.starts_with(&format!("[daily] title {bad:?} ")), "{err}");
        }
        assert_eq!(
            check_title_format("%F %%n %%t"),
            Ok(()),
            "escaped, not fields"
        );
    }

    #[test]
    fn a_tag_counts_as_written_only_whole() {
        let content = "## Day\n#log/2026/09/25 #two words#\n```\n#log/2026/09/2\n```\n";
        assert!(!writes_tag(content, "log/2026/09/2"));
        assert!(!writes_tag(content, "log/2026/09"));
        assert!(!writes_tag(content, "two"));
        assert!(writes_tag(content, "log/2026/09/25"));
        assert!(writes_tag(content, "#two words#"));
        assert!(writes_tag("text #log/2026/09/2 here", "log/2026/09/2"));
        assert!(!writes_tag("## log/2026/09/2", "log/2026/09/2"));
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
        daily_mut(&mut cfg).template = "sub/day".into();
        assert!(
            today(&cfg, &at())
                .unwrap()
                .content
                .starts_with("day September")
        );
        daily_mut(&mut cfg).template = "sub/day.md".into();
        assert!(today(&cfg, &at()).is_ok());
    }

    #[test]
    fn a_daily_title_must_name_exactly_one_day() {
        for ok in [
            "%Y-%m-%d",
            "%F",
            "%B %-d, %Y (%A)",
            "%e %b %Y",
            "%_d.%0m.%y",
            "%D",
            "%x",
            "%v",
            "%C%y-%m-%d",
            "%Y-%j",
            "%y%-j",
            "%Y week %U, %a",
            "%Y-W%W-%u",
            "%G-W%V-%u",
            "%g/%V/%A",
            "Log %F %%",
        ] {
            assert_eq!(check_title_format(ok), Ok(()), "{ok}");
        }
        for (bad, why) in [
            ("%A", "has no year"),
            ("%B %-d", "has no year"),
            ("Work log", "has no year"),
            ("%%d %%Y", "has no year"),
            ("%C-%m-%d", "has no year"),
            ("%Y", "no single day"),
            ("%Y (%A)", "no single day"),
            ("%Y-%m", "no single day"),
            ("%d %Y", "no single day"),
            ("%Y %U", "no single day"),
            ("%G-%m-%d", "ISO week-based year"),
            ("%G-%j", "ISO week-based year"),
            ("%Y-W%V-%u", "ISO week (%V) but the calendar year"),
            ("%F %H:%M", "time of day"),
            ("%F %p", "time of day"),
            ("%F %Z", "time zone"),
            ("%F %z", "time zone"),
            ("%F %s", "time of day"),
            ("%F %.3f", "time of day"),
            ("%c", "time of day"),
            ("%+", "time of day"),
            ("%Q %F", "not a date format"),
            ("%F %", "not a date format"),
        ] {
            let err = check_title_format(bad).unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
            assert!(err.starts_with(&format!("[daily] title {bad:?} ")), "{err}");
        }
    }

    #[test]
    fn multi_line_captures_are_indented_past_the_bullet() {
        let default = DailyConfig::default().capture_format;
        let paste = clean("first\r\n## Foo\r\n---\r\n\r\n  \r\n  kept indent\r\n");
        assert_eq!(
            capture_line(&default, "", &paste, &at()),
            "* 14:05 first\n      ## Foo\n      ---\n        kept indent"
        );
        // Blank lines around a one-line capture go; its own spacing stays.
        assert_eq!(
            capture_line(&default, "", "\n\n call Ana\n\n", &at()),
            "* 14:05  call Ana"
        );
        for (format, pad) in [
            ("- [ ] {{text}}", 6),
            ("1. {{text}}", 7),
            ("10) {{text}}", 8),
            ("  - {{time}}: {{text}}", 8),
            ("\t* {{text}}", 10),
            ("*     {{text}}", 6),
            ("{{text}}", 4),
            ("{{time}}\n* {{text}}", 6),
        ] {
            let line = capture_line(format, "", "a\n## b", &at());
            assert!(
                line.ends_with(&format!("a\n{}## b", " ".repeat(pad))),
                "{format:?}: {line:?}"
            );
        }
        // Captured placeholders are still left alone.
        assert_eq!(
            capture_line("* {{text}}", "", "{{date}}\n{{text}}", &at()),
            "* {{date}}\n      {{text}}"
        );
    }

    #[test]
    fn indented_multi_line_captures_render_as_text_not_structure() {
        let paste = "first\n## Foo\n---\n===\n> quote\n- item\n```\n| a |\n|---|";
        for format in ["* {{time}} {{text}}", "1. {{text}}", "{{text}}"] {
            let line = capture_line(format, "", paste, &at());
            let note = format!("## Day\n{line}\n\n## Inbox\n");
            let (_, headings) = crate::ui::markdown::render_with_headings(&note);
            let names: Vec<&str> = headings.iter().map(|h| h.text.as_str()).collect();
            assert_eq!(names, ["Day", "Inbox"], "{format:?}");
            let html =
                crate::render_html::render_body(&note, &Default::default(), &Default::default());
            for tag in ["<hr", "<blockquote", "<pre", "<table", "<h1", "<h3"] {
                assert!(!html.contains(tag), "{format:?} made {tag}: {html}");
            }
            assert_eq!(html.matches("<h2>").count(), 2, "{format:?}: {html}");
            assert_eq!(
                html.matches("<li>").count(),
                usize::from(format != "{{text}}"),
                "{html}"
            );
        }
    }

    #[test]
    fn only_an_unindented_heading_is_the_capture_section() {
        let note = "# Day\n- one\n      ## inbox\n\t## Inbox\n   ## INBOX  \n";
        assert_eq!(find_section(note, "## Inbox").as_deref(), Some("## INBOX"));
        assert_eq!(find_section("- x\n    ## Inbox\n", "## Inbox"), None);
    }

    #[test]
    fn capture_lines_follow_the_format() {
        let default = DailyConfig::default().capture_format;
        assert_eq!(
            capture_line(&default, "", "call Ana", &at()),
            "* 14:05 call Ana"
        );
        assert_eq!(
            capture_line("- [ ] {{text}}", "", "call Ana", &at()),
            "- [ ] call Ana"
        );
        assert_eq!(
            capture_line("{{workspace}}: {{text}}", "work", "call Ana", &at()),
            "work: call Ana"
        );
    }
}
