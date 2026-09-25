//! `contrib/hugo-publish`, the action that writes a note into a Hugo site.
//!
//! The script runs under `python3` against a temp site, the way an action
//! runs it: the note as `$BJORN_NOTE_FILE` (Markdown or a TextBundle) and the
//! `BJORN_NOTE_*` variables. Every test returns quietly when there is no
//! `python3` to run it with.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use bjorn::actions::Action;
use common::Fake;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("contrib/hugo-publish")
}

fn have_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

macro_rules! need_python {
    () => {
        if !have_python() {
            eprintln!("python3 is not on PATH; skipping");
            return;
        }
    };
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The toast: the first line of stdout on success, of stderr otherwise.
    fn first_line(&self) -> &str {
        let text = if self.ok { &self.stdout } else { &self.stderr };
        text.lines().next().unwrap_or("")
    }
}

/// A temp folder with a Hugo site (`site/content`) and room for notes.
struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    fn new() -> Site {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("site/content")).unwrap();
        Site { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("site")
    }

    fn posts(&self) -> PathBuf {
        self.root().join("content/posts")
    }

    /// The note as a Markdown file, as `format = "md"` hands it over.
    fn note(&self, text: &str) -> PathBuf {
        let dir = self.dir.path().join("note");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Note.md");
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The note as a TextBundle, as `format = "textbundle"` hands it over.
    fn bundle(&self, text: &str, assets: &[(&str, &[u8])]) -> PathBuf {
        let dir = self.dir.path().join("note/Note.textbundle");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("text.md"), text).unwrap();
        for (name, bytes) in assets {
            std::fs::write(dir.join("assets").join(name), bytes).unwrap();
        }
        dir
    }

    fn run(&self, note: &Path, id: &str, args: &[&str]) -> Run {
        self.run_with(note, id, args, &[])
    }

    fn run_with(&self, note: &Path, id: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new("python3");
        cmd.arg(script()).arg("--site").arg(self.root()).args(args);
        for key in [
            "BJORN_NOTE_TITLE",
            "BJORN_NOTE_TAGS",
            "BJORN_NOTE_FORMAT",
            "BJORN_ACTION_INPUT",
        ] {
            cmd.env_remove(key);
        }
        cmd.env("BJORN_NOTE_FILE", note).env("BJORN_NOTE_ID", id);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        Run {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Every file under the site, relative to it, sorted.
    fn files(&self) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.push(path.strip_prefix(root).unwrap().display().to_string());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root(), &self.root(), &mut out);
        out.sort();
        out
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root().join(rel)).unwrap()
    }
}

/// The front matter's lines, between the first two `---`.
fn front(post: &str) -> Vec<String> {
    let mut lines = post.lines();
    assert_eq!(lines.next(), Some("---"), "{post}");
    lines
        .take_while(|l| *l != "---")
        .map(str::to_string)
        .collect()
}

/// The front matter's top-level keys, in order.
fn keys(post: &str) -> Vec<String> {
    front(post)
        .iter()
        .filter(|l| !l.starts_with([' ', '\t', '#']) && !l.is_empty())
        .map(|l| l.split(':').next().unwrap().to_string())
        .collect()
}

fn body(post: &str) -> String {
    let rest = post.splitn(3, "---\n").nth(2).unwrap_or("");
    rest.to_string()
}

#[test]
fn the_script_is_executable_python_3() {
    let mode = std::fs::metadata(script()).unwrap().permissions().mode();
    assert!(mode & 0o111 != 0, "contrib/hugo-publish is not executable");
    let text = std::fs::read_to_string(script()).unwrap();
    assert!(text.starts_with("#!/usr/bin/env python3\n"));
}

#[test]
fn a_post_is_a_draft_unless_asked_otherwise_and_the_toast_says_which() {
    need_python!();
    let site = Site::new();
    let note = site.note("# Hello World\n\nFirst post.\n");
    let run = site.run(&note, "NOTE-1", &[]);
    assert!(run.ok, "{}", run.stderr);
    assert_eq!(
        run.first_line(),
        "Draft saved: content/posts/hello-world.md"
    );
    let post = site.read("content/posts/hello-world.md");
    assert!(front(&post).contains(&"draft: true".to_string()), "{post}");
    assert_eq!(
        keys(&post),
        ["title", "slug", "date", "draft", "tags", "bjorn_note"]
    );
    assert!(post.contains("title: \"Hello World\"\n"), "{post}");
    assert_eq!(body(&post), "\nFirst post.\n");

    let run = site.run(&note, "NOTE-1", &["--live"]);
    assert_eq!(
        run.first_line(),
        "Published (live): content/posts/hello-world.md (updated)"
    );
    let post = site.read("content/posts/hello-world.md");
    assert!(front(&post).contains(&"draft: false".to_string()), "{post}");

    // Back to a draft takes a live post down, and says so.
    let run = site.run(&note, "NOTE-1", &["--draft"]);
    assert_eq!(
        run.first_line(),
        "Draft saved: content/posts/hello-world.md (was live; now a draft) (updated)"
    );
}

#[test]
fn ask_publishes_live_only_on_a_typed_yes() {
    need_python!();
    for (answer, live) in [("yes", true), (" YES ", true), ("y", false), ("", false)] {
        let site = Site::new();
        let note = site.note("# Asked\n\nText.\n");
        let run = site.run_with(&note, "N", &["--ask"], &[("BJORN_ACTION_INPUT", answer)]);
        assert!(run.ok, "{}", run.stderr);
        let expected = if live {
            "Published (live): content/posts/asked.md"
        } else {
            "Draft saved: content/posts/asked.md"
        };
        assert_eq!(run.first_line(), expected, "answer {answer:?}");
    }
    let site = Site::new();
    let note = site.note("# Asked\n");
    let run = site.run(&note, "N", &["--ask", "--live"]);
    assert!(!run.ok, "--ask and --live are one choice");
}

