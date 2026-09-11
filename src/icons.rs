//! Terminal glyphs for the sidebar: top-level tags and the smart views.
//!
//! Icons are named with Lucide names and rendered in one of three styles:
//! `nerd` (Material Design Icons from Nerd Fonts), `emoji`, or `lucide` (the
//! Lucide icon font, by name from the bundled codepoint table pinned to
//! lucide-static LUCIDE_VERSION; opt-in because the terminal must map that
//! Private Use Area range onto `lucide.ttf`). `auto` picks nerd or emoji by
//! looking at the terminal and never picks lucide. Names prefixed `emoji:` are
//! literal glyphs. `none` renders no icons.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use unicode_width::UnicodeWidthStr;

use crate::util::home_dir;

pub const ICON_STYLES: [&str; 5] = ["auto", "nerd", "emoji", "lucide", "none"];
pub const FALLBACK_GLYPH_STYLE: &str = "emoji";
/// The lucide-static release the bundled codepoints (and the font you install)
/// must come from. Lucide reassigns codepoints between releases.
pub const LUCIDE_VERSION: &str = "1.43.0";
/// Glyphs are padded to this many cells plus one separating space, so labels
/// line up whether the glyph is single- or double-width.
pub const ICON_CELL_WIDTH: usize = 2;
pub const DEFAULT_TAG_ICON: &str = "tag";

pub static NERD_GLYPHS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("archive", "\u{f003c}"),
        ("atom", "\u{f0768}"),
        ("book", "\u{f00ba}"),
        ("book-open", "\u{f00bd}"),
        ("bookmark", "\u{f00c0}"),
        ("bot", "\u{f06a9}"),
        ("brain", "\u{f09d1}"),
        ("briefcase", "\u{f00d6}"),
        ("calendar", "\u{f00ed}"),
        ("check-circle", "\u{f05e0}"),
        ("clipboard-list", "\u{f10d4}"),
        ("code", "\u{f0174}"),
        ("compass", "\u{f018b}"),
        ("computer", "\u{f0322}"),
        ("database", "\u{f01bc}"),
        ("file-text", "\u{f0219}"),
        ("flask-conical", "\u{f0093}"),
        ("folder", "\u{f024b}"),
        ("gamepad-2", "\u{f0296}"),
        ("globe", "\u{f01e7}"),
        ("graduation-cap", "\u{f0474}"),
        ("heart", "\u{f02d1}"),
        ("home", "\u{f02dc}"),
        ("landmark", "\u{f0070}"),
        ("leaf", "\u{f032a}"),
        ("library", "\u{f0331}"),
        ("lightbulb", "\u{f0335}"),
        ("lock", "\u{f033e}"),
        ("flower", "\u{f024a}"),
        ("music", "\u{f075a}"),
        ("notebook", "\u{f082e}"),
        ("palette", "\u{f03d8}"),
        ("pencil", "\u{f03eb}"),
        ("pin", "\u{f0403}"),
        ("rocket", "\u{f0463}"),
        ("search", "\u{f0349}"),
        ("settings", "\u{f0493}"),
        ("star", "\u{f04ce}"),
        ("sun", "\u{f05a8}"),
        ("tag", "\u{f04f9}"),
        ("tags", "\u{f04fb}"),
        ("terminal", "\u{f018d}"),
        ("trash", "\u{f01b4}"),
        ("trees", "\u{f0405}"),
        ("trophy", "\u{f0538}"),
        ("users", "\u{f0849}"),
        ("wrench", "\u{f05b7}"),
        ("zap", "\u{f0241}"),
    ])
});

pub static EMOJI_GLYPHS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("archive", "\u{1f5c4}"),
        ("atom", "⚛"),
        ("book", "\u{1f4d5}"),
        ("book-open", "\u{1f4d6}"),
        ("bookmark", "\u{1f516}"),
        ("bot", "\u{1f916}"),
        ("brain", "\u{1f9e0}"),
        ("briefcase", "\u{1f4bc}"),
        ("calendar", "\u{1f4c5}"),
        ("check-circle", "✅"),
        ("clipboard-list", "\u{1f4cb}"),
        ("code", "⌨"),
        ("compass", "\u{1f9ed}"),
        ("computer", "\u{1f4bb}"),
        ("database", "\u{1f5c3}"),
        ("file-text", "\u{1f4c4}"),
        ("flask-conical", "\u{1f9ea}"),
        ("folder", "\u{1f4c1}"),
        ("gamepad-2", "\u{1f3ae}"),
        ("globe", "\u{1f30d}"),
        ("graduation-cap", "\u{1f393}"),
        ("heart", "❤"),
        ("home", "\u{1f3e0}"),
        ("landmark", "\u{1f3db}"),
        ("leaf", "\u{1f342}"),
        ("library", "\u{1f4da}"),
        ("lightbulb", "\u{1f4a1}"),
        ("lock", "\u{1f512}"),
        ("flower", "\u{1f338}"),
        ("music", "\u{1f3b5}"),
        ("notebook", "\u{1f4d3}"),
        ("palette", "\u{1f3a8}"),
        ("pencil", "✏"),
        ("pin", "\u{1f4cc}"),
        ("rocket", "\u{1f680}"),
        ("search", "\u{1f50d}"),
        ("settings", "⚙"),
        ("star", "⭐"),
        ("sun", "☀"),
        ("tag", "\u{1f3f7}"),
        ("tags", "\u{1f3f7}"),
        ("terminal", "▸"),
        ("trash", "\u{1f5d1}"),
        ("trees", "\u{1f332}"),
        ("trophy", "\u{1f3c6}"),
        ("users", "\u{1f465}"),
        ("wrench", "\u{1f527}"),
        ("zap", "⚡"),
    ])
});

