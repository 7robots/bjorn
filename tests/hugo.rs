//! Publishing to a Hugo site: the pure plan/write pair against temp sites,
//! then `P` and the export picker through the harness.

mod common;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use bjorn::bear::Note;
use bjorn::hugo::{self, HugoConfig, Plan, Status};
use chrono::{DateTime, Local, Utc};
use common::Fake;
use pretty_assertions::assert_eq;

const NOW: &str = "2026-09-19T14:00:00Z";
const LATER: &str = "2026-09-21T09:30:00Z";

struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    fn new() -> Site {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("site/content/posts")).unwrap();
        Site { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("site")
    }

    fn posts(&self) -> PathBuf {
        self.root().join("content/posts")
    }

    fn ledger(&self) -> PathBuf {
        self.dir.path().join("config").join(hugo::LEDGER_NAME)
    }

    fn config(&self) -> HugoConfig {
        HugoConfig {
            site: self.root().to_string_lossy().into_owned(),
            tag_prefix: "blog".into(),
            publish_tag: "blog/published".into(),
            media_url: "https://media.example.test/blog".into(),
            media_dir: self.dir.path().join("staging"),
            ..HugoConfig::default()
        }
    }

    fn plan(&self, cfg: &HugoConfig, note: &Note, content: &str) -> Result<Plan, String> {
        self.plan_at(cfg, note, content, &HashMap::new(), NOW)
    }

    fn plan_at(
        &self,
        cfg: &HugoConfig,
        note: &Note,
        content: &str,
        files: &HashMap<String, Vec<u8>>,
        now: &str,
    ) -> Result<Plan, String> {
        hugo::plan(cfg, &self.ledger(), note, content, files, utc(now)).map_err(|e| e.0)
    }

    fn publish(&self, cfg: &HugoConfig, note: &Note, content: &str) -> Plan {
        self.publish_at(cfg, note, content, &HashMap::new(), NOW)
    }

    fn publish_at(
        &self,
        cfg: &HugoConfig,
        note: &Note,
        content: &str,
        files: &HashMap<String, Vec<u8>>,
        now: &str,
    ) -> Plan {
        let plan = self.plan_at(cfg, note, content, files, now).unwrap();
        hugo::write(&plan).unwrap();
        plan
    }
}

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn local(text: &str, fmt: &str) -> String {
    utc(text).with_timezone(&Local).format(fmt).to_string()
}

fn stamp(text: &str) -> String {
    local(text, "%Y-%m-%dT%H:%M:%S%:z")
}

