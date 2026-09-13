//! Bear's own themes, read from the `.theme` files inside Bear.app.
//!
//! A theme file is JSON: sections (`base`, `sidebar`, `notes`, `editor`)
//! holding `"… color": "#RRGGBB"` values or `"$section.key"` references to
//! other values. A file may name another theme under `meta."base theme"`; its
//! values are laid over that theme's before any reference is resolved, so a
//! base theme's `$base.accent color` picks up the child's accent.
//!
//! Each file becomes one `Theme`, named after the file (`Rosé Pine.theme` is
//! `rose-pine`). Bear ships no dark Red Graphite, so `red-graphite-dark` is
//! derived: Dark Graphite with Red Graphite's accent.
//!
//! Bear.app is strictly read-only to Bjorn. Nothing here writes, creates,
//! renames or deletes: files are opened with `read(true)` alone, only regular
//! `*.theme` files at the top of the directory are touched (no symlinks, no
//! recursion), and each is capped at `MAX_BYTES`. `theme::lookup` only calls
//! in here for a name that is not built in, so a default install never opens
//! the bundle at all.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::ui::theme::Theme;

/// Where Bear keeps its theme files.
pub const BEAR_THEMES_DIR: &str =
    "/Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources";

/// References and `base theme` chains deeper than this are treated as broken.
const MAX_DEPTH: usize = 16;