#[test]
fn a_slug_folds_accents_and_never_leaves_the_section() {
    need_python!();
    for (title, file) in [
        ("Crème Brûlée & C++: 2026?", "creme-brulee-c-2026.md"),
        ("\u{212A}elvin scale", "kelvin-scale.md"),
        ("../../etc/passwd", "etc-passwd.md"),
    ] {
        let site = Site::new();
        let note = site.note(&format!("# {title}\n\nText.\n"));
        let run = site.run(&note, "N", &[]);
        assert!(run.ok, "{title}: {}", run.stderr);
        assert_eq!(site.files(), [format!("content/posts/{file}")], "{title}");
    }
    // A title with nothing to make a slug of still gets one.
    let site = Site::new();
    let note = site.note("# 日本語\n\nText.\n");
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let files = site.files();
    assert!(
        files.len() == 1 && files[0].starts_with("content/posts/note-"),
        "{files:?}"
    );
    for section in ["../outside", "a/../b", "/", ".hidden/.."] {
        let site = Site::new();
        let note = site.note("# Title\n");
        let run = site.run(&note, "N", &["--section", section]);
        assert!(!run.ok, "section {section:?} was accepted");
        assert!(site.files().is_empty(), "{section:?}");
    }
    // A slug: from the note shapes a new post, cleaned like a title.
    let site = Site::new();
    let note = site.note("---\nslug: \"../Up There\"\n---\n# Title\n");
    assert!(site.run(&note, "N", &[]).ok);
    assert_eq!(site.files(), ["content/posts/up-there.md"]);
}

#[test]
fn a_symlink_under_the_site_is_refused_never_followed() {
    need_python!();
    let site = Site::new();
    let outside = site.dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, site.posts()).unwrap();
    let note = site.note("# Hello\n\nText.\n");
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert!(run.first_line().contains("symlink"), "{}", run.stderr);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

    // The post's own file as a symlink.
    let site = Site::new();
    let target = site.dir.path().join("victim.md");
    std::fs::write(&target, "mine\n").unwrap();
    std::fs::create_dir_all(site.posts()).unwrap();
    std::os::unix::fs::symlink(&target, site.posts().join("hello.md")).unwrap();
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine\n");

    // A bundle folder that is a symlink.
    let site = Site::new();
    let away = site.dir.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    std::fs::create_dir_all(site.posts()).unwrap();
    std::os::unix::fs::symlink(&away, site.posts().join("hello")).unwrap();
    let run = site.run(&note, "N", &["--bundle"]);
    assert!(!run.ok, "{}", run.stdout);
    assert_eq!(std::fs::read_dir(&away).unwrap().count(), 0);

    // content itself.
    let site = Site::new();
    std::fs::remove_dir(site.root().join("content")).unwrap();
    std::os::unix::fs::symlink(&away, site.root().join("content")).unwrap();
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert!(run.first_line().contains("symlink"), "{}", run.stderr);
}

#[test]
fn a_hand_written_post_is_never_replaced() {
    need_python!();
    let site = Site::new();
    std::fs::create_dir_all(site.posts()).unwrap();
    let mine = "---\ntitle: Hello\n---\nI wrote this.\n";
    std::fs::write(site.posts().join("hello.md"), mine).unwrap();
    let note = site.note("# Hello\n\nFrom Bear.\n");
    for args in [&[][..], &["--live"][..]] {
        let run = site.run(&note, "N", args);
        assert!(!run.ok);
        assert_eq!(
            run.first_line(),
            "content/posts/hello.md exists and was not written by this script; refusing to replace it."
        );
    }
    assert_eq!(site.read("content/posts/hello.md"), mine);

    // A bundle of the same name is the same page to Hugo.
    let run = site.run(&note, "N", &["--bundle"]);
    assert!(!run.ok);
    assert!(
        run.first_line().contains("hello.md already exists"),
        "{}",
        run.stderr
    );
    assert_eq!(site.files(), ["content/posts/hello.md"]);
}

#[test]
fn another_notes_post_is_never_replaced() {
    need_python!();
    let site = Site::new();
    let note = site.note("# Hello\n\nOne.\n");
    assert!(site.run(&note, "NOTE-A", &[]).ok);
    let before = site.read("content/posts/hello.md");
    let run = site.run(&note, "NOTE-B", &[]);
    assert!(!run.ok);
    assert_eq!(
        run.first_line(),
        "content/posts/hello.md was written for another note; refusing to replace it."
    );
    assert_eq!(site.read("content/posts/hello.md"), before);
    // The mark is a hash: the note's id itself never reaches the site.
    assert!(!before.contains("NOTE-A"), "{before}");
}