fn month() -> String {
    local(NOW, "%Y/%m")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn note(tags: &[&str]) -> Note {
    Note {
        id: "NOTE-1".into(),
        title: "Pixel-Perfect: Real Pixel Art".into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created: Some(utc("2026-07-15T10:15:00Z")),
        modified: Some(utc("2026-09-16T12:00:00Z")),
        ..Note::default()
    }
}

fn other(id: &str, title: &str) -> Note {
    Note {
        id: id.into(),
        title: title.into(),
        ..note(&[])
    }
}

const BODY: &str = "# Pixel-Perfect: Real Pixel Art\n#blog/ai #blog/image processing#\n\n\
Ask any model for pixel art and you get something that looks right.\n\n\
## Before and after\n\nText with a #blog/ai tag inline.\n";

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

// -- decision A: shape, date, URL, dialog ---------------------------------------

#[test]
fn a_new_post_is_dated_now_and_has_the_sites_front_matter_shape() {
    let site = Site::new();
    let plan = site.publish(
        &site.config(),
        &note(&["blog", "blog/ai", "blog/image processing"]),
        BODY,
    );
    assert_eq!(plan.status, Status::New);
    assert!(plan.draft);
    let rel = format!("content/posts/{}/pixel-perfect-real-pixel-art.md", month());
    assert_eq!(plan.display, rel);
    assert_eq!(
        plan.url,
        format!("/{}/pixel-perfect-real-pixel-art/", local(NOW, "%Y/%m/%d"))
    );
    assert_eq!(
        read(&plan.target),
        format!(
            "---\ntitle: \"Pixel-Perfect: Real Pixel Art\"\nslug: \"pixel-perfect-real-pixel-art\"\n\
             date: {}\ndraft: true\ntags:\n  - ai\n  - image-processing\ndescription: \"\"\n---\n\n\
             Ask any model for pixel art and you get something that looks right.\n\n<!--more-->\n\n\
             ## Before and after\n\nText with a tag inline.\n",
            stamp(NOW),
        )
    );
    let text = read(&plan.target);
    assert!(!text.contains("NOTE-1") && !text.contains(&*site.dir.path().to_string_lossy()));
    assert!(
        read(&site.ledger()).contains("NOTE-1"),
        "the ledger, outside the site, knows the note"
    );
}

#[test]
fn the_dialog_puts_each_fact_on_its_own_line_below_the_title() {
    let site = Site::new();
    let n = Note {
        title: "Evil\u{202e}gpj.exe".into(),
        ..note(&["blog/ai"])
    };
    let plan = site
        .plan(&site.config(), &n, "# Evil\u{202e}gpj.exe\n\nBody\n")
        .unwrap();
    let q = plan.question();
    let lines: Vec<&str> = q.lines().collect();
    assert_eq!(lines[0], "Publish a new Hugo post?");
    assert_eq!(lines[1], "“Evilgpj.exe”", "bidi controls stripped");
    assert_eq!(
        lines[2],
        format!("File: content/posts/{}/evil-gpj-exe.md", month())
    );
    assert_eq!(
        lines[3],
        format!("URL: /{}/evil-gpj-exe/", local(NOW, "%Y/%m/%d"))
    );
    assert_eq!(lines[4], "State: draft");
    assert_eq!(lines[5], "Tags: ai");
    let long = Note {
        title: "word ".repeat(40),
        ..note(&[])
    };
    let plan = site.plan(&site.config(), &long, "Body").unwrap();
    assert!(plan.question().lines().nth(1).unwrap().chars().count() <= 72);
}

#[test]
fn republishing_keeps_date_slug_and_hand_added_keys() {
    let site = Site::new();
    let cfg = site.config();
    let first = site.publish(&cfg, &note(&["blog/ai"]), BODY);
    let text = read(&first.target);
    let edited = text
        .replace(
            "description: \"\"\n",
            "description: \"Written by hand\"\ncover:\n  image: \"https://media.example.test/c.png\"\n  alt: \"A cover\"\n# keep me\nseries: pixels\n",
        )
        .replace("draft: true", "draft: false # live now");
    std::fs::write(&first.target, &edited).unwrap();

    // Renamed, retagged, and with its own slug and date: the post keeps its URL.
    let renamed = Note {
        title: "A New Title".into(),
        ..note(&["blog/ai", "blog/rust"])
    };
    let again = site
        .plan_at(
            &cfg,
            &renamed,
            "---\nslug: other-url\ndate: 2020-01-01\n---\n# A New Title\n#blog/ai #blog/rust\n\nNew lead.\n",
            &HashMap::new(),
            LATER,
        )
        .unwrap();
    assert_eq!(again.target, first.target);
    assert_eq!(again.status, Status::Edited);
    assert!(again.question().contains("edited since Bjorn wrote it"));
    assert_eq!(again.url, first.url);
    hugo::write(&again).unwrap();
    let text = read(&again.target);
    assert!(
        text.starts_with(&format!(
            "---\ntitle: \"A New Title\"\nslug: \"pixel-perfect-real-pixel-art\"\ndate: {}\nlastmod: {}\ndraft: false\ntags:\n  - ai\n  - rust\n",
            stamp(NOW),
            stamp(LATER)
        )),
        "{text}"
    );
    assert!(
        text.contains("description: \"Written by hand\"\ncover:\n  image: \"https://media.example.test/c.png\"\n  alt: \"A cover\"\n# keep me\nseries: pixels\n---\n\nNew lead.\n\n<!--more-->\n"),
        "{text}"
    );
    assert_eq!(walk(&site.posts()).len(), 1, "no duplicate");
    let third = site
        .plan(&cfg, &renamed, "# A New Title\n\nNew lead.\n")
        .unwrap();
    assert_eq!(third.status, Status::Update);
    assert!(third.question().starts_with("Update this Hugo post?"));
}

// -- finding 1: live stays live ---------------------------------------------------

#[test]
fn nothing_from_the_note_takes_a_live_post_offline() {
    for live in [
        "draft: false",
        "draft: false # live",
        "draft: False",
        "Draft: FALSE",
    ] {
        let site = Site::new();
        let cfg = site.config();
        let first = site.publish(&cfg, &note(&["blog/ai"]), BODY);
        let text = read(&first.target).replace("draft: true", live);
        std::fs::write(&first.target, text).unwrap();
        for note_draft in ["draft: true", "draft: \"true\"", "Draft: True"] {
            let plan = site
                .plan(
                    &cfg,
                    &note(&["blog/ai"]),
                    &format!("---\n{note_draft}\n---\n# T\n\nBody"),
                )
                .unwrap();
            assert!(!plan.draft, "{live} / {note_draft}");
            assert!(plan.text.contains("draft: false\n"), "{}", plan.text);
            assert!(!plan.text.to_lowercase().contains("draft: true"));
        }
    }
    // A post without draft: at all is live to Hugo, and stays live.
    let site = Site::new();
    let dir = site.posts().join(month());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pixel-perfect-real-pixel-art.md"),
        "---\ntitle: x\n---\nold\n",
    )
    .unwrap();
    let plan = site.plan(&site.config(), &note(&[]), BODY).unwrap();
    assert!(!plan.draft);
    // A new post still follows the note.
    let fresh = Site::new();
    let plan = fresh
        .plan(
            &fresh.config(),
            &note(&[]),
            "---\ndraft: false\n---\n# T\n\nBody",
        )
        .unwrap();
    assert!(!plan.draft);
}