/// Bear's theme files are about 5 KB; anything past this is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// The name a theme file goes by: lowercase, words joined by `-`, accents
/// folded away (`Rosé Pine` is `rose-pine`). macOS stores file names
/// decomposed (`e` + U+0301) while a typed `é` is one character, so folding
/// both to `e` is what lets the two meet.
pub fn slug(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| !('\u{300}'..='\u{36F}').contains(c))
        .map(|c| match c {
            'à'..='å' => 'a',
            'ç' => 'c',
            'è'..='ë' => 'e',
            'ì'..='ï' => 'i',
            'ñ' => 'n',
            'ò'..='ö' | 'ø' => 'o',
            'ù'..='ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Every theme in `dir` that parses, plus the derived `red-graphite-dark`,
/// sorted by name. A missing directory or a broken file is skipped rather than
/// reported: Bear's themes are extras on top of the built-in ones.
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
        .filter_map(|name| {
            let merged = merged(&raw, name, 0)?;
            to_theme(&slug(name), &merged)
        })
        .collect();
    if let Some(theme) = red_graphite_dark(&raw) {
        themes.push(theme);
    }
    themes.sort_by(|a, b| a.name.cmp(b.name));
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

/// Dark Graphite wearing Red Graphite's accent.
fn red_graphite_dark(raw: &HashMap<String, Value>) -> Option<Theme> {
    let red = merged(raw, "Red Graphite", 0)?;
    let accent = lookup(&red, "base.accent color", 0)?;
    let mut dark = merged(raw, "Dark Graphite", 0)?;
    let base = dark.get_mut("base")?.as_object_mut()?;
    base.insert("accent color".into(), Value::String(hex(accent)));
    to_theme("red-graphite-dark", &dark)
}

/// `name`'s values laid over its `base theme`'s, recursively.
fn merged(raw: &HashMap<String, Value>, name: &str, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let own = raw.get(name)?;
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

/// The colour at a dotted `section.key` path, following `$` references.
fn lookup(root: &Value, path: &str, depth: usize) -> Option<Color> {
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

/// `#RRGGBB`, or `#RRGGBBAA` with the alpha dropped.
fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

fn hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => String::new(),
    }
}

/// WCAG relative luminance, 0 (black) to 1 (white).
fn luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        return 0.0;
    };
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// Map a merged theme file onto Bjorn's palette. Only the page background,
/// the text and the accent are required; everything else falls back to the
/// nearest key Bear would use.
fn to_theme(name: &str, root: &Value) -> Option<Theme> {
    let first = |paths: &[&str]| paths.iter().find_map(|p| lookup(root, p, 0));
    let background = first(&["base.background color"])?;
    let foreground = first(&["base.text color"])?;
    let accent = first(&["base.accent color"])?;

    let dark = luminance(background) < 0.5;
    let or = |paths: &[&str], fallback: Color| first(paths).unwrap_or(fallback);
    let background_2 = or(&["base.background secondary color"], background);
    let muted = or(&["base.text secondary color"], foreground);
    let sidebar_bg = or(&["sidebar.background color"], background_2);
    let sidebar_fg = or(&["sidebar.text color"], foreground);
    let sidebar_bg_2 = or(&["sidebar.background secondary color"], background_2);
    let sidebar_fg_2 = or(&["sidebar.text secondary color"], sidebar_fg);
    let on_accent = if luminance(accent) > 0.4 {
        Color::Rgb(0x1A, 0x1A, 0x1A)
    } else {
        Color::Rgb(0xFF, 0xFF, 0xFF)
    };
    let (success, warning, error) = if dark {
        (0x6FB98F, 0xE0A458, 0xE05C5C)
    } else {
        (0x3F9D63, 0xB7791F, 0xC0392B)
    };
    let rgb = |hex: u32| Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);

    Some(Theme {
        name: Box::leak(name.to_string().into_boxed_str()),
        dark,

        background,
        surface: background,
        surface_focus: background,
        sidebar_bg,
        sidebar_focus: sidebar_bg,

        foreground,
        muted,
        sidebar_fg,
        sidebar_muted: or(&["sidebar.icon color"], muted),

        border: or(
            &["editor.separator.border color", "base.stroke color"],
            muted,
        ),
        sidebar_border: or(&["sidebar.stroke color"], sidebar_bg),
        header_bg: background_2,
        header_fg: foreground,
        sidebar_header_bg: sidebar_bg_2,
        sidebar_header_fg: sidebar_fg_2,
        header_focus_bg: accent,
        header_focus_fg: on_accent,
        cursor_bg: accent,
        cursor_fg: on_accent,
        cursor_blur_bg: or(&["notes.selection background color"], background_2),
        sidebar_cursor_blur_bg: sidebar_bg_2,
        cursor_blur_fg: foreground,
        sidebar_cursor_blur_fg: sidebar_fg_2,
        footer_bg: background_2,
        footer_fg: foreground,
        footer_key: accent,

        accent,
        primary: accent,
        success: rgb(success),
        warning: rgb(warning),
        error: rgb(error),

        heading: or(&["editor.headers.text color"], foreground),
        heading_alt: foreground,
        link: or(&["editor.link color"], accent),
        bullet: or(&["editor.list marker color"], accent),
        code_fg: or(&["editor.code.text color"], foreground),
        code_bg: or(&["editor.code.background color"], background_2),
        tag_fg: or(&["editor.tag.text color"], foreground),
        tag_bg: or(
            &[
                "editor.tag.background color",
                "base.background tertiary color",
            ],
            background_2,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{RED_GRAPHITE, RED_GRAPHITE_DARK};

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
        // Keys the file leaves out fall back to what Bear would use.
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

    #[test]
    fn red_graphite_dark_is_dark_graphite_with_red_graphites_accent() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Red Graphite.theme",
            r##"{"base": {"text color": "#444444", "background color": "#FFFFFF",
                 "accent color": "#DD4C4F"}}"##,
        );
        write(
            dir.path(),
            "Dark Graphite.theme",
            r##"{"base": {"text color": "#DFE0E0", "background color": "#1D1E1F",
                 "accent color": "#44A2E5"},
                 "editor": {"link color": "$base.accent color"}}"##,
        );
        let themes = load_dir(dir.path());
        let theme = find(&themes, "red-graphite-dark");
        assert!(theme.dark);
        assert_eq!(theme.background, Color::Rgb(0x1D, 0x1E, 0x1F));
        assert_eq!(theme.link, Color::Rgb(0xDD, 0x4C, 0x4F));
        assert_eq!(theme.cursor_fg, Color::Rgb(0xFF, 0xFF, 0xFF));
    }

    /// Against the real Bear.app, when it is installed: every shipped theme
    /// parses, and the built-in fallbacks match what the files say.
    #[test]
    fn bear_app_themes_parse_and_match_the_built_ins() {
        let dir = Path::new(BEAR_THEMES_DIR);
        let files = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "theme"))
                .count(),
            Err(_) => return,
        };
        let themes = load_dir(dir);
        assert_eq!(
            themes.len(),
            files + 1,
            "one per file, plus the derived one"
        );
        assert_eq!(*find(&themes, "red-graphite"), RED_GRAPHITE);
        assert_eq!(*find(&themes, "red-graphite-dark"), RED_GRAPHITE_DARK);
    }
}
