//! The palette.
//!
//! One `Theme` is a flat table of true colours. `textual-dark` reproduces what
//! the Python Bjorn draws through Textual's default theme, down to the blended
//! values Textual computes for its `auto`/alpha colours, so the two
//! implementations look the same side by side. `red-graphite` is Bear's Red
//! Graphite theme file; `red-graphite-dark` (the default) is Bear's Dark
//! Graphite with Red Graphite's brick red (`#DD4C4F`) as the accent.
//!
//! The active theme is a process-wide index into `THEMES`, set once from the
//! config at startup, so drawing code can read it without threading a
//! reference through every function.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::{Color, Modifier, Style};

use crate::ui::bear_theme;

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// Every colour the app draws with. Panes carry their own surface and text
/// colours because Bear's Red Graphite puts a graphite sidebar next to a white
/// notes list; in `textual-dark` the two are simply equal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// True when the terminal behind this theme is expected to be dark. Only
    /// used to pick sensible `Modifier::DIM` behaviour.
    pub dark: bool,

    // -- surfaces ----------------------------------------------------------
    /// The screen: behind the reader, the footer gaps and the modals.
    pub background: Color,
    /// The notes column.
    pub surface: Color,
    /// The notes column while it has focus.
    pub surface_focus: Color,
    /// The sidebar column.
    pub sidebar_bg: Color,
    /// The sidebar column while it has focus.
    pub sidebar_focus: Color,

    // -- text --------------------------------------------------------------
    pub foreground: Color,
    pub muted: Color,
    pub sidebar_fg: Color,
    pub sidebar_muted: Color,

    // -- chrome ------------------------------------------------------------
    pub border: Color,
    pub sidebar_border: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    pub sidebar_header_bg: Color,
    pub sidebar_header_fg: Color,
    pub header_focus_bg: Color,
    pub header_focus_fg: Color,
    /// The highlighted row of a focused pane.
    pub cursor_bg: Color,
    pub cursor_fg: Color,
    /// The highlighted row of a pane without focus, over each pane's surface.
    pub cursor_blur_bg: Color,
    pub sidebar_cursor_blur_bg: Color,
    pub cursor_blur_fg: Color,
    pub sidebar_cursor_blur_fg: Color,
    pub footer_bg: Color,
    pub footer_fg: Color,
    pub footer_key: Color,

    // -- semantics ---------------------------------------------------------
    pub accent: Color,
    pub primary: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,

    // -- the rendered note -------------------------------------------------
    /// Headings 1-3.
    pub heading: Color,
    /// Headings 4-5.
    pub heading_alt: Color,
    pub link: Color,
    /// List markers, rules, table gridlines, quote bars.
    pub bullet: Color,
    pub code_fg: Color,
    pub code_bg: Color,
    /// `#tags` in the body.
    pub tag_fg: Color,
    pub tag_bg: Color,
}

/// Textual's default theme, the one the Python Bjorn runs under. The blended
/// values are what Textual resolves `auto 60%`, `$warning 10%` and the rest to
/// over the surface underneath them.
pub const TEXTUAL_DARK: Theme = Theme {
    name: "textual-dark",
    dark: true,

    background: rgb(0x121212),
    surface: rgb(0x1E1E1E),
    surface_focus: rgb(0x272727),
    sidebar_bg: rgb(0x1E1E1E),
    sidebar_focus: rgb(0x272727),

    foreground: rgb(0xE0E0E0),
    muted: rgb(0xA5A5A5),
    sidebar_fg: rgb(0xE0E0E0),
    sidebar_muted: rgb(0xA5A5A5),

    border: rgb(0x45505A),
    sidebar_border: rgb(0x45505A),
    header_bg: rgb(0x33424E),
    header_fg: rgb(0x4EBF71),
    sidebar_header_bg: rgb(0x33424E),
    sidebar_header_fg: rgb(0x4EBF71),
    header_focus_bg: rgb(0xFEA62B),
    header_focus_fg: rgb(0x0D0802),
    cursor_bg: rgb(0x0178D4),
    cursor_fg: rgb(0xDDEDF9),
    cursor_blur_bg: rgb(0x153854),
    sidebar_cursor_blur_bg: rgb(0x153854),
    cursor_blur_fg: rgb(0xE0E0E0),
    sidebar_cursor_blur_fg: rgb(0xE0E0E0),
    footer_bg: rgb(0x242F38),
    footer_fg: rgb(0xE0E0E0),
    footer_key: rgb(0xFFA62B),

    accent: rgb(0xFEA62B),
    primary: rgb(0x0178D4),
    success: rgb(0x4EBF71),
    warning: rgb(0xFEA62B),
    error: rgb(0xB93C5B),

    heading: rgb(0x0178D4),
    heading_alt: rgb(0xE0E0E0),
    link: rgb(0x57A5E2),
    bullet: rgb(0xA5A5A5),
    code_fg: rgb(0xF5BC6F),
    code_bg: rgb(0x342C1F),
    tag_fg: rgb(0xF5BC6F),
    tag_bg: rgb(0x342C1F),
};