// -- finding 2: the ledger belongs to the site ------------------------------------

#[test]
fn ledger_entries_outside_the_section_are_ignored_and_sections_cannot_escape() {
    let site = Site::new();
    let elsewhere = site.dir.path().join("other-site/post.md");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, "---\ntitle: x\n---\n").unwrap();
    std::fs::create_dir_all(site.ledger().parent().unwrap()).unwrap();
    std::fs::write(
        site.ledger(),
        serde_json::json!({"posts": {"NOTE-1": {"path": elsewhere, "sha1": "x"}}}).to_string(),
    )
    .unwrap();
    let plan = site.plan(&site.config(), &note(&[]), BODY).unwrap();
    assert_eq!(plan.status, Status::New);
    assert!(
        plan.target
            .starts_with(site.posts().canonicalize().unwrap())
    );
    for section in ["../..", "..", "posts/../../x", "./posts"] {
        let cfg = HugoConfig {
            section: section.into(),
            ..site.config()
        };
        let why = site.plan(&cfg, &note(&[]), BODY).unwrap_err();
        assert!(why.contains("section"), "{section}: {why}");
    }
}

// -- finding 3: one file per note ---------------------------------------------------

#[test]
fn a_second_note_with_the_same_slug_gets_a_suffix() {
    let site = Site::new();
    let cfg = site.config();
    let first = site.publish(&cfg, &other("A", "Weekly Notes"), "Body A");
    let second = site
        .plan(&cfg, &other("B", "Weekly Notes"), "Body B")
        .unwrap();
    assert_eq!(second.status, Status::New);
    assert!(
        second.display.ends_with("/weekly-notes-2.md"),
        "{}",
        second.display
    );
    assert!(
        second
            .question()
            .contains("already uses the slug weekly-notes; this one gets weekly-notes-2")
    );
    hugo::write(&second).unwrap();
    assert!(read(&first.target).contains("Body A"));
    let ledger = read(&site.ledger());
    assert!(ledger.contains("weekly-notes.md") && ledger.contains("weekly-notes-2.md"));
}

// -- finding 4: a missing post is announced, an existing one found ---------------------

#[test]
fn a_post_that_went_missing_is_announced_and_one_with_the_slug_is_found() {
    let site = Site::new();
    let cfg = site.config();
    let first = site.publish(&cfg, &note(&[]), BODY);
    // Moved by hand to another month.
    let moved = site.posts().join("2025/01/pixel-perfect-real-pixel-art.md");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&first.target, &moved).unwrap();
    let plan = site.plan(&cfg, &note(&[]), BODY).unwrap();
    let q = plan.question();
    assert!(
        q.contains(&format!(
            "The post Bjorn wrote at {} is gone",
            first.display
        )),
        "{q}"
    );
    assert_eq!(plan.target, moved.canonicalize().unwrap());
    assert_eq!(plan.status, Status::Foreign);
    assert!(
        q.contains("already exists at content/posts/2025/01/"),
        "{q}"
    );
    // Deleted outright: announced, and a new file.
    std::fs::remove_file(&moved).unwrap();
    let plan = site.plan(&cfg, &note(&[]), BODY).unwrap();
    assert_eq!(plan.status, Status::New);
    assert!(plan.question().contains("is gone"));
    // Found by its slug: key too, not only its file name.
    let named = site.posts().join("2024/renamed-file.md");
    std::fs::create_dir_all(named.parent().unwrap()).unwrap();
    std::fs::write(
        &named,
        "---\ntitle: x\nslug: pixel-perfect-real-pixel-art\ndraft: true\n---\n",
    )
    .unwrap();
    let plan = site.plan(&cfg, &note(&[]), BODY).unwrap();
    assert_eq!(plan.target, named.canonicalize().unwrap());
}

// -- finding 5: images, then the post, then the ledger ------------------------------------

#[test]
fn a_failed_image_write_leaves_no_post_and_no_ledger() {
    let site = Site::new();
    let blocker = site.dir.path().join("blocked");
    std::fs::write(&blocker, "a file, not a folder").unwrap();
    let cfg = HugoConfig {
        media_dir: blocker,
        ..site.config()
    };
    let files = HashMap::from([("a.png".to_string(), vec![1u8])]);
    let plan = site
        .plan_at(&cfg, &note(&[]), "# T\n\nLead\n\n![](a.png)", &files, NOW)
        .unwrap();
    assert!(hugo::write(&plan).is_err());
    assert!(walk(&site.posts()).is_empty(), "no post");
    assert!(!site.ledger().exists(), "no ledger");
}

// -- finding 6: images ---------------------------------------------------------------

