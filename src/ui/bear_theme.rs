//! Themes in Bear's `.theme` format, read from a directory the user owns:
//! `~/.config/bjorn/themes/` (see `theme::themes_dir`).
//!
//! A theme file is JSON: sections (`base`, `sidebar`, `notes`, `editor`)
//! holding `"… color": "#RRGGBB"` values or `"$section.key"` references to
//! other values. A file may name another theme under `meta."base theme"`; its
//! values are laid over that theme's before any reference is resolved, so a
//! base theme's `$base.accent color` picks up the child's accent.
//!
//! Each file becomes one `Theme`, named after the file (`Rosé Pine.theme` is
//! `rose-pine`). The mapping onto Bjorn's palette is the one
//! `tools/bear_theme.py` uses to generate `palettes.rs`, so a theme file
//! dropped in the directory draws exactly as it would built in; the test
//! `repo_theme_files_match_the_built_in_palettes` holds the two together.
//!
//! Loading only reads: files are opened with `read(true)` alone, only regular
//! `*.theme` files at the top of the directory are touched (no symlinks, no
//! recursion), and each is capped at `MAX_BYTES`. A broken file is skipped.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::ui::theme::Theme;

/// References and `base theme` chains deeper than this are treated as broken.
const MAX_DEPTH: usize = 16;

