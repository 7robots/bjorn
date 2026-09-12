//! The palette.
//!
//! One `Theme` is a flat table of true colours. `textual-dark` reproduces what
//! the Python Bjorn draws through Textual's default theme, down to the blended
//! values Textual computes for its `auto`/alpha colours, so the two
//! implementations look the same side by side. `red-graphite` and
//! `red-graphite-dark` are Bear's Red Graphite: graphite chrome, one coral red
//! (`#CD5654`, sampled from Bear) for every accent.
//!
//! The active theme is a process-wide index into `THEMES`, set once from the
//! config at startup, so drawing code can read it without threading a
//! reference through every function.

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::{Color, Modifier, Style};

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
#[derive(Clone, Copy, Debug)]
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
/// and a white page, with `#CD5654` on every accent. The greys are sampled
/// from Bear itself.
pub const RED_GRAPHITE: Theme = Theme {
    name: "red-graphite",
    dark: false,

    background: rgb(0xFFFFFF),
    surface: rgb(0xFDFDFD),
    surface_focus: rgb(0xFFFFFF),
    sidebar_bg: rgb(0x2F3235),
    sidebar_focus: rgb(0x35383B),

    foreground: rgb(0x2B2B2D),
    muted: rgb(0x6B6B6D),
    sidebar_fg: rgb(0xD8DADC),
    sidebar_muted: rgb(0x9A9DA0),

    border: rgb(0xE3E4E6),
    sidebar_border: rgb(0x2F3235),
    header_bg: rgb(0xF3F5F7),
    header_fg: rgb(0x2B2B2D),
    sidebar_header_bg: rgb(0x3A3D40),
    sidebar_header_fg: rgb(0xD8DADC),
    header_focus_bg: rgb(0xCD5654),
    header_focus_fg: rgb(0xFFFFFF),
    cursor_bg: rgb(0xCD5654),
    cursor_fg: rgb(0xFFFFFF),
    cursor_blur_bg: rgb(0xF3F5F7),
    sidebar_cursor_blur_bg: rgb(0x3F403F),
    cursor_blur_fg: rgb(0x2B2B2D),
    sidebar_cursor_blur_fg: rgb(0xEDEEEF),
    footer_bg: rgb(0xF3F5F7),
    footer_fg: rgb(0x4A4A4C),
    footer_key: rgb(0xCD5654),

    accent: rgb(0xCD5654),
    primary: rgb(0xCD5654),
    success: rgb(0x3F9D63),
    warning: rgb(0xB7791F),
    error: rgb(0xC0392B),

    heading: rgb(0x2B2B2D),
    heading_alt: rgb(0x2B2B2D),
    link: rgb(0xCD5654),
    bullet: rgb(0xCD5654),
    code_fg: rgb(0x4A4A4C),
    code_bg: rgb(0xE4E5E6),
    tag_fg: rgb(0x6B6B6D),
    tag_bg: rgb(0xE4E5E6),
};

/// Red Graphite in the dark: the same coral red over Bear's graphite, for a
/// dark terminal. The sidebar is a shade below the page, as it is in Bear.
pub const RED_GRAPHITE_DARK: Theme = Theme {
    name: "red-graphite-dark",
    dark: true,

    background: rgb(0x1B1C1E),
    surface: rgb(0x232528),
    surface_focus: rgb(0x2A2C30),
    sidebar_bg: rgb(0x1F2123),
    sidebar_focus: rgb(0x26282B),

    foreground: rgb(0xD7D9DC),
    muted: rgb(0x8A8D92),
    sidebar_fg: rgb(0xD7D9DC),
    sidebar_muted: rgb(0x8A8D92),

    border: rgb(0x3A3D42),
    sidebar_border: rgb(0x3A3D42),
    header_bg: rgb(0x2E3135),
    header_fg: rgb(0xE0736A),
    sidebar_header_bg: rgb(0x2E3135),
    sidebar_header_fg: rgb(0xE0736A),
    header_focus_bg: rgb(0xCD5654),
    header_focus_fg: rgb(0xFFF3F2),
    cursor_bg: rgb(0xCD5654),
    cursor_fg: rgb(0xFFF3F2),
    cursor_blur_bg: rgb(0x4A2F30),
    sidebar_cursor_blur_bg: rgb(0x462C2D),
    cursor_blur_fg: rgb(0xD7D9DC),
    sidebar_cursor_blur_fg: rgb(0xD7D9DC),
    footer_bg: rgb(0x2E3135),
    footer_fg: rgb(0xD7D9DC),
    footer_key: rgb(0xE0736A),

    accent: rgb(0xCD5654),
    primary: rgb(0xCD5654),
    success: rgb(0x6FB98F),
    warning: rgb(0xE0A458),
    error: rgb(0xE05C5C),

    heading: rgb(0xEDEFF2),
    heading_alt: rgb(0xD7D9DC),
    link: rgb(0xE0736A),
    bullet: rgb(0xCD5654),
    code_fg: rgb(0xE0A98F),
    code_bg: rgb(0x2E3135),
    tag_fg: rgb(0xB9BCC0),
    tag_bg: rgb(0x2E3135),
};

pub const THEMES: &[Theme] = &[TEXTUAL_DARK, RED_GRAPHITE, RED_GRAPHITE_DARK];

/// The default is Textual's, so an unconfigured Rust Bjorn matches the Python one.
pub const DEFAULT_THEME: &str = "textual-dark";

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// The theme every drawing function reads.
#[inline]
pub fn current() -> &'static Theme {
    // The index only ever comes from `set`, which bounds it.
    &THEMES[ACTIVE.load(Ordering::Relaxed)]
}

/// The theme called `name`, spelling-insensitively; `None` when there is no
/// such theme.
pub fn lookup(name: &str) -> Option<usize> {
    let wanted = name.trim().to_ascii_lowercase();
    THEMES.iter().position(|t| t.name == wanted)
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
    THEMES.iter().map(|t| t.name)
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
        assert_eq!(THEMES[lookup(DEFAULT_THEME).unwrap()].name, DEFAULT_THEME);
        assert_eq!(
            THEMES[lookup("  Red-Graphite ").unwrap()].name,
            "red-graphite"
        );
        assert!(lookup("mauve").is_none());
        assert_eq!(names().count(), THEMES.len());
    }

    #[test]
    fn every_theme_is_complete() {
        for theme in THEMES {
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