#[test]
fn image_links_in_every_form_are_rewritten_outside_code_only() {
    let site = Site::new();
    // Bear writes decomposed names on disk; the note may carry composed ones.
    let nfd = "Rose\u{301}.png".to_string();
    let files = HashMap::from([
        ("Front bed.png".to_string(), vec![1u8]),
        (nfd, vec![2u8]),
        ("unused.png".to_string(), vec![3u8]),
    ]);
    let content = "# Garden\n\nLead.\n\n![a](Front%20bed.png \"The bed\")\n![b](<Front bed.png>)\n![c](Ros\u{e9}.png)\n\
                   `![](Front%20bed.png)`\n```\n![](Front%20bed.png)\n```\n";
    let plan = site.publish_at(&site.config(), &note(&[]), content, &files, NOW);
    let base = format!("https://media.example.test/blog/{}", month());
    let text = read(&plan.target);
    assert!(
        text.contains(&format!("![a]({base}/garden-front-bed.png \"The bed\")")),
        "{text}"
    );
    assert!(
        text.contains(&format!("![b]({base}/garden-front-bed.png)")),
        "{text}"
    );
    assert!(
        text.contains(&format!("![c]({base}/garden-rose.png)")),
        "{text}"
    );
    assert!(
        text.contains("`![](Front%20bed.png)`\n```\n![](Front%20bed.png)\n```"),
        "code untouched: {text}"
    );
    let staged: Vec<String> = walk(&site.dir.path().join("staging"))
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(staged.len(), 2, "only linked images: {staged:?}");
}

#[test]
fn only_images_are_published_and_unmatched_images_are_refused() {
    let site = Site::new();
    let files = HashMap::from([
        ("x.svg".to_string(), b"<svg onload=alert(1)>".to_vec()),
        ("page.html".to_string(), b"<script>".to_vec()),
        ("plan.pdf".to_string(), vec![0u8]),
    ]);
    let why = site
        .plan_at(&site.config(), &note(&[]), "# T\n\n![](x.svg)", &files, NOW)
        .unwrap_err();
    assert!(why.contains("only those are published"), "{why}");
    let why = site
        .plan_at(
            &site.config(),
            &note(&[]),
            "# T\n\n![](missing.png)",
            &files,
            NOW,
        )
        .unwrap_err();
    assert!(why.contains("not an attachment"), "{why}");
    let plan = site
        .plan_at(
            &site.config(),
            &note(&[]),
            "# T\n\nSee [the plan](plan.pdf) and [page](page.html). ![](https://x.test/a.png)",
            &files,
            NOW,
        )
        .unwrap();
    assert!(plan.media.is_empty());
    assert!(
        plan.text
            .contains("See the plan and page. ![](https://x.test/a.png)"),
        "{}",
        plan.text
    );
    assert!(plan.question().contains("plan.pdf became plain text"));
}

#[test]
fn a_bundle_keeps_images_beside_the_post_and_never_on_its_name() {
    let site = Site::new();
    let cfg = HugoConfig {
        media_url: String::new(),
        path: "{year}/{slug}/index.md".into(),
        ..site.config()
    };
    let files = HashMap::from([
        ("index.png".to_string(), vec![1u8]),
        ("_index.png".to_string(), vec![2u8]),
        ("Front bed.png".to_string(), vec![3u8]),
    ]);
    let plan = site.publish_at(
        &cfg,
        &note(&[]),
        "# Garden\n\n![](index.png) ![](_index.png) ![](Front%20bed.png)",
        &files,
        NOW,
    );
    assert!(plan.display.ends_with("/garden/index.md"));
    let dir = plan.target.parent().unwrap();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["front-bed.png", "index-2.png", "index-3.png", "index.md"]
    );
    assert!(read(&plan.target).contains("![](index-2.png)"));
    assert!(plan.staging.is_none());
    // No media URL and no bundle: refused.
    let cfg = HugoConfig {
        media_url: String::new(),
        ..site.config()
    };
    let why = site
        .plan_at(&cfg, &other("N2", "G"), "![](index.png)", &files, NOW)
        .unwrap_err();
    assert!(why.contains("media_url"));
}

// -- finding 7: a title line without `# ` ---------------------------------------------------

#[test]
fn a_plain_or_lower_heading_title_line_is_dropped() {
    let site = Site::new();
    for (n, content) in [
        "Pixel-Perfect: Real Pixel Art\n\nLead.",
        "## Pixel-Perfect: Real Pixel Art\n#blog/ai\n\nLead.",
    ]
    .iter()
    .enumerate()
    {
        let plan = site
            .plan(
                &site.config(),
                &other(&format!("N{n}"), "Pixel-Perfect: Real Pixel Art"),
                content,
            )
            .unwrap();
        let body = plan.text.split("---\n\n").nth(1).unwrap();
        assert!(body.starts_with("Lead."), "{body}");
    }
}

// -- finding 8: privacy ---------------------------------------------------------------------