/// Bear's Red Graphite, light: a graphite sidebar beside a white notes list
/// and a white page, with `#DD4C4F` on every accent. Colours are taken from
/// Bear's own `Red Graphite.theme`; only the status colours are Bjorn's.
pub const RED_GRAPHITE: Theme = Theme {
    name: "red-graphite",
    dark: false,

    background: rgb(0xFFFFFF),    // base.background
    surface: rgb(0xFFFFFF),       // base.background
    surface_focus: rgb(0xFFFFFF), // base.background
    sidebar_bg: rgb(0x2E3235),    // sidebar.background
    sidebar_focus: rgb(0x2E3235), // sidebar.background

    foreground: rgb(0x444444),    // base.text
    muted: rgb(0x888888),         // base.text secondary
    sidebar_fg: rgb(0xD1D1D1),    // sidebar.text
    sidebar_muted: rgb(0x9FA09F), // sidebar.icon

    border: rgb(0xD9D9D9),         // base.stroke
    sidebar_border: rgb(0x2E3235), // sidebar.stroke
    header_bg: rgb(0xF3F5F7),      // base.background secondary
    header_fg: rgb(0x444444),
    sidebar_header_bg: rgb(0x474747), // sidebar.background secondary
    sidebar_header_fg: rgb(0xFFFFFF), // sidebar.text secondary
    header_focus_bg: rgb(0xDD4C4F),
    header_focus_fg: rgb(0xFFFFFF),
    cursor_bg: rgb(0xDD4C4F),
    cursor_fg: rgb(0xFFFFFF),
    cursor_blur_bg: rgb(0xF3F5F7),         // notes.selection background
    sidebar_cursor_blur_bg: rgb(0x474747), // sidebar.background secondary
    cursor_blur_fg: rgb(0x444444),
    sidebar_cursor_blur_fg: rgb(0xFFFFFF),
    footer_bg: rgb(0xF3F5F7),
    footer_fg: rgb(0x444444),
    footer_key: rgb(0xDD4C4F),

    accent: rgb(0xDD4C4F), // base.accent
    primary: rgb(0xDD4C4F),
    success: rgb(0x3F9D63),
    warning: rgb(0xB7791F),
    error: rgb(0xC0392B),

    heading: rgb(0x444444), // editor.headers.text
    heading_alt: rgb(0x444444),
    link: rgb(0xDD4C4F),    // editor.link
    bullet: rgb(0xDD4C4F),  // editor.list marker
    code_fg: rgb(0x444444), // editor.code.text
    code_bg: rgb(0xF3F5F7), // editor.code.background
    tag_fg: rgb(0x444444),  // editor.tag.text
    tag_bg: rgb(0xE4E5E6),  // editor.tag.background
};

