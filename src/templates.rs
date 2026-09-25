//! Note templates: Markdown files in the templates directory (`[templates]
//! dir`, default `~/.config/bjorn/templates/`), with `{{placeholders}}` filled
//! in when a note is made from one.
//!
//! A template is the body of the note. When its first line (after any YAML
//! front matter) is a heading, that heading is the note's title: `N` offers
//! its text as the title to edit, and the note is written with the heading at
//! the same level carrying the title that was settled on, so Bear keeps the
//! line as it is instead of adding a `# ` title above it.
//!
//! Placeholders: `{{date}}` (2026-09-19), `{{time}}` (14:05), `{{date:FMT}}`
//! (any strftime format), `{{title}}`, `{{workspace}}` and `{{tag}}` (the
//! note's tags written the way Bear reads them). Anything else in braces, an
//! empty `{{date:}}`, or a `{{date:...}}` whose format chrono cannot read,
//! stays as it was typed. A line holding only `{{tag}}` is dropped when there
//! is no tag.
//!
//! Reading is careful the way theme loading is: only regular files, checked
//! before opening (so a FIFO never blocks), checked again on the open handle,
//! and read through a size cap. Symlinks are followed on purpose: people keep
//! their templates in a dotfiles repo and link them in.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Local};

/// The daily note's layout when no `daily.md` is in the templates directory.
/// The file in the repo is the same text, so copying it is a starting point.
pub const DAILY_TEMPLATE: &str = include_str!("../config/templates/daily.md");

/// Template files larger than this are refused: a template is a page of
/// scaffolding, and the picker reads every file each time it opens.
pub const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The file name without `.md`: what the picker shows and filters on.
    pub name: String,
    pub body: String,
    pub path: PathBuf,
}

/// Why a template could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Nothing at that path.
    Missing,
    /// Something is there but it cannot be used: the reason, for a message.
    Unusable(String),
}

/// What `list` found: the usable templates, and how many `*.md` entries it
/// had to leave out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub templates: Vec<Template>,
    pub skipped: usize,
}

/// Every usable `*.md` template in `dir`, by name. A missing directory is an
/// empty listing; an unreadable, oversized or non-UTF-8 file is counted in
/// `skipped`.
pub fn list(dir: &Path) -> Listing {
    let mut listing = Listing::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return listing;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match read(&path) {
            Ok(template) => listing.templates.push(template),
            Err(_) => listing.skipped += 1,
        }
    }
    listing.templates.sort_by_key(|t| t.name.to_lowercase());
    listing
}

/// Read one template. Symlinks are followed; the target must be a regular
/// file of at most `MAX_TEMPLATE_BYTES` holding UTF-8 text.
pub fn read(path: &Path) -> Result<Template, LoadError> {
    let unusable = |why: &str| LoadError::Unusable(format!("{}: {why}", path.display()));
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(err) => return Err(unusable(&err.to_string())),
    };
    // Checked before opening: opening a FIFO for reading would block.
    if !meta.is_file() {
        return Err(unusable("not a regular file"));
    }
    let too_big = || unusable(&format!("larger than {} KiB", MAX_TEMPLATE_BYTES / 1024));
    if meta.len() > MAX_TEMPLATE_BYTES {
        return Err(too_big());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| unusable(&e.to_string()))?;
    // The path may have been swapped between the check and the open.
    let meta = file.metadata().map_err(|e| unusable(&e.to_string()))?;
    if !meta.is_file() {
        return Err(unusable("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEMPLATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| unusable(&e.to_string()))?;
    if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err(too_big());
    }
    let body = String::from_utf8(bytes).map_err(|_| unusable("not UTF-8 text"))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Template {
        name,
        body,
        path: path.to_path_buf(),
    })
}

/// Where the template called `name` lives. An absolute path or one starting
/// with `~/` is taken as it is; anything else (`daily`, `daily.md`,
/// `sub/day`) is inside `dir`. `.md` is added unless it is already there.
pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    let name = name.trim();
    let file = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    if file.starts_with('/') || file.starts_with("~/") {
        crate::util::expand_tilde(&file)
    } else {
        dir.join(file)
    }
}

/// Do `a` and `b` name the same file? Compared resolved (symlinks, `..`)
/// when both exist, else by their components, which drops `./` segments.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.components().eq(b.components()),
    }
}

/// The template called `name` (see `path_for`).
pub fn load(dir: &Path, name: &str) -> Result<Template, LoadError> {
    read(&path_for(dir, name))
}

/// `now` in a strftime `format`, or `None` when chrono cannot read the format
/// (formatting an invalid one would panic).
pub fn strftime(now: &DateTime<Local>, format: &str) -> Option<String> {
    let items: Vec<Item> = StrftimeItems::new(format).collect();
    if items.iter().any(|item| matches!(item, Item::Error)) {
        return None;
    }
    let mut out = String::new();
    write!(out, "{}", now.format_with_items(items.into_iter())).ok()?;
    Some(out)
}