#[test]
fn private_links_and_tags_do_not_reach_the_post() {
    let site = Site::new();
    let n = Note {
        title: "Plans #private".into(),
        ..note(&["private", "blog/ai"])
    };
    let plan = site
        .plan(
            &site.config(),
            &n,
            "# Plans #private\n\nLead with [[Secret Note|a note]] and [[Other]].\n\n\
             See [this](bear://x-callback-url/open-note?id=SECRET-ID) (#private) and it is #private.\n",
        )
        .unwrap();
    assert!(plan.text.contains("title: \"Plans\""), "{}", plan.text);
    assert!(
        plan.text.contains("Lead with a note and Other."),
        "{}",
        plan.text
    );
    assert!(plan.text.contains("See this and it is."), "{}", plan.text);
    assert!(!plan.text.contains("SECRET") && !plan.text.contains("private"));
    let why = site
        .plan(&site.config(), &n, "# T\n\n[doc](file:///Users/me/doc.txt)")
        .unwrap_err();
    assert!(why.contains("file://"));
}

// -- finding 9: containment -----------------------------------------------------------------

#[test]
fn symlinks_out_of_the_site_are_refused() {
    let site = Site::new();
    let outside = site.dir.path().join("outside.md");
    std::fs::write(&outside, "---\ntitle: x\n---\n").unwrap();
    let dir = site.posts().join(month());
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("pixel-perfect-real-pixel-art.md")).unwrap();
    let why = site.plan(&site.config(), &note(&[]), BODY).unwrap_err();
    assert!(why.contains("symlink"), "{why}");
    assert_eq!(read(&outside), "---\ntitle: x\n---\n");

    let site = Site::new();
    let away = site.dir.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    std::os::unix::fs::symlink(&away, site.root().join("content/linked")).unwrap();
    let cfg = HugoConfig {
        section: "linked".into(),
        ..site.config()
    };
    assert!(
        site.plan(&cfg, &note(&[]), BODY)
            .unwrap_err()
            .contains("outside")
    );
}

// -- decision B / findings 10 and 11: reading front matter -------------------------------------

#[test]
fn the_notes_front_matter_is_only_read_at_the_very_top() {
    let site = Site::new();
    let why = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\nEdit: the fix is out: see v2.\n---\nBody",
        )
        .unwrap_err();
    assert!(why.contains("not front matter Bjorn can read"), "{why}");
    let why = site
        .plan(&site.config(), &note(&[]), "---\n- a list\n---\nBody")
        .unwrap_err();
    assert!(why.contains("not a list of keys"), "{why}");
    // Under the title it is just part of the body.
    let plan = site
        .plan(
            &site.config(),
            &note(&[]),
            "# T\n\n---\nEdit: the fix is out: see v2.\n---\n\nMore",
        )
        .unwrap();
    assert!(
        plan.text
            .contains("\n---\nEdit: the fix is out: see v2.\n---\n")
    );
    assert!(plan.text.starts_with("---\ntitle: \"T\"\n"));
}

#[test]
fn an_existing_post_bjorn_cannot_read_is_refused_not_rewritten() {
    for (name, bytes) in [
        ("toml", b"+++\ntitle = 'x'\n+++\nbody\n".to_vec()),
        ("json", b"{\"title\": \"x\"}\nbody\n".to_vec()),
        ("broken", b"---\ntitle: [unclosed\n---\nbody\n".to_vec()),
        ("case twins", b"---\ntitle: a\nTitle: b\n---\n".to_vec()),
        ("latin-1", b"---\ntitle: caf\xe9\n---\n".to_vec()),
    ] {
        let site = Site::new();
        let dir = site.posts().join(month());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pixel-perfect-real-pixel-art.md");
        std::fs::write(&path, &bytes).unwrap();
        let why = site.plan(&site.config(), &note(&[]), BODY).unwrap_err();
        assert!(why.contains("refusing to edit it"), "{name}: {why}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
    }
    // A byte order mark is fine.
    let site = Site::new();
    let dir = site.posts().join(month());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pixel-perfect-real-pixel-art.md");
    std::fs::write(
        &path,
        "\u{feff}---\ntitle: x\ncustom: kept\ndraft: true\n---\nold\n",
    )
    .unwrap();
    let plan = site.publish(&site.config(), &note(&[]), BODY);
    let text = read(&plan.target);
    assert!(
        text.starts_with("---\ntitle: \"Pixel-Perfect: Real Pixel Art\"\n")
            && text.contains("custom: kept\n")
    );
}

#[test]
fn dates_are_read_or_refused() {
    let site = Site::new();
    let plan = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\ndate: 2025-03-04 06:15:00\n---\n# T\n\nBody",
        )
        .unwrap();
    let local = chrono::NaiveDateTime::parse_from_str("2025-03-04 06:15:00", "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_local_timezone(Local)
        .earliest()
        .unwrap();
    assert!(
        plan.text
            .contains(&format!("date: {}\n", local.format("%Y-%m-%dT%H:%M:%S%:z")))
    );
    assert!(plan.display.contains("/2025/03/"));
    let plan = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\ndate: 2025-03-04 06:15:00 -04:00\n---\n# T\n\nBody",
        )
        .unwrap();
    assert!(plan.text.contains("date: 2025-03-04T06:15:00-04:00\n"));
    let why = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\ndate: last tuesday\n---\n# T\n\nBody",
        )
        .unwrap_err();
    assert!(why.contains("not a date"), "{why}");
}