#[test]
fn publishing_again_updates_its_own_post_wherever_it_is() {
    need_python!();
    let site = Site::new();
    let note = site.note("# First Title\n\nOne.\n");
    assert!(site.run(&note, "NOTE-1", &[]).ok);
    let path = "content/posts/first-title.md";
    let first = site.read(path);
    let date = front(&first)
        .into_iter()
        .find(|l| l.starts_with("date: "))
        .unwrap();
    // Keys added by hand stay; so does the post's own date and slug.
    let edited = first.replacen(
        "bjorn_note",
        "# mine\ncanonicalURL: \"https://x.test/\"\nbjorn_note",
        1,
    );
    std::fs::write(site.root().join(path), &edited).unwrap();

    let note = site.note("# A New Title\n\nTwo.\n");
    let run = site.run(&note, "NOTE-1", &[]);
    assert!(run.ok, "{}", run.stderr);
    assert_eq!(run.first_line(), format!("Draft saved: {path} (updated)"));
    assert_eq!(site.files(), [path], "a new title does not move the post");
    let post = site.read(path);
    let lines = front(&post);
    assert!(lines.contains(&date), "{post}");
    assert!(
        lines.contains(&"slug: \"first-title\"".to_string()),
        "{post}"
    );
    assert!(
        lines.contains(&"title: \"A New Title\"".to_string()),
        "{post}"
    );
    assert!(lines.iter().any(|l| l.starts_with("lastmod: ")), "{post}");
    assert!(lines.contains(&"# mine".to_string()), "{post}");
    assert!(
        lines.contains(&"canonicalURL: \"https://x.test/\"".to_string()),
        "{post}"
    );
    assert_eq!(body(&post), "\nTwo.\n");
}

#[test]
fn images_go_beside_a_bundle_and_never_over_an_existing_file() {
    need_python!();
    let site = Site::new();
    let text = "# With Picture\n\n![a bed](assets/Front%20bed.png)\n";
    let note = site.bundle(text, &[("Front bed.png", b"one")]);

    // Without a bundle there is nowhere to put it.
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert!(run.first_line().contains("--bundle"), "{}", run.stderr);
    assert!(site.files().is_empty());

    let run = site.run(&note, "N", &["--bundle"]);
    assert!(run.ok, "{}", run.stderr);
    let dir = "content/posts/with-picture";
    assert_eq!(
        site.files(),
        [format!("{dir}/front-bed.png"), format!("{dir}/index.md")]
    );
    assert!(
        site.read(&format!("{dir}/index.md"))
            .contains("![a bed](front-bed.png)")
    );

    // The same image again is left alone; a changed one gets a new name
    // and the old file is kept.
    assert!(site.run(&note, "N", &["--bundle"]).ok);
    let note = site.bundle(text, &[("Front bed.png", b"two")]);
    let run = site.run(&note, "N", &["--bundle"]);
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout
            .contains("front-bed.png was already there and differs"),
        "{}",
        run.stdout
    );
    assert_eq!(site.read(&format!("{dir}/front-bed.png")), "one");
    assert_eq!(site.read(&format!("{dir}/front-bed-2.png")), "two");
    assert!(
        site.read(&format!("{dir}/index.md"))
            .contains("![a bed](front-bed-2.png)")
    );

    // A file someone else put in the bundle is never overwritten either.
    let site = Site::new();
    std::fs::create_dir_all(site.posts().join("with-picture")).unwrap();
    std::fs::write(site.posts().join("with-picture/front-bed.png"), "theirs").unwrap();
    let note = site.bundle(text, &[("Front bed.png", b"ours")]);
    assert!(site.run(&note, "N", &["--bundle"]).ok);
    assert_eq!(site.read(&format!("{dir}/front-bed.png")), "theirs");
    assert_eq!(site.read(&format!("{dir}/front-bed-2.png")), "ours");
}

#[test]
fn an_image_that_would_break_or_run_is_refused() {
    need_python!();
    for (text, why) in [
        ("# P\n\n![](assets/logo.svg)\n", "not a"),
        ("# P\n\n![](assets/missing.png)\n", "not among"),
        ("# P\n\n![](photo.png)\n", "broken link"),
        ("# P\n\n![](file:///Users/me/p.png)\n", "on this Mac"),
        ("# P\n\n![](data:image/png;base64,AAAA)\n", "data:"),
    ] {
        let site = Site::new();
        let note = site.bundle(text, &[("logo.svg", b"<svg/>")]);
        let run = site.run(&note, "N", &["--bundle"]);
        assert!(!run.ok, "{text}");
        assert!(run.first_line().contains(why), "{text}: {}", run.stderr);
        assert!(site.files().is_empty(), "{text}");
    }
}

#[test]
fn front_matter_from_the_note_cannot_add_keys_or_lines() {
    need_python!();
    let site = Site::new();
    let note = site.note(
        "---\n\
         title: \"Hi\\nurl: /x/\"\n\
         description: \"a: b\\nlayout: evil\\u2028markup: html\"\n\
         summary: *anchor\n\
         aliases: &anchor [/x]\n\
         url: /evil/\n\
         layout: evil\n\
         showtoc: true\n\
         draft: false\n\
         cover:\n  image: \"https://img.test/a.png\\nmarkup: html\"\n  alt: 'it''s'\n\
         ---\n# Hi\n\nBody.\n",
    );
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = site.read("content/posts/hi-url-x.md");
    assert_eq!(
        keys(&post),
        [
            "title",
            "slug",
            "date",
            "draft",
            "tags",
            "description",
            "showtoc",
            "cover",
            "bjorn_managed",
            "bjorn_note"
        ],
        "{post}"
    );
    let lines = front(&post);
    assert!(
        lines.contains(&"bjorn_managed: [\"cover\", \"description\", \"showtoc\"]".to_string()),
        "{post}"
    );
    assert!(
        lines.contains(&"title: \"Hi url: /x/\"".to_string()),
        "{post}"
    );
    assert!(
        lines.contains(&"description: \"a: b layout: evil markup: html\"".to_string()),
        "{post}"
    );
    assert!(
        lines.contains(&"draft: true".to_string()),
        "the note's draft: is ignored: {post}"
    );
    assert!(lines.contains(&"  alt: \"it's\"".to_string()), "{post}");
    assert!(
        lines.contains(&"  image: \"https://img.test/a.png markup: html\"".to_string()),
        "{post}"
    );
    // Every value is one JSON-quoted line, or a boolean, or a list.
    for line in &lines {
        let value = line.split_once(": ").map(|(_, v)| v);
        assert!(
            line == "cover:"
                || value.is_some_and(|v| v.starts_with('"')
                    || v.starts_with('[')
                    || v == "true"
                    || v == "false"),
            "{line}"
        );
    }
    let report = run.stdout;
    for dropped in [
        "aliases",
        "layout",
        "url",
        "summary (not a simple value)",
        "draft",
    ] {
        assert!(report.contains(dropped), "{dropped}: {report}");
    }

    // cover takes only its own fields; a stray one drops the whole key.
    let site = Site::new();
    let note = site.note("---\ncover:\n  image: https://img.test/a.png\n  url: /x/\n---\n# C\n");
    let run = site.run(&note, "N", &[]);
    assert!(run.ok);
    let post = site.read("content/posts/c.md");
    assert!(!keys(&post).contains(&"cover".to_string()), "{post}");
    assert!(!post.contains("url"), "{post}");
}