/// A tag as Bear reads it in a note: `#log/2026/09/19`, or `#two words#` when
/// it holds a space. Empty for no tag.
pub fn hashtag(tag: &str) -> String {
    let tag = tag.trim().trim_matches('#').trim();
    if tag.is_empty() {
        String::new()
    } else if tag.chars().any(char::is_whitespace) {
        format!("#{tag}#")
    } else {
        format!("#{tag}")
    }
}

/// Several tags as hashtags, space-separated.
pub fn hashtags(tags: &[String]) -> String {
    tags.iter()
        .map(|t| hashtag(t))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fill the placeholders in `text`. `vars` are the named ones (`title`,
/// `workspace`, `tag`, `text`); `date`, `time` and `date:FMT` come from `now`.
/// Unknown names are left alone, braces and all.
pub fn render(text: &str, now: &DateTime<Local>, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            rest = &rest[start..];
            break;
        };
        // A stray `{{` before a real placeholder: keep it and start again
        // at the inner one.
        if let Some(inner) = after[..end].find("{{") {
            out.push_str(&rest[start..start + 2 + inner]);
            rest = &after[inner..];
            continue;
        }
        let key = &after[..end];
        match value(key.trim(), now, vars) {
            Some(value) => out.push_str(&value),
            None => {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn value(key: &str, now: &DateTime<Local>, vars: &[(&str, &str)]) -> Option<String> {
    match key {
        "date" => return Some(now.format("%Y-%m-%d").to_string()),
        "time" => return Some(now.format("%H:%M").to_string()),
        _ => {}
    }
    if let Some(format) = key.strip_prefix("date:") {
        if format.is_empty() {
            return None;
        }
        return strftime(now, format);
    }
    vars.iter()
        .find(|(name, _)| *name == key)
        .map(|(_, v)| v.to_string())
}

/// `render`, after dropping lines that hold only `{{tag}}` when `tag` is empty.
pub fn render_note(body: &str, now: &DateTime<Local>, vars: &[(&str, &str)]) -> String {
    let no_tag = vars
        .iter()
        .find(|(name, _)| *name == "tag")
        .is_none_or(|(_, v)| v.is_empty());
    if no_tag {
        let kept: String = body
            .split_inclusive('\n')
            .filter(|line| line.trim() != "{{tag}}")
            .collect();
        render(&kept, now, vars)
    } else {
        render(body, now, vars)
    }
}

/// A body split into its YAML front matter (a `---` block opening on the
/// first line, closing line included) and the rest. The block may end its
/// lines with `\r\n` as well as `\n`: a template saved on Windows has front
/// matter too.
pub fn split_front_matter(body: &str) -> (&str, &str) {
    let Some(mut offset) = ["---\n", "---\r\n"]
        .iter()
        .find(|opener| body.starts_with(*opener))
        .map(|opener| opener.len())
    else {
        return ("", body);
    };
    for line in body[offset..].split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == "---" {
            return (&body[..offset], &body[offset..]);
        }
    }
    ("", body)
}

/// The line ending `body` mostly uses: `\r\n` when more of its lines end
/// that way than with a bare `\n`, else `\n`. A line Bjorn writes into the
/// body takes this ending, so a CRLF template does not come out mixed.
pub fn line_ending(body: &str) -> &'static str {
    let crlf = body.matches("\r\n").count();
    let lf = body.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

/// The heading a template opens with (after any front matter), as (front
/// matter, level, text), and the body after the heading.
fn split_heading(body: &str) -> (&str, Option<(usize, &str)>, &str) {
    let (front, body) = split_front_matter(body);
    let (first, rest) = match body.split_once('\n') {
        Some((first, rest)) => (first, rest),
        None => (body, ""),
    };
    let level = first.len() - first.trim_start_matches('#').len();
    if (1..=6).contains(&level) && first[level..].starts_with(' ') {
        (front, Some((level, first[level..].trim())), rest)
    } else {
        (front, None, body)
    }
}

impl Template {
    /// The title to offer for a new note: the opening heading, filled in.
    pub fn title(&self, now: &DateTime<Local>, workspace: &str, tags: &[String]) -> String {
        let tag = hashtags(tags);
        match split_heading(&self.body).1 {
            Some((_, text)) => render(
                text,
                now,
                &[("title", ""), ("workspace", workspace), ("tag", &tag)],
            )
            .trim()
            .to_string(),
            None => String::new(),
        }
    }

    /// The note's content once its title and tags are settled: any front
    /// matter, the opening heading at its own level carrying `title`, then the
    /// rest filled in. The heading line ends the way the template's lines
    /// mostly do.
    pub fn content(
        &self,
        now: &DateTime<Local>,
        title: &str,
        workspace: &str,
        tags: &[String],
    ) -> String {
        let tag = hashtags(tags);
        let vars = [("title", title), ("workspace", workspace), ("tag", &tag)];
        match split_heading(&self.body) {
            (front, Some((level, _)), rest) => format!(
                "{}{} {title}{}{}",
                render(front, now, &vars),
                "#".repeat(level),
                line_ending(&self.body),
                render_note(rest, now, &vars)
            ),
            (_, None, _) => render_note(&self.body, now, &vars),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 19, 14, 5, 0).unwrap()
    }

    fn template(body: &str) -> Template {
        Template {
            name: "t".into(),
            body: body.into(),
            path: PathBuf::new(),
        }
    }

    #[test]
    fn placeholders_are_filled_and_unknown_ones_kept() {
        let text = "{{date}} {{time}} {{ title }} {{workspace}} {{date:%A}} {{nope}} {{date:%Q}} {{date:}} {{";
        assert_eq!(
            render(text, &at(), &[("title", "Plan"), ("workspace", "work")]),
            "2026-09-19 14:05 Plan work Saturday {{nope}} {{date:%Q}} {{date:}} {{"
        );
    }

    #[test]
    fn a_stray_opening_does_not_swallow_the_next_placeholder() {
        assert_eq!(
            render("{{ oops {{date}} and {{time}}", &at(), &[]),
            "{{ oops 2026-09-19 and 14:05"
        );
        assert_eq!(render("{{a {{b {{date}}", &at(), &[]), "{{a {{b 2026-09-19");
    }

    #[test]
    fn strftime_refuses_a_bad_format_instead_of_panicking() {
        assert_eq!(
            strftime(&at(), "%B %-d, %Y (%A)").as_deref(),
            Some("September 19, 2026 (Saturday)")
        );
        assert_eq!(
            strftime(&at(), "log/%Y/%m/%d").as_deref(),
            Some("log/2026/09/19")
        );
        assert_eq!(strftime(&at(), "%Q"), None);
    }

    #[test]
    fn hashtags_close_when_they_hold_a_space() {
        assert_eq!(hashtag("log/2026/09/19"), "#log/2026/09/19");
        assert_eq!(hashtag("#two words"), "#two words#");
        assert_eq!(hashtag("  "), "");
        assert_eq!(hashtags(&["a".into(), "".into(), "b c".into()]), "#a #b c#");
    }

    #[test]
    fn a_heading_template_keeps_its_level_under_the_new_title() {
        let t = template("## Meeting {{date}}\n{{tag}}\nAbout {{title}} in {{workspace}}\n");
        assert_eq!(t.title(&at(), "work", &[]), "Meeting 2026-09-19");
        assert_eq!(
            t.content(&at(), "Meeting with Ana", "work", &["work/meetings".into()]),
            "## Meeting with Ana\n#work/meetings\nAbout Meeting with Ana in work\n"
        );
        assert_eq!(
            t.content(&at(), "M", "", &[]),
            "## M\nAbout M in \n",
            "a tag-only line goes when there is no tag"
        );
        let plain = template("#tag\nbody {{title}}");
        assert_eq!(plain.title(&at(), "", &[]), "");
        assert_eq!(plain.content(&at(), "T", "", &[]), "#tag\nbody T");
    }

    #[test]
    fn front_matter_stays_on_top_and_the_heading_after_it_is_the_title() {
        let t = template("---\ntype: meeting\ndate: {{date}}\n---\n## Sync\nbody\n");
        assert_eq!(t.title(&at(), "", &[]), "Sync");
        assert_eq!(
            t.content(&at(), "Sync with Ana", "", &[]),
            "---\ntype: meeting\ndate: 2026-09-19\n---\n## Sync with Ana\nbody\n"
        );
        let bare = template("---\na: 1\n---\nbody\n");
        assert_eq!(bare.title(&at(), "", &[]), "");
        assert_eq!(bare.content(&at(), "T", "", &[]), "---\na: 1\n---\nbody\n");
        assert_eq!(
            split_front_matter("---\nunclosed\n"),
            ("", "---\nunclosed\n")
        );
    }

    #[test]
    fn a_crlf_template_keeps_its_front_matter_and_its_line_endings() {
        let t = template("---\r\ntype: meeting\r\n---\r\n## Sync\r\n{{tag}}\r\nbody\r\n");
        assert_eq!(t.title(&at(), "", &[]), "Sync");
        assert_eq!(
            t.content(&at(), "Sync with Ana", "", &["work".into()]),
            "---\r\ntype: meeting\r\n---\r\n## Sync with Ana\r\n#work\r\nbody\r\n"
        );
        assert_eq!(
            t.content(&at(), "S", "", &[]),
            "---\r\ntype: meeting\r\n---\r\n## S\r\nbody\r\n",
            "a tag-only CRLF line goes when there is no tag"
        );
        assert_eq!(
            split_front_matter("---\r\na: 1\r\n---\r\nrest"),
            ("---\r\na: 1\r\n---\r\n", "rest")
        );
        assert_eq!(line_ending("a\r\nb\r\nc\n"), "\r\n");
        assert_eq!(line_ending("a\r\nb\nc\n"), "\n");
        assert_eq!(line_ending("one line"), "\n");
        // A mostly-LF template stays LF.
        let lf = template("## T\nbody\r\nmore\n");
        assert_eq!(lf.content(&at(), "X", "", &[]), "## X\nbody\r\nmore\n");
    }

    #[test]
    fn templates_are_listed_by_name_and_unusable_ones_counted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.md"), "B").unwrap();
        std::fs::write(dir.path().join("A.md"), "A").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "no").unwrap();
        std::fs::create_dir(dir.path().join("dir.md")).unwrap();
        std::fs::write(dir.path().join("latin1.md"), [0x63, 0x61, 0x66, 0xe9]).unwrap();
        std::fs::write(
            dir.path().join("huge.md"),
            "x".repeat(MAX_TEMPLATE_BYTES as usize + 1),
        )
        .unwrap();
        let listing = list(dir.path());
        let names: Vec<&str> = listing.templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["A", "b"]);
        assert_eq!(listing.skipped, 3, "dir.md, latin1.md, huge.md");
        assert_eq!(list(&dir.path().join("missing")), Listing::default());
        assert_eq!(load(dir.path(), "b").unwrap().body, "B");
        assert_eq!(load(dir.path(), "c"), Err(LoadError::Missing));
        for bad in ["latin1", "huge", "dir"] {
            match load(dir.path(), bad) {
                Err(LoadError::Unusable(why)) => assert!(why.contains(bad), "{why}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
    }

    #[test]
    fn symlinked_templates_are_followed_and_fifos_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("real.md"), "linked").unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("real.md"), dir.path().join("l.md"))
            .unwrap();
        assert_eq!(load(dir.path(), "l").unwrap().body, "linked");
        let fifo = dir.path().join("pipe.md");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            // On a thread with a deadline, so a read that blocks on the FIFO
            // fails the test instead of hanging the suite.
            let (tx, rx) = std::sync::mpsc::channel();
            let templates = dir.path().to_path_buf();
            std::thread::spawn(move || {
                let _ = tx.send(load(&templates, "pipe"));
            });
            let loaded = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("reading a FIFO template blocked");
            match loaded {
                Err(LoadError::Unusable(why)) => {
                    assert!(why.contains("not a regular file"), "{why}")
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(list(dir.path()).skipped, 1, "the listing skips it too");
        }
    }

    #[test]
    fn same_file_sees_through_dot_segments_and_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("daily.md"), "x").unwrap();
        let plain = dir.path().join("daily.md");
        assert!(same_file(&plain, &path_for(dir.path(), "./daily")));
        std::os::unix::fs::symlink(&plain, dir.path().join("link.md")).unwrap();
        assert!(same_file(&plain, &dir.path().join("link.md")));
        assert!(!same_file(&plain, &dir.path().join("other.md")));
        assert!(same_file(
            Path::new("/nope/./a.md"),
            Path::new("/nope/a.md")
        ));
    }

    #[test]
    fn relative_names_live_in_the_templates_dir() {
        let dir = Path::new("/tpl");
        assert_eq!(path_for(dir, "daily"), Path::new("/tpl/daily.md"));
        assert_eq!(path_for(dir, "daily.md"), Path::new("/tpl/daily.md"));
        assert_eq!(path_for(dir, "sub/day"), Path::new("/tpl/sub/day.md"));
        assert_eq!(path_for(dir, "/x/day"), Path::new("/x/day.md"));
        assert_eq!(
            path_for(dir, "~/day.md"),
            crate::util::home_dir().join("day.md")
        );
    }

    #[test]
    fn the_shipped_templates_render() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/templates");
        let listing = list(&dir);
        assert_eq!(listing.skipped, 0);
        let names: Vec<&str> = listing.templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["daily", "decision", "meeting", "one-on-one"]);
        for t in &listing.templates {
            for tags in [vec![], vec!["log/2026/09/19".to_string()]] {
                let content = t.content(&at(), "T", "", &tags);
                assert!(!content.contains("{{"), "{}: {content}", t.name);
            }
        }
        let daily = load(&dir, "daily").unwrap();
        assert_eq!(daily.body, DAILY_TEMPLATE);
        assert_eq!(
            daily.content(&at(), "T", "", &["log/2026/09/19".into()]),
            "## T\n#log/2026/09/19\n* People:\n* Topic:\n\n---\n"
        );
    }
}