// -- decision C: which of the note's keys pass ------------------------------------------------

#[test]
fn only_allowed_note_keys_pass_and_are_written_fresh() {
    let site = Site::new();
    let content = "---\ntitle: 'It''s: \"quoted\"'\nslug: My Custom URL!\ndescription: 'x: y # not a comment'\n\
                   cover:\n  image: x.png\n  alt: A cover\nsummary: Short\nshowtoc: false\n\
                   layout: evil\nurl: /admin/\naliases: [/old/]\nmarkup: {goldmark: {renderer: {unsafe: true}}}\n\
                   outputs: [json]\nbuild: {render: never}\ntype: page\n---\nBody\n";
    let plan = site.publish(&site.config(), &note(&[]), content);
    let text = read(&plan.target);
    assert!(
        plan.display.ends_with("/my-custom-url.md"),
        "slug normalized"
    );
    assert!(
        text.contains("title: \"It's: \\\"quoted\\\"\"\nslug: \"my-custom-url\"\n"),
        "{text}"
    );
    assert!(
        text.contains("description: \"x: y # not a comment\"\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "cover:\n  image: \"x.png\"\n  alt: \"A cover\"\nsummary: \"Short\"\nshowtoc: false\n"
        ),
        "{text}"
    );
    for key in [
        "layout", "url:", "aliases", "markup", "outputs", "build", "type:",
    ] {
        assert!(!text.contains(key), "{key} passed: {text}");
    }
    assert_eq!(
        plan.dropped,
        [
            "aliases", "build", "layout", "markup", "outputs", "type", "url"
        ]
    );
    assert!(
        plan.question()
            .contains("Not published from the note's front matter: aliases, build, layout")
    );
}

// -- decision D: tags ----------------------------------------------------------------------

#[test]
fn an_empty_tag_prefix_publishes_no_tags_and_the_publish_tag_decides_draft() {
    let site = Site::new();
    let cfg = HugoConfig {
        tag_prefix: String::new(),
        ..site.config()
    };
    let plan = site
        .plan(&cfg, &note(&["home", "work/secret", "blog/ai"]), BODY)
        .unwrap();
    assert!(plan.tags.is_empty() && plan.text.contains("tags: []\n"));
    assert!(plan.question().contains("\nTags: none"));
    let plan = site
        .plan(
            &site.config(),
            &note(&["blog/ai", "blog/published", "work/private"]),
            BODY,
        )
        .unwrap();
    assert!(!plan.draft);
    assert_eq!(plan.tags, ["ai"]);
    assert!(plan.question().contains("\nState: published (live)"));
}

// -- finding 15: permissions ---------------------------------------------------------------

#[test]
fn posts_and_images_are_0644_and_the_ledger_is_0600() {
    let site = Site::new();
    let files = HashMap::from([("a.png".to_string(), vec![1u8])]);
    let plan = site.publish_at(
        &site.config(),
        &note(&[]),
        "# T\n\nLead\n\n![](a.png)",
        &files,
        NOW,
    );
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&plan.target), 0o644);
    assert_eq!(mode(&plan.media[0].0), 0o644);
    assert_eq!(mode(&site.ledger()), 0o600);
}

// -- a real build ----------------------------------------------------------------------------