#[test]
fn front_matter_that_is_not_simple_keys_or_too_big_is_refused() {
    need_python!();
    for text in [
        "---\n\"quoted key\": v\n---\n# T\n".to_string(),
        "---\n  indented: under nothing\ntitle: T\n---\n# T\n".to_string(),
        "---\ntitle: T\ntitle: again\n---\n# T\n".to_string(),
        "---\ntitle: never closes\n# T\n".to_string(),
        format!(
            "---\ndescription: \"{}\"\n---\n# T\n",
            "x".repeat(70 * 1024)
        ),
    ] {
        let site = Site::new();
        let note = site.note(&text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text:.80}");
        assert!(site.files().is_empty());
    }
}

#[test]
fn wiki_links_become_their_text() {
    need_python!();
    let site = Site::new();
    let note = site.note(
        "# Links\n\n\
         See [[Plan [v2]]], [[Note|the alias]], [[Note/Heading]], [[A\\/B/Heading]], [[/Just a heading]].\n\
         [see [[Plan]]](https://example.com) and `[[in code]]`.\n\
         ```\n[[fenced]]\n```\n",
    );
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/links.md"));
    assert!(
        post.contains("See Plan \\[v2\\], the alias, Note/Heading, A/B, Just a heading.\n"),
        "{post}"
    );
    assert!(
        post.contains("[see Plan](https://example.com) and `[[in code]]`."),
        "{post}"
    );
    assert!(post.contains("```\n[[fenced]]\n```"), "{post}");

    // A title in a wiki link is text in the title, and in the slug.
    let site = Site::new();
    let note = site.note("# About [[Plan [v2]|plans]]\n");
    assert!(site.run(&note, "N", &[]).ok);
    let post = site.read("content/posts/about-plans.md");
    assert!(post.contains("title: \"About plans\""), "{post}");
}

#[test]
fn links_to_this_mac_or_other_apps_lose_their_target() {
    need_python!();
    let site = Site::new();
    let note = site.note(
        "# Links\n\n\
         [a](bear://x-callback-url/open-note?id=ABC) [b](file:///tmp/x) [c](/Users/me/doc.txt) \
         [d](~/notes/x.md) [e](things:///show?id=1) [f](obsidian://open?vault=v) \
         [g](https://example.com) [h](/about/) [i](#top) [j](mailto:me@example.com)\n\
         Bare bear://open?id=1 and <x-devonthink-item://abc> go.\n\
         [ref]: bear://x-callback-url/open-note?id=ABC\n\
         [web]: https://example.com\n",
    );
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/links.md"));
    assert!(
        post.contains(
            "a b c d e f [g](https://example.com) [h](/about/) [i](#top) [j](mailto:me@example.com)"
        ),
        "{post}"
    );
    for gone in [
        "bear:",
        "file:",
        "/Users",
        "~/",
        "things:",
        "obsidian:",
        "devonthink",
        "[ref]:",
    ] {
        assert!(!post.contains(gone), "{gone} survived: {post}");
    }
    assert!(post.contains("[web]: https://example.com"), "{post}");

    // Code is published as written, so a local detail in it is refused.
    for text in [
        "# C\n\n```\ncat /Users/me/secret\n```\n",
        "# C\n\nRun `open bear://x-callback-url/open-note?id=A`.\n",
        "# C\n\nMy home is /Users/me.\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}");
        assert!(site.files().is_empty(), "{text}");
    }
}

#[test]
fn only_tags_under_the_prefix_are_published() {
    need_python!();
    let site = Site::new();
    let note = site.note(
        "# Tagged\n#blog/rust #private/diary\n\nA line about #private/diary and (#blog/rust).\n\
         #private/diary\n\n```\n#private/diary stays in code\n```\n",
    );
    let tags = "blog/rust,blog/deep,blog/deep/Image Processing,private/diary,blog";
    let run = site.run_with(
        &note,
        "N",
        &["--tag-prefix", "blog/"],
        &[("BJORN_NOTE_TAGS", tags)],
    );
    assert!(run.ok, "{}", run.stderr);
    let post = site.read("content/posts/tagged.md");
    assert!(
        front(&post).contains(&"tags: [\"rust\", \"deep-image-processing\"]".to_string()),
        "{post}"
    );
    let text = body(&post);
    assert_eq!(
        text,
        "\nA line about and.\n\n```\n#private/diary stays in code\n```\n"
    );

    // No prefix, no tags at all.
    let site = Site::new();
    let note = site.note("# Tagged\n#blog/rust\n\nText.\n");
    let run = site.run_with(&note, "N", &[], &[("BJORN_NOTE_TAGS", "blog/rust")]);
    assert!(run.ok);
    let post = site.read("content/posts/tagged.md");
    assert!(front(&post).contains(&"tags: []".to_string()), "{post}");
}