/// Bear's theme files are about 5 KB; anything past this is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// The name a theme file goes by: lowercase, words joined by `-`, accents
/// folded away (`Rosé Pine` is `rose-pine`, `D.Boring` is `d-boring`). macOS
/// stores file names decomposed (`e` + U+0301) while a typed `é` is one
/// character, so folding both to `e` is what lets the two meet.
pub fn slug(name: &str) -> String {
    let folded: String = name
        .to_lowercase()
        .chars()
        .filter(|c| !('\u{300}'..='\u{36F}').contains(c))
        .filter_map(|c| {
            let c = match c {
                'à'..='å' => 'a',
                'ç' => 'c',
                'è'..='ë' => 'e',
                'ì'..='ï' => 'i',
                'ñ' => 'n',
                'ò'..='ö' | 'ø' => 'o',
                'ù'..='ü' => 'u',
                'ý' | 'ÿ' => 'y',
                c => c,
            };
            match c {
                c if c.is_ascii_alphanumeric() => Some(c),
                c if c.is_ascii() => Some(' '),
                _ => None,
            }
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Every theme in `dir` that parses, sorted by name. A missing directory or a
/// broken file is skipped rather than reported: these are extras on top of
/// the built-in themes.
pub fn load_dir(dir: &Path) -> Vec<Theme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut raw: HashMap<String, Value> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("theme") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(value @ Value::Object(_)) = read_json(&path) {
            raw.insert(stem.to_string(), value);
        }
    }

    let mut themes: Vec<Theme> = raw
        .keys()
        .filter_map(|name| to_theme(&slug(name), &merged(&raw, name, 0)?))
        .collect();
    themes.sort_by(|a, b| a.name.cmp(b.name));
    themes.dedup_by(|a, b| a.name == b.name);
    themes
}

/// One theme file, opened read-only. A symlink, a non-file or an oversized
/// file is refused before it is opened.
fn read_json(path: &Path) -> Option<Value> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut text = String::new();
    OpenOptions::new()
        .read(true)
        .open(path)
        .ok()?
        .take(MAX_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// `name`'s values laid over its `base theme`'s, recursively. The base is
/// looked up in the same directory, by file name or by slug.
fn merged(raw: &HashMap<String, Value>, name: &str, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let own = raw.get(name).or_else(|| {
        let wanted = slug(name);
        raw.iter().find(|(k, _)| slug(k) == wanted).map(|(_, v)| v)
    })?;
    match own.pointer("/meta/base theme").and_then(Value::as_str) {
        Some(parent) if parent != name => Some(overlay(&merged(raw, parent, depth + 1)?, own)),
        _ => Some(own.clone()),
    }
}

/// `over` laid on `base`: objects merge key by key, anything else replaces.
fn overlay(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out: Map<String, Value> = b.clone();
            for (key, value) in o {
                let next = match b.get(key) {
                    Some(existing) => overlay(existing, value),
                    None => value.clone(),
                };
                out.insert(key.clone(), next);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

type Rgb = (u8, u8, u8);

/// The colour at a dotted `section.key` path, following `$` references.
fn lookup(root: &Value, path: &str, depth: usize) -> Option<Rgb> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut node = root;
    for segment in path.split('.') {
        node = node.get(segment)?;
    }
    let text = node.as_str()?.trim();
    match text.strip_prefix('$') {
        Some(reference) => lookup(root, reference, depth + 1),
        None => parse_hex(text),
    }
}

/// `#RGB`, `#RRGGBB`, or `#RRGGBBAA` with the alpha dropped.
fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.is_ascii() {
        return None;
    }
    let digits = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits[..6].to_string(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

fn color((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

// The colour arithmetic below mirrors `tools/bear_theme.py` step for step,
// down to Python's round-half-to-even, so both produce the same bytes.

/// `a` moved a fraction `t` of the way to `b`.
fn blend(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let mix = |x: u8, y: u8| {
        let (x, y) = (f64::from(x), f64::from(y));
        (x + (y - x) * t).round_ties_even() as u8
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// WCAG relative luminance, 0 (black) to 1 (white).
fn luminance((r, g, b): Rgb) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a) + 0.05, luminance(b) + 0.05);
    if x > y { x / y } else { y / x }
}

/// The candidate that reads best over `bg` (the first, on a tie).
fn best(bg: Rgb, candidates: &[Rgb]) -> Rgb {
    candidates.iter().copied().fold(candidates[0], |kept, c| {
        if contrast(c, bg) > contrast(kept, bg) {
            c
        } else {
            kept
        }
    })
}

/// Text over `bg`: the theme's own colour that reads best, when one reads at
/// 3:1; plain white or near-black otherwise.
fn on(bg: Rgb, candidates: &[Rgb]) -> Rgb {
    let pick = best(bg, candidates);
    if contrast(pick, bg) >= 3.0 {
        pick
    } else {
        best(bg, &[(0xFF, 0xFF, 0xFF), (0x11, 0x11, 0x11)])
    }
}

/// `fg` pushed toward `toward` until it reads at 3:1 on `bg`.
fn legible(fg: Rgb, bg: Rgb, toward: Rgb) -> Rgb {
    let mut t = 0.0;
    while contrast(blend(fg, toward, t), bg) < 3.0 && t < 1.0 {
        t += 0.05;
    }
    blend(fg, toward, t)
}

/// A highlighter background's hue as a foreground: same hue, full enough
/// saturation, and a lightness that sits on the page. Python's `colorsys`.
fn recolor((r, g, b): Rgb, dark: bool) -> Rgb {
    let (r, g, b) = (
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    );
    let (maxc, minc) = (r.max(g).max(b), r.min(g).min(b));
    let (sumc, rangec) = (maxc + minc, maxc - minc);
    let (h, s) = if minc == maxc {
        (0.0, 0.0)
    } else {
        let s = if sumc / 2.0 <= 0.5 {
            rangec / sumc
        } else {
            rangec / (2.0 - maxc - minc)
        };
        let (rc, gc, bc) = (
            (maxc - r) / rangec,
            (maxc - g) / rangec,
            (maxc - b) / rangec,
        );
        let h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        ((h / 6.0).rem_euclid(1.0), s)
    };
    let l: f64 = if dark { 0.68 } else { 0.36 };
    let s = s.max(0.45);
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let m1 = 2.0 * l - m2;
    let v = |hue: f64| {
        let hue = hue.rem_euclid(1.0);
        if hue < 1.0 / 6.0 {
            m1 + (m2 - m1) * hue * 6.0
        } else if hue < 0.5 {
            m2
        } else if hue < 2.0 / 3.0 {
            m1 + (m2 - m1) * (2.0 / 3.0 - hue) * 6.0
        } else {
            m1
        }
    };
    let byte = |x: f64| (x * 255.0).round_ties_even() as u8;
    (byte(v(h + 1.0 / 3.0)), byte(v(h)), byte(v(h - 1.0 / 3.0)))
}

/// Map a merged theme file onto Bjorn's palette. Only the page background,
/// the text and the accent are required; a hand-written file that leaves the
/// rest out gets the nearest colour it does have.
fn to_theme(name: &str, root: &Value) -> Option<Theme> {
    let get = |path: &str| lookup(root, path, 0);
    let bg = get("base.background color")?;
    let text = get("base.text color")?;
    let accent = get("base.accent color")?;
    let or = |path: &str, fallback: Rgb| get(path).unwrap_or(fallback);

    let dark = luminance(bg) < 0.5;
    let muted = or("base.text secondary color", text);
    let bg2 = or("base.background secondary color", bg);
    let sb_bg = or("sidebar.background color", bg2);
    let sb_text = or("sidebar.text color", text);
    let headers = or("editor.headers.text color", text);
    let blur_bg = or("notes.selection background color", bg2);
    let sb_blur_bg = or("sidebar.background secondary color", sb_bg);
    let sb_text_2 = or("sidebar.text secondary color", sb_text);
    let over_accent = on(accent, &[bg, text, sb_bg]);

    let highlighter = |hue: &str| {
        get(&format!("editor.highlighter.{hue}.background color")).map(|c| recolor(c, dark))
    };
    let fixed = if dark {
        [(0x6F, 0xB9, 0x8F), (0xE0, 0xA4, 0x58), (0xE0, 0x5C, 0x5C)]
    } else {
        [(0x3F, 0x9D, 0x63), (0xB7, 0x79, 0x1F), (0xC0, 0x39, 0x2B)]
    };
    let [success, warning, error] = [
        highlighter("green").unwrap_or(fixed[0]),
        highlighter("yellow").unwrap_or(fixed[1]),
        highlighter("red").unwrap_or(fixed[2]),
    ]
    .map(|c| legible(c, bg, text));

    Some(Theme {
        name: Box::leak(name.to_string().into_boxed_str()),
        dark,

        background: color(bg),
        surface: color(bg),
        surface_focus: color(blend(bg, text, 0.04)),
        sidebar_bg: color(sb_bg),
        sidebar_focus: color(blend(sb_bg, sb_text, 0.04)),

        foreground: color(text),
        muted: color(muted),
        sidebar_fg: color(sb_text),
        sidebar_muted: color(blend(sb_text, sb_bg, 0.35)),

        border: color(or("base.stroke color", muted)),
        sidebar_border: color(or("sidebar.stroke color", sb_bg)),
        header_bg: color(bg2),
        header_fg: color(headers),
        sidebar_header_bg: color(blend(sb_bg, sb_text, 0.06)),
        sidebar_header_fg: color(sb_text),
        header_focus_bg: color(accent),
        header_focus_fg: color(over_accent),
        cursor_bg: color(accent),
        cursor_fg: color(over_accent),
        cursor_blur_bg: color(blur_bg),
        cursor_blur_fg: color(on(blur_bg, &[text, headers])),
        sidebar_cursor_blur_bg: color(sb_blur_bg),
        sidebar_cursor_blur_fg: color(on(sb_blur_bg, &[sb_text_2, sb_text, text])),
        footer_bg: color(bg2),
        footer_fg: color(muted),
        footer_key: color(legible(accent, bg2, text)),

        accent: color(accent),
        primary: color(accent),
        success: color(success),
        warning: color(warning),
        error: color(error),

        heading: color(headers),
        heading_alt: color(text),
        link: color(or("editor.link color", accent)),
        bullet: color(or("editor.list marker color", accent)),
        code_fg: color(or("editor.code.text color", text)),
        code_bg: color(or("editor.code.background color", bg2)),
        tag_fg: color(or("editor.tag.text color", text)),
        tag_bg: color(or("editor.tag.background color", bg2)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::THEMES;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn find<'a>(themes: &'a [Theme], name: &str) -> &'a Theme {
        themes
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    /// Loading must leave a read-only theme directory exactly as it was, and
    /// must not follow a symlink out of it.
    #[cfg(unix)]
    #[test]
    fn loading_reads_without_touching_the_directory() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Plain.theme",
            r##"{"base": {"text color": "#111111", "background color": "#FFFFFF",
                 "accent color": "#DD4C4F"}}"##,
        );
        write(outside.path(), "Elsewhere.theme", r##"{"base": {}}"##);
        symlink(
            outside.path().join("Elsewhere.theme"),
            dir.path().join("Linked.theme"),
        )
        .unwrap();

        let snapshot = |dir: &Path| {
            let mut files: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| {
                    let meta = std::fs::symlink_metadata(e.path()).unwrap();
                    let bytes = std::fs::read(e.path()).unwrap_or_default();
                    (e.file_name(), bytes, meta.modified().unwrap(), meta.len())
                })
                .collect();
            files.sort();
            files
        };
        let set_mode = |path: &Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        set_mode(&dir.path().join("Plain.theme"), 0o444);
        set_mode(dir.path(), 0o555);
        let before = snapshot(dir.path());

        let themes = load_dir(dir.path());

        let after = snapshot(dir.path());
        set_mode(dir.path(), 0o755);
        assert_eq!(before, after);
        assert_eq!(
            themes.iter().map(|t| t.name).collect::<Vec<_>>(),
            ["plain"],
            "the symlinked file is not followed"
        );
    }

    #[test]
    fn names_are_slugs_of_the_file_name() {
        assert_eq!(slug("Rosé Pine Dawn"), "rose-pine-dawn");
        // A macOS file name: `e` followed by a combining acute accent.
        assert_eq!(slug("Rose\u{301} Pine"), "rose-pine");
        assert_eq!(slug("Rose\u{301} Pine"), slug("Rosé Pine"));
        assert_eq!(slug("  Red   Graphite "), "red-graphite");
        assert_eq!(slug("D.Boring"), "d-boring");
        assert_eq!(slug("Shibuya Lo-fi"), "shibuya-lo-fi");
    }

    #[test]
    fn references_resolve_after_the_base_theme_is_merged() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Parent.theme",
            r##"{"base": {"text color": "#111111", "background color": "#FFFFFF",
                 "accent color": "#0000FF"},
                 "editor": {"link color": "$base.accent color"}}"##,
        );
        write(
            dir.path(),
            "Child Theme.theme",
            r##"{"meta": {"base theme": "Parent"},
                 "base": {"accent color": "#FF0000"}}"##,
        );
        write(dir.path(), "Broken.theme", "{ not json");
        write(dir.path(), "notes.txt", "{}");

        let themes = load_dir(dir.path());
        assert_eq!(
            themes.iter().map(|t| t.name).collect::<Vec<_>>(),
            ["child-theme", "parent"]
        );
        let child = find(&themes, "child-theme");
        assert_eq!(child.link, Color::Rgb(0xFF, 0, 0));
        assert_eq!(child.foreground, Color::Rgb(0x11, 0x11, 0x11));
        assert!(!child.dark);
        // Keys the file leaves out fall back to the nearest colour it has.
        assert_eq!(child.bullet, child.accent);
        assert_eq!(child.code_bg, child.background);
        assert_eq!(find(&themes, "parent").link, Color::Rgb(0, 0, 0xFF));
    }

    #[test]
    fn loops_and_missing_required_colours_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Loop.theme",
            r##"{"base": {"text color": "$base.text color",
                 "background color": "#000000", "accent color": "#FF0000"}}"##,
        );
        write(
            dir.path(),
            "Cycle.theme",
            r#"{"meta": {"base theme": "Cycle 2"}, "base": {}}"#,
        );
        write(
            dir.path(),
            "Cycle 2.theme",
            r#"{"meta": {"base theme": "Cycle"}, "base": {}}"#,
        );
        assert!(load_dir(dir.path()).is_empty());
        assert!(load_dir(&dir.path().join("absent")).is_empty());
    }

    /// The generator names these themes' toast colours from the palette they
    /// are named after (`SEMANTICS` in `tools/bear_theme.py`); a theme file
    /// has no such table, so the parser derives them and only those differ.
    const NAMED_SEMANTICS: &[&str] = &[
        "atom",
        "ayu",
        "ayu-mirage",
        "catppuccin-latte",
        "catppuccin-macchiato",
        "cobalt",
        "dracula",
        "everforest-dark",
        "everforest-light",
        "gruvbox",
        "nord",
        "rose-pine",
        "rose-pine-dawn",
        "shibuya-jazz",
        "shibuya-lo-fi",
        "solarized-dark",
        "solarized-light",
        "tokyo-night",
        "tokyo-night-light",
    ];

    /// The theme files kept in the repo (`config/themes/`, copied from Bear)
    /// parse, and each draws exactly as the palette generated from it. Needs
    /// nothing installed, so it runs everywhere.
    #[test]
    fn repo_theme_files_match_the_built_in_palettes() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/themes");
        let files = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "theme"))
            .count();
        let parsed = load_dir(&dir);
        assert_eq!(parsed.len(), files, "every file parses");

        let mut compared = 0;
        for built_in in THEMES {
            // Hand-tuned in theme.rs rather than generated.
            if ["red-graphite", "red-graphite-dark", "textual-dark"].contains(&built_in.name) {
                continue;
            }
            let mut theme = *find(&parsed, built_in.name);
            if NAMED_SEMANTICS.contains(&built_in.name) {
                theme.success = built_in.success;
                theme.warning = built_in.warning;
                theme.error = built_in.error;
            }
            assert_eq!(theme, *built_in, "{}", built_in.name);
            compared += 1;
        }
        assert_eq!(compared, files - 1, "every generated palette has its file");
    }
}