/// If `hugo` is installed, the output builds in a temp site, with the cache in
/// the temp dir too.
#[test]
fn hugo_builds_the_output_when_installed() {
    if bjorn::util::which("hugo").is_none() {
        return;
    }
    let site = Site::new();
    std::fs::write(
        site.root().join("hugo.yaml"),
        "baseURL: \"https://example.test/\"\ntitle: Test\npermalinks:\n  page:\n    posts: \"/:year/:month/:day/:slug/\"\nbuildDrafts: true\n",
    )
    .unwrap();
    let layouts = site.root().join("layouts");
    std::fs::create_dir_all(&layouts).unwrap();
    std::fs::write(
        layouts.join("single.html"),
        "{{ .Title }}|{{ .Params.tags }}|{{ .Draft }}|{{ .Summary }}",
    )
    .unwrap();
    // The note that once broke a site build: a rule-fenced edit under the title.
    let content = format!("{BODY}\n---\nEdit: the fix is out: see v2.\n---\n");
    let plan = site.publish(&site.config(), &note(&["blog/ai"]), &content);
    let out = std::process::Command::new("hugo")
        .arg("--source")
        .arg(site.root())
        .arg("--quiet")
        .env("HUGO_CACHEDIR", site.dir.path().join("hugo-cache"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let page = site
        .root()
        .join("public")
        .join(plan.url.trim_matches('/'))
        .join("index.html");
    let html = read(&page);
    assert!(
        html.starts_with("Pixel-Perfect: Real Pixel Art|[ai]|true|"),
        "{html}"
    );
    assert!(
        html.contains("looks right") && !html.contains("Before and after"),
        "{html}"
    );
}

// -- through the app ------------------------------------------------------------------------

fn app_config(fake: &Fake, site: &Site) -> bjorn::config::Config {
    bjorn::config::Config {
        // The ledger goes beside the config file: keep both in the temp dir.
        path: Some(site.dir.path().join("config/config.toml")),
        hugo: HugoConfig {
            tag_prefix: "home".into(),
            publish_tag: String::new(),
            ..site.config()
        },
        ..fake.config()
    }
}

#[tokio::test]
async fn p_publishes_through_the_confirm_dialog() {
    let fake = Fake::new();
    let site = Site::new();
    let mut h = fake.harness_with(app_config(&fake, &site), None);
    h.load().await;
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    h.press("P");
    h.until(|app| app.overlay.as_ref().is_some_and(|o| o.name() == "Confirm"))
        .await;
    let screen = h.text();
    assert!(screen.contains("garden-plan.md"), "{screen}");
    assert!(
        screen.contains("URL: /")
            && screen.contains("State: draft")
            && screen.contains("Tags: garden"),
        "{screen}"
    );
    h.press("n");
    assert!(walk(&site.root()).is_empty(), "cancel writes nothing");
    h.press("P");
    h.until(|app| app.overlay.is_some()).await;
    h.press("y");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("garden-plan.md"))
    })
    .await;
    let post = walk(&site.posts()).pop().unwrap();
    let text = read(&post);
    assert!(
        text.starts_with("---\ntitle: \"Garden Plan\"\nslug: \"garden-plan\"\n"),
        "{text}"
    );
    assert!(text.contains("tags:\n  - garden\n"), "{text}");
    assert!(!text.contains("#home/garden") && !text.contains("# Garden Plan"));
    assert!(text.contains("/garden-plan-front-bed.png)"), "{text}");
    let toast = h.app.toast_messages().join(" ");
    assert!(toast.contains("(draft). Upload 1 file from"), "{toast}");
}

#[tokio::test]
async fn the_export_picker_offers_hugo_and_p_refuses_without_a_site() {
    let fake = Fake::new();
    let mut h = fake.harness();
    h.load().await;
    h.press("P");
    assert!(h.app.toast_messages().iter().any(|m| m.contains("[hugo]")));
    assert!(h.app.overlay.is_none());

    let site = Site::new();
    let mut h = fake.harness_with(app_config(&fake, &site), None);
    h.load().await;
    h.press("x");
    assert!(h.text().contains("p Hugo"), "{}", h.text());
    h.press("left");
    h.press("enter");
    h.until(|app| app.overlay.as_ref().is_some_and(|o| o.name() == "Confirm"))
        .await;
    assert!(h.text().contains("sprint-planning.md"));
    h.press("escape");
    h.press("x");
    h.press("p");
    h.until(|app| app.overlay.as_ref().is_some_and(|o| o.name() == "Confirm"))
        .await;
    h.press("y");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("sprint-planning.md (draft)"))
    })
    .await;
}

// -- finding 14: a plan that arrives while another dialog is up ---------------------------------

#[tokio::test]
async fn a_plan_arriving_under_another_dialog_says_so() {
    let fake = Fake::new();
    let site = Site::new();
    let mut h = fake.harness_with(app_config(&fake, &site), None);
    h.load().await;
    h.press("P");
    h.press("?");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("press P again"))
    })
    .await;
    assert_eq!(h.app.overlay.as_ref().map(|o| o.name()), Some("Help"));
}

#[test]
fn a_posts_own_tags_are_not_wiped() {
    let site = Site::new();
    let dir = site.posts().join(month());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pixel-perfect-real-pixel-art.md");
    std::fs::write(
        &path,
        "---\ntitle: x\ntags:\n  - pixel-art\n  - ai\n---\nold\n",
    )
    .unwrap();
    // Not Bjorn's post: its tags stay even though the note has some.
    let plan = site.publish(&site.config(), &note(&["blog/rust"]), BODY);
    assert_eq!(plan.tags, ["pixel-art", "ai"]);
    // Bjorn's now: the note's tags replace them, but no tags never wipe them.
    let plan = site.publish(&site.config(), &note(&["blog/rust"]), BODY);
    assert_eq!(plan.tags, ["rust"]);
    let plan = site.publish(&site.config(), &note(&[]), BODY);
    assert_eq!(plan.tags, ["rust"]);
    // Tags named in the note's own front matter always win.
    let plan = site.publish(
        &site.config(),
        &note(&[]),
        "---\ntags: []\n---\n# T\n\nBody",
    );
    assert!(plan.tags.is_empty());
}

// -- final check: privacy in the title and the note's own values ---------------------------------