/// The Kelvin sign lower-cases to a shorter `k`; the built-in once sliced
/// the text by the lowered tag's byte length and panicked.
#[test]
fn a_tag_that_changes_length_when_lowered_is_still_taken_out() {
    need_python!();
    let site = Site::new();
    let note = site.note("# Heat\n\nIn \u{212A}elvin: #\u{212A}elvin/scale and #blog/\u{212A}.\n");
    let run = site.run_with(
        &note,
        "N",
        &["--tag-prefix", "blog"],
        &[("BJORN_NOTE_TAGS", "\u{212A}elvin/scale,blog/\u{212A}")],
    );
    assert!(run.ok, "{}", run.stderr);
    let post = site.read("content/posts/heat.md");
    assert!(
        front(&post).contains(&"tags: [\"k\"]".to_string()),
        "{post}"
    );
    assert_eq!(body(&post), "\nIn \u{212A}elvin: and.\n");
}

#[test]
fn shortcodes_and_raw_html_are_refused_unless_allowed() {
    need_python!();
    for text in [
        "# S\n\n{{< youtube abc >}}\n",
        "# S\n\n```\n{{% param x %}}\n```\n",
        "# S\n\n<script>alert(1)</script>\n",
        "# S\n\nInline <span style=\"x\">html</span>.\n",
        "# S\n\n<!-- hidden -->\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}");
        assert!(
            run.first_line().contains("--allow-html"),
            "{text}: {}",
            run.stderr
        );
        assert!(site.files().is_empty());
        let run = site.run(&note, "N", &["--allow-html"]);
        assert!(run.ok, "{text}: {}", run.stderr);
    }
    // The summary divider and autolinks are not HTML.
    let site = Site::new();
    let note = site.note("# S\n\nLead.\n\n<!--more-->\n\n<https://example.com> <me@example.com>\n");
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    assert!(site.read("content/posts/s.md").contains("<!--more-->"));
}

#[test]
fn a_refusal_writes_nothing_and_says_why_on_one_line() {
    need_python!();
    let site = Site::new();
    let note = site.note("# Hello\n\n{{< x >}}\n");
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
    assert!(run.stdout.is_empty());
    assert!(site.files().is_empty());
    // Not a Hugo site, not an action, not a format it reads.
    let run = Site::new().run_with(
        &note,
        "N",
        &["--section", "posts"],
        &[("BJORN_NOTE_FORMAT", "html")],
    );
    assert!(run.first_line().contains("textbundle"), "{}", run.stderr);
    let empty = tempfile::tempdir().unwrap();
    let plain = site.note("# Plain\n\nText.\n");
    let out = Command::new("python3")
        .arg(script())
        .arg("--site")
        .arg(empty.path())
        .env("BJORN_NOTE_FILE", &plain)
        .env("BJORN_NOTE_ID", "N")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("no content folder"));
    let out = Command::new("python3")
        .arg(script())
        .arg("--site")
        .arg(site.root())
        .env_remove("BJORN_NOTE_FILE")
        .env_remove("BJORN_NOTE_ID")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("Bjorn action"));
    assert!(site.files().is_empty());
}

/// What a post's body says, or None when the run was refused.
fn published(site: &Site, run: &Run, rel: &str) -> Option<String> {
    run.ok.then(|| site.read(rel))
}

#[test]
fn html_or_a_shortcode_left_behind_by_a_removed_link_is_refused() {
    need_python!();
    // Taking a link or a bare address out once joined what was on either
    // side into a shortcode or a tag.
    for text in [
        "# S\n\n{{[](x:y)< figure src=\"https://evil.test/x\" >}}\n",
        "# S\n\n{{x://y< param \"k\" >}}\n",
        "# S\n\n{{x://y% param \"k\" %}}\n",
        "# S\n\n<[](x:y)img src=x onerror=alert(1)>\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        if let Some(post) = published(&site, &run, "content/posts/s.md") {
            for bad in ["{{<", "{{%", "<img"] {
                assert!(!post.contains(bad), "{text}: {post}");
            }
        }
    }
    // What the cleaning leaves is checked again: link text that makes a
    // shortcode, and a wiki link's text that makes a tag name.
    for (text, why) in [
        ("# S\n\n{[{](x:y)< x >}}\n", "shortcode"),
        ("# S\n\n<[[img]] src=x onerror=alert(1)>\n", "raw HTML"),
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}: {}", run.stdout);
        assert!(run.first_line().contains(why), "{text}: {}", run.stderr);
        assert!(run.first_line().contains("--allow-html"), "{}", run.stderr);
        assert!(site.files().is_empty(), "{text}");
    }
}