/// Red Graphite in the dark. Bear ships no dark Red Graphite, so this is
/// Bear's `Dark Graphite.theme` with its blue accent swapped for Red
/// Graphite's `#DD4C4F`. The sidebar sits a shade above the page, as in Bear.
pub const RED_GRAPHITE_DARK: Theme = Theme {
    name: "red-graphite-dark",
    dark: true,

    background: rgb(0x1D1E1F),    // base.background
    surface: rgb(0x1D1E1F),       // base.background
    surface_focus: rgb(0x1D1E1F), // base.background
    sidebar_bg: rgb(0x2C2D2F),    // sidebar.background
    sidebar_focus: rgb(0x2C2D2F), // sidebar.background

    foreground: rgb(0xDFE0E0),    // base.text
    muted: rgb(0xA2A3A4),         // base.text secondary
    sidebar_fg: rgb(0xA5A6A6),    // sidebar.text
    sidebar_muted: rgb(0xABACAB), // sidebar.icon

    border: rgb(0x525354),         // editor.separator
    sidebar_border: rgb(0x2C2D2F), // sidebar.stroke
    header_bg: rgb(0x2E2F30),      // base.background secondary
    header_fg: rgb(0xDFE0E0),
    sidebar_header_bg: rgb(0x535354), // sidebar.background secondary
    sidebar_header_fg: rgb(0xD9D8DA), // sidebar.text secondary
    header_focus_bg: rgb(0xDD4C4F),
    header_focus_fg: rgb(0xFFFFFF),
    cursor_bg: rgb(0xDD4C4F),
    cursor_fg: rgb(0xFFFFFF),
    cursor_blur_bg: rgb(0x2E2F30),         // notes.selection background
    sidebar_cursor_blur_bg: rgb(0x535354), // sidebar.background secondary
    cursor_blur_fg: rgb(0xDFE0E0),
    sidebar_cursor_blur_fg: rgb(0xD9D8DA),
    footer_bg: rgb(0x2E2F30),
    footer_fg: rgb(0xDFE0E0),
    footer_key: rgb(0xDD4C4F),

    accent: rgb(0xDD4C4F),
    primary: rgb(0xDD4C4F),
    success: rgb(0x6FB98F),
    warning: rgb(0xE0A458),
    error: rgb(0xE05C5C),

    heading: rgb(0xCCDBE5), // editor.headers.text
    heading_alt: rgb(0xDFE0E0),
    link: rgb(0xDD4C4F),
    bullet: rgb(0xDD4C4F),
    code_fg: rgb(0xDFE0E0), // editor.code.text
    code_bg: rgb(0x2E2F30), // editor.code.background
    tag_fg: rgb(0xDFE0E0),  // editor.tag.text
    tag_bg: rgb(0x454647),  // editor.tag.background
};

/// The themes compiled in, so Bjorn has a palette without Bear.app.
pub const BUILT_IN: &[Theme] = &[TEXTUAL_DARK, RED_GRAPHITE, RED_GRAPHITE_DARK];

/// The default is Bear's Red Graphite, dark.
pub const DEFAULT_THEME: &str = "red-graphite-dark";

/// `DEFAULT_THEME`'s index in `themes()`, so drawing before `set` still uses it.
const DEFAULT_INDEX: usize = 2;

static ACTIVE: AtomicUsize = AtomicUsize::new(DEFAULT_INDEX);

static BEAR: OnceLock<Vec<Theme>> = OnceLock::new();

/// The themes read (read-only) from Bear.app, minus any a built-in already
/// names: the built-ins always win, and match Bear's files (a test checks).
/// Loaded once, and only when something asks past the built-ins.
fn bear() -> &'static [Theme] {
    BEAR.get_or_init(|| {
        bear_theme::load_dir(Path::new(bear_theme::BEAR_THEMES_DIR))
            .into_iter()
            .filter(|t| BUILT_IN.iter().all(|b| b.name != t.name))
            .collect()
    })
}

/// Every theme: the built-ins in their fixed order, then Bear's. Opens
/// Bear.app, so it is for listing, not for drawing.
pub fn themes() -> Vec<&'static Theme> {
    BUILT_IN.iter().chain(bear()).collect()
}