#[test]
fn links_in_the_title_never_reach_the_title_or_the_slug() {
    let site = Site::new();
    let n = other("T1", "See [[Secret Note|Plans]] [x](bear://open?id=SECRET)");
    let plan = site
        .plan(
            &site.config(),
            &n,
            "# See [[Secret Note|Plans]] [x](bear://open?id=SECRET)\n\nBody",
        )
        .unwrap();
    assert!(
        plan.text
            .contains("title: \"See Plans x\"\nslug: \"see-plans-x\"\n"),
        "{}",
        plan.text
    );
    assert!(
        !plan.text.contains("SECRET") && !plan.text.contains("bear") && !plan.text.contains("[[")
    );
    let why = site
        .plan(
            &site.config(),
            &other("T2", "x"),
            "# Look [here](file:///Users/me/a.txt)\n\nBody",
        )
        .unwrap_err();
    assert!(why.contains("file://"), "{why}");
}

#[test]
fn the_notes_own_values_get_the_same_privacy_pass() {
    let site = Site::new();
    let why = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\ncover:\n  image: file:///Users/me/secret.png\n---\n# T\n\nBody",
        )
        .unwrap_err();
    assert!(why.contains("file://"), "{why}");
    let why = site
        .plan(
            &site.config(),
            &note(&[]),
            "---\ntags: [\"file:///x\"]\n---\n# T\n\nBody",
        )
        .unwrap_err();
    assert!(why.contains("file://"), "{why}");
    let plan = site
        .plan(
            &site.config(),
            &note(&["private"]),
            "---\ndescription: see bear://x-callback-url/open-note?id=SECRET-ID and [[Other|that]] #private\n\
             summary: \"[this](bear://open?id=SECRET)\"\ncover:\n  alt: \"About [[Secret Note]]\"\n---\n# T\n\nBody",
        )
        .unwrap();
    assert!(
        plan.text.contains("description: \"see and that\""),
        "{}",
        plan.text
    );
    assert!(plan.text.contains("summary: \"this\""), "{}", plan.text);
    assert!(
        plan.text.contains("alt: \"About Secret Note\""),
        "{}",
        plan.text
    );
    assert!(
        !plan.text.contains("SECRET")
            && !plan.text.contains("bear:")
            && !plan.text.contains("private")
    );
}

// -- final check: write-time guards ------------------------------------------------------------

#[test]
fn a_post_edited_between_plan_and_write_is_left_alone() {
    let site = Site::new();
    let first = site.publish(&site.config(), &note(&[]), BODY);
    let plan = site.plan(&site.config(), &note(&[]), BODY).unwrap();
    std::fs::write(&first.target, "edited meanwhile").unwrap();
    let why = hugo::write(&plan).unwrap_err().0;
    assert!(why.contains("changed while the dialog was up"), "{why}");
    assert_eq!(read(&first.target), "edited meanwhile");
    // A file that appeared under a new post's path meanwhile is left alone too.
    let fresh = site
        .plan(&site.config(), &other("N9", "Brand New"), "Body")
        .unwrap();
    std::fs::create_dir_all(fresh.target.parent().unwrap()).unwrap();
    std::fs::write(&fresh.target, "someone else's").unwrap();
    assert!(hugo::write(&fresh).is_err());
    assert_eq!(read(&fresh.target), "someone else's");
}

#[test]
fn a_failed_post_write_after_the_images_records_nothing_in_the_ledger() {
    let site = Site::new();
    let files = HashMap::from([("a.png".to_string(), vec![1u8])]);
    let plan = site
        .plan_at(
            &site.config(),
            &note(&[]),
            "# T\n\nLead\n\n![](a.png)",
            &files,
            NOW,
        )
        .unwrap();
    let dir = plan.target.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = hugo::write(&plan);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert!(plan.media[0].0.exists(), "images go first");
    assert!(!plan.target.exists(), "no post");
    assert!(!site.ledger().exists(), "no ledger entry");
}

// -- final check: a different slug at the computed path, an empty body ---------------------------

#[test]
fn a_file_at_the_path_with_another_slug_is_named_in_the_dialog() {
    let site = Site::new();
    let dir = site.posts().join(month());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pixel-perfect-real-pixel-art.md"),
        "---\ntitle: x\nslug: something-else\ndraft: true\n---\nold\n",
    )
    .unwrap();
    let plan = site.plan(&site.config(), &note(&[]), BODY).unwrap();
    assert_eq!(plan.status, Status::Foreign);
    let q = plan.question();
    assert!(
        q.contains(
            "already exists with the slug “something-else”, not “pixel-perfect-real-pixel-art”"
        ),
        "{q}"
    );
    assert!(plan.url.ends_with("/something-else/"));
}

#[test]
fn an_empty_body_ends_right_after_the_front_matter() {
    let site = Site::new();
    let plan = site.publish(
        &site.config(),
        &note(&[]),
        "# Pixel-Perfect: Real Pixel Art\n\n",
    );
    let text = read(&plan.target);
    assert!(text.ends_with("description: \"\"\n---\n"), "{text:?}");
}