#[test]
fn markdown_that_hides_html_from_a_line_scanner_is_refused() {
    need_python!();
    for text in [
        // A backslash makes the first backtick text, so this is no code span.
        "# S\n\n\\`<img src=x onerror=alert(1)>`\n",
        // A backtick fence's info string cannot hold a backtick: no fence.
        "# S\n\n```a`\n<img src=x onerror=alert(1)>\n```\n",
        // Indented four or more, a fence is none.
        "# S\n\n        ```\n<img src=x onerror=alert(1)>\n        ```\n",
        // A code span runs over lines within a paragraph.
        "# S\n\n`a\nb` <img src=x onerror=alert(1)> `c`\n",
        // A fence in a list item ends with the item.
        "# S\n\n- ```\n  x\n  ```\n  <img src=x onerror=alert(1)>\n",
        "# S\n\n- item\n  ```\n<img src=x onerror=alert(1)>\n```\n",
        "# S\n\n- a\n\n    ```\n    x\n  ```\n  <img src=x onerror=alert(1)>\n",
        // Inline code gets no pass for a tag that could run.
        "# S\n\nInline `<img src=x onerror=alert(1)>` code.\n",
        "# S\n\nA `<script>` mention.\n",
        // An autolink is read before a code span.
        "# S\n\n<https://x.test/`> <img src=x onerror=alert(1)> `\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}: {}", run.stdout);
        assert!(
            run.first_line().contains("--allow-html"),
            "{text}: {}",
            run.stderr
        );
        assert!(site.files().is_empty(), "{text}");
        assert!(site.run(&note, "N", &["--allow-html"]).ok, "{text}");
    }
}

#[test]
fn inline_code_keeps_harmless_tags_and_fenced_code_keeps_html() {
    need_python!();
    let site = Site::new();
    let text = "# Code\n\n\
                Use `Vec<String>`, `Box<dyn Error>` and `<div>`; see <https://example.com>.\n\n\
                ```html\n<div onclick=\"x()\">top level</div>\n```\n\n\
                1. Step\n\n   ```html\n   <script>in a list</script>\n   ```\n2. Next\n";
    let note = site.note(text);
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/code.md"));
    assert!(
        post.contains("`Vec<String>`, `Box<dyn Error>` and `<div>`"),
        "{post}"
    );
    assert!(post.contains("<script>in a list</script>"), "{post}");
}

#[test]
fn html_split_over_lines_and_app_autolinks_are_caught() {
    need_python!();
    for text in [
        "# S\n\n<details\nopen ontoggle=alert(1)>\n",
        "# S\n\nx <img\nsrc=x onerror=alert(1)>\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}: {}", run.stdout);
        assert!(run.first_line().contains("--allow-html"), "{}", run.stderr);
    }
    // An autolink to another app loses its target in prose ...
    let site = Site::new();
    let note = site.note(
        "# S\n\nGo <javascript:alert(1)> and <things:show?id=ABC>, or <https://example.com>.\n",
    );
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/s.md"));
    assert!(
        !post.contains("javascript") && !post.contains("things:"),
        "{post}"
    );
    assert!(post.contains("<https://example.com>"), "{post}");
    // ... and is refused where it cannot be taken out, --allow-html or not.
    for text in [
        "# S\n\nIn code `<things:show?id=ABC>`.\n",
        "---\ndescription: \"<things:show?id=ABC>\"\n---\n# S\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        for args in [&[][..], &["--allow-html"][..]] {
            let run = site.run(&note, "N", args);
            assert!(!run.ok, "{text}: {}", run.stdout);
        }
        assert!(site.files().is_empty(), "{text}");
    }
}

#[test]
fn a_new_post_never_shares_a_name_with_a_page_hugo_already_has() {
    need_python!();
    // `index` is a folder's own page, never a post's name.
    for text in ["# Index\n\nText.\n", "---\nslug: _index\n---\n# Home\n"] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}");
        assert!(run.first_line().contains("index"), "{}", run.stderr);
        assert!(site.files().is_empty());
    }
    let note_text = "# Hello\n\nText.\n";
    // Beside a single file: another format, a translation, a case variant
    // or a folder of the same name.
    for (existing, is_dir) in [
        ("hello.html", false),
        ("hello.en.md", false),
        ("Hello.MD", false),
        ("hello", true),
    ] {
        let site = Site::new();
        std::fs::create_dir_all(site.posts()).unwrap();
        if is_dir {
            std::fs::create_dir_all(site.posts().join(existing)).unwrap();
            std::fs::write(site.posts().join(existing).join("_index.md"), "x").unwrap();
        } else {
            std::fs::write(site.posts().join(existing), "x").unwrap();
        }
        let before = site.files();
        let note = site.note(note_text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{existing}: {}", run.stdout);
        assert!(
            run.first_line().contains("already exists"),
            "{existing}: {}",
            run.stderr
        );
        assert_eq!(site.files(), before, "{existing}");
    }
    // A bundle into a folder that holds a page, or a section's posts.
    for inside in ["_index.md", "index.en.md", "other-post.md", "sub/"] {
        let site = Site::new();
        let dir = site.posts().join("hello");
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(sub) = inside.strip_suffix('/') {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        } else {
            std::fs::write(dir.join(inside), "x").unwrap();
        }
        let before = site.files();
        let note = site.note(note_text);
        let run = site.run(&note, "N", &["--bundle"]);
        assert!(!run.ok, "{inside}: {}", run.stdout);
        assert!(
            run.first_line().contains("already holds"),
            "{inside}: {}",
            run.stderr
        );
        assert_eq!(site.files(), before, "{inside}");
    }
    // Its own post, bundle or single file, is still updated in place.
    for args in [&["--bundle"][..], &[][..]] {
        let site = Site::new();
        let note = site.note(note_text);
        assert!(site.run(&note, "N", args).ok);
        let run = site.run(&note, "N", args);
        assert!(run.ok, "{args:?}: {}", run.stderr);
        assert!(run.first_line().ends_with("(updated)"), "{}", run.stdout);
    }
}