const LUCIDE_CODEPOINTS_JSON: &str = include_str!("data/lucide-codepoints.json");

/// Every Lucide icon name -> the codepoint in lucide.ttf, loaded once.
pub fn lucide_glyphs() -> &'static HashMap<String, String> {
    static TABLE: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
        let data: HashMap<String, u32> =
            serde_json::from_str(LUCIDE_CODEPOINTS_JSON).expect("bundled lucide table");
        data.into_iter()
            .map(|(name, code)| (name, char::from_u32(code).unwrap_or('\u{fffd}').to_string()))
            .collect()
    });
    &TABLE
}

fn table_glyph(style: &str, name: &str) -> Option<String> {
    match style {
        "lucide" => lucide_glyphs().get(name).cloned(),
        "nerd" => NERD_GLYPHS.get(name).map(|g| g.to_string()),
        _ => EMOJI_GLYPHS.get(name).map(|g| g.to_string()),
    }
}

/// Icon names for top-level tags when the config says nothing.
pub static DEFAULT_TAG_ICONS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("work", "briefcase"),
        ("home", "home"),
        ("projects", "folder"),
        ("ideas", "lightbulb"),
        ("journal", "notebook"),
        ("books", "book-open"),
        ("reading", "book-open"),
        ("tech", "code"),
        ("code", "code"),
        ("garden", "leaf"),
        ("travel", "compass"),
        ("health", "heart"),
        ("music", "music"),
        ("robotics", "bot"),
        ("school", "graduation-cap"),
    ])
});

/// Icon names for the smart views, keyed by `View::value()`.
pub static VIEW_ICONS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("all", "notebook"),
        ("untagged", "tag"),
        ("todo", "check-circle"),
        ("today", "calendar"),
        ("pinned", "pin"),
        ("archive", "archive"),
        ("trash", "trash"),
    ])
});

const NERD_FONT_TERMINALS: [&str; 2] = ["ghostty", "wezterm"];
const FONT_SUFFIXES: [&str; 4] = ["ttf", "otf", "ttc", "dfont"];

fn font_directories() -> [PathBuf; 2] {
    [
        home_dir().join("Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ]
}

pub fn has_nerd_font_terminal(term_program: Option<&str>) -> bool {
    term_program
        .map(|t| NERD_FONT_TERMINALS.contains(&t.trim().to_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn has_nerd_font_installed() -> bool {
    font_directories().iter().any(|dir| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                let suffix = name.rsplit('.').next().unwrap_or("");
                FONT_SUFFIXES.contains(&suffix) && name.contains("nerd")
            })
        })
    })
}

/// Nerd when the terminal bundles the symbols or a Nerd Font is installed,
/// emoji otherwise. A heuristic; `icon_style` in config overrides it.
pub fn detect_glyph_style(term_program: Option<&str>) -> &'static str {
    if has_nerd_font_terminal(term_program) || has_nerd_font_installed() {
        "nerd"
    } else {
        FALLBACK_GLYPH_STYLE
    }
}

pub fn pad_glyph(glyph: &str) -> String {
    if glyph.is_empty() {
        return String::new();
    }
    let width = UnicodeWidthStr::width(glyph);
    format!(
        "{glyph}{} ",
        " ".repeat(ICON_CELL_WIDTH.saturating_sub(width))
    )
}

/// Resolved icon style plus the per-tag overrides from config.
#[derive(Debug, Clone)]
pub struct IconSet {
    pub enabled: bool,
    pub style: &'static str,
    pub tag_icons: HashMap<String, String>,
}