/// The theme every drawing function reads. A built-in never touches Bear.app.
#[inline]
pub fn current() -> &'static Theme {
    // The index only ever comes from `set`, which bounds it.
    let index = ACTIVE.load(Ordering::Relaxed);
    BUILT_IN
        .get(index)
        .unwrap_or_else(|| &bear()[index - BUILT_IN.len()])
}

/// The theme called `name`, ignoring case and spacing (`Rosé Pine`,
/// `rosé-pine`); `None` when there is no such theme.
pub fn lookup(name: &str) -> Option<usize> {
    find(name, bear)
}

/// `lookup` over the built-ins, then over `extra` — called only when no
/// built-in matches, so naming a built-in never reads Bear.app.
fn find(name: &str, extra: impl FnOnce() -> &'static [Theme]) -> Option<usize> {
    let wanted = bear_theme::slug(name);
    BUILT_IN.iter().position(|t| t.name == wanted).or_else(|| {
        extra()
            .iter()
            .position(|t| t.name == wanted)
            .map(|i| BUILT_IN.len() + i)
    })
}

/// Make `name` the active theme; false (and no change) when there is no such
/// theme, so a typo in the config keeps the app running on the default.
pub fn set(name: &str) -> bool {
    match lookup(name) {
        Some(index) => {
            ACTIVE.store(index, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Every theme name, in the order they are offered.
pub fn names() -> impl Iterator<Item = &'static str> {
    BUILT_IN.iter().chain(bear()).map(|t| t.name)
}

// -- the styles the drawing code asks for ------------------------------------

/// The whole screen, under everything else.
pub fn screen() -> Style {
    let t = current();
    Style::default().bg(t.background).fg(t.foreground)
}

/// A pane's body: notes and reader take the surface, the sidebar its own.
pub fn surface(focused: bool) -> Style {
    let t = current();
    Style::default()
        .bg(if focused { t.surface_focus } else { t.surface })
        .fg(t.foreground)
}

pub fn sidebar_surface(focused: bool) -> Style {
    let t = current();
    Style::default()
        .bg(if focused {
            t.sidebar_focus
        } else {
            t.sidebar_bg
        })
        .fg(t.sidebar_fg)
}

/// Column header at rest.
pub fn header() -> Style {
    let t = current();
    Style::default()
        .bg(t.header_bg)
        .fg(t.header_fg)
        .add_modifier(Modifier::BOLD)
}

pub fn sidebar_header() -> Style {
    let t = current();
    Style::default()
        .bg(t.sidebar_header_bg)
        .fg(t.sidebar_header_fg)
        .add_modifier(Modifier::BOLD)
}

/// Column header of the focused pane: filled with the accent.
pub fn header_focused() -> Style {
    let t = current();
    Style::default()
        .bg(t.header_focus_bg)
        .fg(t.header_focus_fg)
        .add_modifier(Modifier::BOLD)
}

pub fn footer() -> Style {
    let t = current();
    Style::default().bg(t.footer_bg).fg(t.footer_fg)
}

pub fn footer_key() -> Style {
    let t = current();
    Style::default()
        .bg(t.footer_bg)
        .fg(t.footer_key)
        .add_modifier(Modifier::BOLD)
}

/// Second-rank text inside a row: the date and preview of a note, a tag's
/// count. `DIM` reads as grey on every terminal that honours it; the colour is
/// there for the ones that do not.
pub fn dim() -> Style {
    Style::default()
        .fg(current().muted)
        .add_modifier(Modifier::DIM)
}

pub fn muted() -> Style {
    Style::default().fg(current().muted)
}

pub fn sidebar_muted() -> Style {
    Style::default().fg(current().sidebar_muted)
}

/// List markers: grey in Textual's theme, Bear's red in Red Graphite.
pub fn bullet() -> Style {
    Style::default().fg(current().bullet)
}

/// Inline code, and the fenced block's body.
pub fn code() -> Style {
    let t = current();
    Style::default().bg(t.code_bg).fg(t.code_fg)
}

/// A `#tag` in the body: Bear draws it as a pill, so it gets a background.
pub fn tag() -> Style {
    let t = current();
    Style::default().bg(t.tag_bg).fg(t.tag_fg)
}

/// Headings: 1-3 take the accent colour, 4-5 the body colour, 6 is muted.
pub fn heading(level: u8) -> Style {
    let t = current();
    match level {
        1 => Style::default().fg(t.heading).add_modifier(Modifier::BOLD),
        2 => Style::default()
            .fg(t.heading)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        3 => Style::default().fg(t.heading).add_modifier(Modifier::BOLD),
        4 => Style::default()
            .fg(t.heading_alt)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        5 => Style::default()
            .fg(t.heading_alt)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(t.muted).add_modifier(Modifier::BOLD),
    }
}

pub fn link() -> Style {
    Style::default()
        .fg(current().link)
        .add_modifier(Modifier::UNDERLINED)
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The highlighted row of a focused pane.
pub fn cursor_focused() -> Style {
    let t = current();
    Style::default().bg(t.cursor_bg).fg(t.cursor_fg)
}

/// The highlighted row of a pane without focus.
pub fn cursor_unfocused() -> Style {
    let t = current();
    Style::default().bg(t.cursor_blur_bg).fg(t.cursor_blur_fg)
}

/// The sidebar's highlighted row; its surface is its own, so its blurred
/// cursor has to be too.
pub fn sidebar_cursor(focused: bool) -> Style {
    let t = current();
    if focused {
        Style::default().bg(t.cursor_bg).fg(t.cursor_fg)
    } else {
        Style::default()
            .bg(t.sidebar_cursor_blur_bg)
            .fg(t.sidebar_cursor_blur_fg)
    }
}

pub fn border() -> Style {
    Style::default().fg(current().border)
}

pub fn sidebar_border() -> Style {
    Style::default().fg(current().sidebar_border)
}

pub fn accent() -> Style {
    Style::default().fg(current().accent)
}

/// Search matches, as `reverse bold` was in the Python Bjorn.
pub fn match_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

// Shorthands for the raw colours, so call sites read like the CSS they mirror.
pub fn accent_color() -> Color {
    current().accent
}
pub fn warning_color() -> Color {
    current().warning
}
pub fn error_color() -> Color {
    current().error
}
pub fn success_color() -> Color {
    current().success
}

#[cfg(test)]
mod tests {
    use super::*;

    // `set` is not exercised here: the active theme is process-wide and the
    // rest of the suite draws with it. The lookup it is built on is the part
    // with logic in it.
    #[test]
    fn themes_are_found_by_name() {
        assert_eq!(themes()[lookup(DEFAULT_THEME).unwrap()].name, DEFAULT_THEME);
        assert_eq!(themes()[DEFAULT_INDEX].name, DEFAULT_THEME);
        assert_eq!(BUILT_IN[DEFAULT_INDEX].name, DEFAULT_THEME);
        assert_eq!(
            themes()[lookup("  Red Graphite ").unwrap()].name,
            "red-graphite"
        );
        assert!(lookup("mauve").is_none());
        assert_eq!(names().count(), themes().len());
    }

    #[test]
    fn built_in_names_never_read_bear_app() {
        let untouched = || -> &'static [Theme] { panic!("Bear.app was read") };
        for theme in BUILT_IN {
            assert!(find(&theme.name.to_uppercase(), untouched).is_some());
        }
        assert_eq!(find("mauve", || &[]), None);
        assert_eq!(find("Nord", || &BUILT_IN[..1]), None);
    }

    #[test]
    fn every_theme_is_complete() {
        for theme in themes() {
            assert_eq!(theme.name, theme.name.to_ascii_lowercase());
            // A true colour everywhere: an ANSI name would let the terminal's
            // own palette decide and the two implementations would diverge.
            for color in [
                theme.background,
                theme.surface,
                theme.sidebar_bg,
                theme.foreground,
                theme.muted,
                theme.border,
                theme.header_bg,
                theme.cursor_bg,
                theme.footer_bg,
                theme.accent,
                theme.heading,
                theme.code_bg,
            ] {
                assert!(matches!(color, Color::Rgb(..)), "{} {color:?}", theme.name);
            }
        }
    }
}