#[test]
fn front_matter_values_are_held_to_the_body_rules() {
    need_python!();
    for (text, html) in [
        (
            "---\ndescription: \"<img src=x onerror=alert(1)>\"\n---\n# T\n",
            true,
        ),
        ("---\nsummary: \"{{< x >}}\"\n---\n# T\n", true),
        (
            "---\ncover:\n  image: https://img.test/a.png\n  caption: \"<b onclick=x>c</b>\"\n---\n# T\n",
            true,
        ),
        ("# A <b>bold</b> title\n\nText.\n", true),
        (
            "---\nsummary: \"[x](x:y)<img src=x onerror=alert(1)>\"\n---\n# T\n",
            true,
        ),
        ("---\ndescription: \"see <bear:abc>\"\n---\n# T\n", false),
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}: {}", run.stdout);
        assert!(site.files().is_empty(), "{text}");
        let run = site.run(&note, "N", &["--allow-html"]);
        assert_eq!(run.ok, html, "{text}: {}", run.stderr);
    }
}

#[test]
fn a_value_the_note_no_longer_gives_leaves_the_post() {
    need_python!();
    let site = Site::new();
    let path = "content/posts/t.md";
    let note = site.note("---\ndescription: \"From the note\"\nsummary: \"Kept\"\n---\n# T\n");
    assert!(site.run(&note, "N", &[]).ok);
    let post = site.read(path);
    assert!(
        front(&post).contains(&"bjorn_managed: [\"description\", \"summary\"]".to_string()),
        "{post}"
    );
    let edited = post.replacen("bjorn_note", "keywords: \"mine\"\nbjorn_note", 1);
    std::fs::write(site.root().join(path), edited).unwrap();

    let note = site.note("---\nsummary: \"Kept\"\n---\n# T\n");
    assert!(site.run(&note, "N", &[]).ok);
    let post = site.read(path);
    let lines = front(&post);
    assert!(!post.contains("From the note"), "{post}");
    assert!(lines.contains(&"summary: \"Kept\"".to_string()), "{post}");
    assert!(lines.contains(&"keywords: \"mine\"".to_string()), "{post}");
    assert!(
        lines.contains(&"bjorn_managed: [\"summary\"]".to_string()),
        "{post}"
    );

    // A value set by hand, for a key the note never gave, stays.
    let edited = post.replacen("bjorn_note", "description: \"By hand\"\nbjorn_note", 1);
    std::fs::write(site.root().join(path), edited).unwrap();
    assert!(site.run(&note, "N", &[]).ok);
    assert!(site.read(path).contains("description: \"By hand\""));
}