impl IconSet {
    pub fn new(
        style: &str,
        tag_icons: &std::collections::BTreeMap<String, String>,
        term_program: Option<&str>,
    ) -> IconSet {
        let mut style = style.trim().to_lowercase();
        if !ICON_STYLES.contains(&style.as_str()) {
            style = "auto".into();
        }
        let enabled = style != "none";
        let resolved: &'static str = match style.as_str() {
            "auto" => detect_glyph_style(term_program),
            "nerd" => "nerd",
            "emoji" => "emoji",
            "lucide" => "lucide",
            _ => FALLBACK_GLYPH_STYLE,
        };
        let tag_icons = tag_icons
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| {
                (
                    k.trim().trim_matches('#').to_lowercase(),
                    v.trim().to_string(),
                )
            })
            .collect();
        IconSet {
            enabled,
            style: resolved,
            tag_icons,
        }
    }

    pub fn none() -> IconSet {
        IconSet {
            enabled: false,
            style: FALLBACK_GLYPH_STYLE,
            tag_icons: HashMap::new(),
        }
    }

    /// A padded glyph for an icon name, `emoji:<literal>`, or "".
    pub fn glyph(&self, name: &str) -> String {
        if !self.enabled || name.is_empty() {
            return String::new();
        }
        if let Some(literal) = name.strip_prefix("emoji:") {
            return pad_glyph(literal);
        }
        let glyph = table_glyph(self.style, name)
            .or_else(|| table_glyph(self.style, DEFAULT_TAG_ICON))
            .unwrap_or_default();
        pad_glyph(&glyph)
    }

    pub fn for_tag(&self, top_level_tag: &str) -> String {
        let key = top_level_tag.trim().trim_matches('#').to_lowercase();
        let name = self
            .tag_icons
            .get(&key)
            .cloned()
            .or_else(|| DEFAULT_TAG_ICONS.get(key.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| DEFAULT_TAG_ICON.to_string());
        self.glyph(&name)
    }

    pub fn for_view(&self, view_value: &str) -> String {
        self.glyph(VIEW_ICONS.get(view_value).unwrap_or(&DEFAULT_TAG_ICON))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn tables_cover_every_default_in_every_style() {
        let lucide = lucide_glyphs();
        for name in DEFAULT_TAG_ICONS
            .values()
            .chain(VIEW_ICONS.values())
            .chain([&DEFAULT_TAG_ICON])
        {
            assert!(
                NERD_GLYPHS.contains_key(name)
                    && EMOJI_GLYPHS.contains_key(name)
                    && lucide.contains_key(*name),
                "{name}"
            );
        }
        let nerd: std::collections::BTreeSet<_> = NERD_GLYPHS.keys().collect();
        let emoji: std::collections::BTreeSet<_> = EMOJI_GLYPHS.keys().collect();
        assert_eq!(nerd, emoji);
        assert!(nerd.iter().all(|n| lucide.contains_key(**n)));
    }

    #[test]
    fn lucide_style_emits_font_codepoints() {
        let lucide = lucide_glyphs();
        assert!(lucide.len() > 2000);
        assert_eq!(LUCIDE_VERSION, "1.43.0");
        assert_eq!(lucide["bot"], "\u{e1bb}");
        assert_eq!(lucide["tag"], "\u{e17f}");
        assert!(
            lucide
                .values()
                .all(|g| (0xE000..=0xE7FF).contains(&(g.chars().next().unwrap() as u32)))
        );
        let icons = IconSet::new("lucide", &BTreeMap::new(), None);
        assert_eq!(icons.for_tag("robotics"), pad_glyph("\u{e1bb}"));
        assert_eq!(icons.for_tag("unknown"), pad_glyph(&lucide["tag"]));
        assert_eq!(icons.glyph("sparkles"), pad_glyph(&lucide["sparkles"]));
        assert_eq!(
            IconSet::new("auto", &BTreeMap::new(), Some("ghostty")).style,
            "nerd"
        );
    }

    #[test]
    fn nerd_icons_for_known_tags_and_fallback() {
        let icons = IconSet::new("nerd", &BTreeMap::new(), None);
        assert_eq!(icons.for_tag("robotics"), pad_glyph("\u{f06a9}"));
        assert_eq!(icons.for_tag("#Tech"), pad_glyph(NERD_GLYPHS["code"]));
        assert_eq!(icons.for_tag("brand-new"), pad_glyph(NERD_GLYPHS["tag"]));
        assert_eq!(
            icons.for_view("todo"),
            pad_glyph(NERD_GLYPHS["check-circle"])
        );
        assert_eq!(pad_glyph("\u{f06a9}"), "\u{f06a9}  ");
    }

    #[test]
    fn config_overrides_and_literal_emoji() {
        let overrides = BTreeMap::from([
            ("#tech".to_string(), "terminal".to_string()),
            ("school".to_string(), "emoji:🎓".to_string()),
            ("bogus".to_string(), String::new()),
        ]);
        let icons = IconSet::new("emoji", &overrides, None);
        assert_eq!(icons.for_tag("tech"), pad_glyph("▸"));
        assert_eq!(icons.for_tag("school"), "🎓 ");
        assert_eq!(icons.for_tag("bogus"), pad_glyph(EMOJI_GLYPHS["tag"]));
    }

    #[test]
    fn none_and_auto() {
        assert_eq!(
            IconSet::new("none", &BTreeMap::new(), None).for_tag("tech"),
            ""
        );
        assert_eq!(
            IconSet::new("none", &BTreeMap::new(), None).for_view("all"),
            ""
        );
        assert_eq!(detect_glyph_style(Some("ghostty")), "nerd");
        assert_eq!(
            IconSet::new("garbage", &BTreeMap::new(), Some("WezTerm")).style,
            "nerd"
        );
    }
}