#[test]
fn link_targets_written_to_slip_past_the_cleaner_are_caught() {
    need_python!();
    let site = Site::new();
    let note = site.note(
        "# L\n\n\
         [a](< things:abc>) [b](&#47;Users/ventz/a) [c](&#116;hings:abc) [d](thi&#9;ngs:abc) \
         [e](/users/ventz/a)\n\n\
         - [f]: things:abc\n\
         > [g]: /Volumes/Secret/a\n\n\
         Plain `GET /users/42` stays.\n",
    );
    let run = site.run(&note, "N", &[]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/l.md"));
    assert!(post.contains("a b c d e\n"), "{post}");
    for gone in [
        "things",
        "Users",
        "users/ventz",
        "Volumes",
        "&#",
        "[f]",
        "[g]",
    ] {
        assert!(!post.contains(gone), "{gone}: {post}");
    }
    assert!(post.contains("`GET /users/42`"), "{post}");

    for text in [
        "# L\n\n[x](\nthings:show?id=1)\n",
        "# L\n\n[x]:\nthings:abc\n",
        "# L\n\nsee /users/ventz/a\n",
    ] {
        let site = Site::new();
        let note = site.note(text);
        let run = site.run(&note, "N", &[]);
        assert!(!run.ok, "{text}: {}", run.stdout);
        assert!(site.files().is_empty(), "{text}");
    }
}

#[test]
fn a_private_tag_in_a_wiki_links_text_stays_home() {
    need_python!();
    let site = Site::new();
    let note = site.note("# W\n\nSee [[Plan #private]] and [[#private]].\n");
    let run = site.run_with(&note, "N", &[], &[("BJORN_NOTE_TAGS", "private")]);
    assert!(run.ok, "{}", run.stderr);
    let post = body(&site.read("content/posts/w.md"));
    assert!(!post.contains("private"), "{post}");
    assert!(post.contains("See Plan and"), "{post}");
}

/// An Exif block (big-endian TIFF) with or without a GPS position.
fn exif(gps: bool) -> Vec<u8> {
    let mut t = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01".to_vec();
    if gps {
        // GPSInfo -> the IFD at 26, holding GPSLatitude.
        t.extend([0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0]);
        t.extend([0, 1, 0, 2, 0, 5, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
    } else {
        // Orientation only.
        t.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
    }
    t
}

fn jpeg(gps: bool) -> Vec<u8> {
    let tiff = exif(gps);
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(u16::try_from(8 + tiff.len()).unwrap().to_be_bytes());
    out.extend(b"Exif\x00\x00");
    out.extend(tiff);
    out.extend([0xFF, 0xD9]);
    out
}

fn png(gps: bool) -> Vec<u8> {
    let tiff = exif(gps);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(u32::try_from(tiff.len()).unwrap().to_be_bytes());
    out.extend(b"eXIf");
    out.extend(tiff);
    out.extend([0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend(b"IEND\x00\x00\x00\x00");
    out
}

#[test]
fn an_image_with_a_gps_location_is_refused_unless_allowed() {
    need_python!();
    for (name, bytes) in [("photo.jpg", jpeg(true)), ("photo.png", png(true))] {
        let site = Site::new();
        let note = site.bundle(&format!("# P\n\n![](assets/{name})\n"), &[(name, &bytes)]);
        let run = site.run(&note, "N", &["--bundle"]);
        assert!(!run.ok, "{name}");
        assert!(run.first_line().contains("GPS"), "{}", run.stderr);
        assert!(site.files().is_empty());
        let run = site.run(&note, "N", &["--bundle", "--allow-exif"]);
        assert!(run.ok, "{name}: {}", run.stderr);
    }
    for (name, bytes) in [("photo.jpg", jpeg(false)), ("photo.png", png(false))] {
        let site = Site::new();
        let note = site.bundle(&format!("# P\n\n![](assets/{name})\n"), &[(name, &bytes)]);
        let run = site.run(&note, "N", &["--bundle"]);
        assert!(run.ok, "{name}: {}", run.stderr);
    }
}

#[test]
fn notes_built_to_be_slow_are_refused_or_finish_quickly() {
    need_python!();
    // One line over 64 KB is refused before any work is done on it.
    let site = Site::new();
    let note = site.note(&format!("# Long\n\n{}\n", "x".repeat(70 * 1024)));
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert!(run.first_line().contains("64 KB"), "{}", run.stderr);

    let line = |unit: &str| unit.repeat(60 * 1024 / unit.len());
    let backticks: String = (1..300).map(|n| "`".repeat(n) + " x ").collect();
    for (what, text) in [
        ("tags", line("x #tag ")),
        ("scheme runs", line("a+")),
        ("brackets", line("[a")),
        ("backtick runs", backticks),
        ("wiki", line("[[a")),
    ] {
        let site = Site::new();
        let many = format!("{text}\n").repeat(40);
        let note = site.note(&format!("# Slow\n\n{many}"));
        let start = std::time::Instant::now();
        let run = site.run_with(&note, "N", &[], &[("BJORN_NOTE_TAGS", "tag")]);
        let took = start.elapsed();
        assert!(
            took < std::time::Duration::from_secs(20),
            "{what} took {took:?}: {}",
            run.stderr
        );
    }

    // More images than it publishes.
    let site = Site::new();
    let names: Vec<String> = (0..201).map(|n| format!("i{n}.png")).collect();
    let text: String = names.iter().map(|n| format!("![](assets/{n})\n")).collect();
    let assets: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let note = site.bundle(&format!("# Many\n\n{text}"), &assets);
    let run = site.run(&note, "N", &["--bundle"]);
    assert!(!run.ok);
    assert!(run.first_line().contains("at most 200"), "{}", run.stderr);
    assert!(site.files().is_empty());
}

#[test]
fn a_date_the_clock_cannot_hold_is_a_refusal_not_a_crash() {
    need_python!();
    for date in ["0001-01-01", "9999-12-31T23:59:59-23:59"] {
        let site = Site::new();
        let note = site.note(&format!("---\ndate: {date}\n---\n# D\n"));
        let run = site.run(&note, "N", &[]);
        if !run.ok {
            assert_eq!(run.stderr.lines().count(), 1, "{date}: {}", run.stderr);
            assert!(!run.stderr.contains("Traceback"), "{}", run.stderr);
        }
    }
    let site = Site::new();
    let note = site.note("---\ndate: 0001-01-01\n---\n# D\n");
    let run = site.run(&note, "N", &[]);
    assert!(!run.ok);
    assert!(run.first_line().contains("not a date"), "{}", run.stderr);
}

// -- through Bjorn's own action machinery --------------------------------------------------

fn shell_quote(value: &Path) -> String {
    format!("'{}'", value.display().to_string().replace('\'', r"'\''"))
}

#[tokio::test]
async fn an_action_saves_the_note_as_a_draft_bundle() {
    need_python!();
    let fake = Fake::new();
    let site = Site::new();
    let config = bjorn::config::Config {
        actions: vec![Action {
            name: "Hugo: save draft".into(),
            command: format!(
                "{} --site {} --bundle --tag-prefix home",
                shell_quote(&script()),
                shell_quote(&site.root())
            ),
            format: "textbundle".into(),
            default: true,
            ..Action::default()
        }],
        ..fake.config()
    };
    let mut h = fake.harness_with(config, None);
    h.load().await;
    h.press("j");
    h.until(|app| {
        app.reader
            .note
            .as_ref()
            .is_some_and(|n| n.id == "NOTE-GARDEN")
    })
    .await;
    h.press("!");
    h.until(|app| {
        app.toast_messages()
            .iter()
            .any(|m| m.contains("content/posts/garden-plan/index.md"))
    })
    .await;
    let toasts = h.app.toast_messages().join(" | ");
    assert!(
        toasts.contains("Draft saved: content/posts/garden-plan/index.md"),
        "{toasts}"
    );
    assert_eq!(
        site.files(),
        [
            "content/posts/garden-plan/front-bed.png",
            "content/posts/garden-plan/index.md"
        ]
    );
    let post = site.read("content/posts/garden-plan/index.md");
    let lines = front(&post);
    assert!(
        lines.contains(&"title: \"Garden Plan\"".to_string()),
        "{post}"
    );
    assert!(lines.contains(&"tags: [\"garden\"]".to_string()), "{post}");
    assert!(lines.contains(&"draft: true".to_string()), "{post}");
    assert!(
        !post.contains("#home/garden") && !post.contains("# Garden Plan"),
        "{post}"
    );
    assert!(post.contains("![](front-bed.png)"), "{post}");
    assert!(!post.contains("NOTE-GARDEN"), "{post}");
}
