//! The palette.
//!
//! One `Theme` is a flat table of true colours. `red-graphite-dark`, the
//! default, and `red-graphite` are Bear's Red Graphite: graphite chrome, one
//! coral red (`#CD5654`, sampled from Bear) for every accent. `textual-dark`
//! is the original palette, a dark grey page with blue and amber accents. The
//! rest, in `palettes`, are generated from Bear's own theme files by
//! `tools/bear_theme.py`. More can be added without a rebuild: any Bear
//! `.theme` file in `themes_dir()` is offered after the built-ins, under its
//! file name (see `bear_theme`).
//!
//! The active theme is a process-wide index into the built-ins followed by the
//! user's themes, set once from the config at startup, so drawing code can read
//! it without threading a reference through every function.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::{Color, Modifier, Style};

use super::{bear_theme, palettes};

pub const fn rgb(hex: u32) -> Color {
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

/// The original palette: a dark grey page, blue for the cursor and headings,
/// amber for the focused header and the key hints.
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

pub const THEMES: &[Theme] = &[
    RED_GRAPHITE_DARK,
    RED_GRAPHITE,
    TEXTUAL_DARK,
    palettes::ACADEMIA,
    palettes::ATOM,
    palettes::AYU,
    palettes::AYU_MIRAGE,
    palettes::CATPPUCCIN_LATTE,
    palettes::CATPPUCCIN_MACCHIATO,
    palettes::CHARCOAL,
    palettes::COBALT,
    palettes::D_BORING,
    palettes::DARK_GRAPHITE,
    palettes::DARK_NOTES,
    palettes::DIECI,
    palettes::DRACULA,
    palettes::DUOTONE_HEAT,
    palettes::DUOTONE_LIGHT,
    palettes::DUOTONE_SNOW,
    palettes::EVERFOREST_DARK,
    palettes::EVERFOREST_LIGHT,
    palettes::GANDALF,
    palettes::GOTHAM,
    palettes::GRUVBOX,
    palettes::HIGH_CONTRAST,
    palettes::LIGHTHAUS,
    palettes::NORD,
    palettes::NORD_LIGHT,
    palettes::NOTES,
    palettes::OLIVE_DUNK,
    palettes::PANIC_MODE,
    palettes::PRINT,
    palettes::ROSE_PINE,
    palettes::ROSE_PINE_DAWN,
    palettes::SHIBUYA_JAZZ,
    palettes::SHIBUYA_LO_FI,
    palettes::SOLARIZED_DARK,
    palettes::SOLARIZED_LIGHT,
    palettes::TOKYO_NIGHT,
    palettes::TOKYO_NIGHT_LIGHT,
    palettes::TOOTHPASTE,
];

/// Must be `THEMES[0]`: `ACTIVE` starts at zero, so anything that draws
/// before the config is read gets this one.
pub const DEFAULT_THEME: &str = "red-graphite-dark";

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where the user's own `.theme` files live: `themes/` beside the config
/// file, so `--config` brings its own themes along. That is
/// `${XDG_CONFIG_HOME:-~/.config}/bjorn/themes` unless `use_config` named
/// another file.
pub fn themes_dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_else(|| themes_dir_for(None))
}

/// Read themes from beside `config` (the file `--config` named) rather than
/// the default. Only the first call counts, and it has to come before
/// anything asks for a theme past the built-ins.
pub fn use_config(config: Option<&Path>) {
    if config.is_some() {
        let _ = DIR.set(themes_dir_for(config));
    }
}

/// `themes/` in the directory holding `config`, or in the default config
/// directory without one.
fn themes_dir_for(config: Option<&Path>) -> PathBuf {
    let Some(config) = config else {
        return crate::config::config_dir().join("themes");
    };
    let config = crate::util::expand_tilde(&config.to_string_lossy());
    match config.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("themes"),
        _ => PathBuf::from("themes"),
    }
}

static USER: OnceLock<bear_theme::Loaded> = OnceLock::new();

/// The themes directory, loaded: the themes it offers, which exclude any a
/// built-in already names (the built-ins always win), and the files it set
/// aside. Read once, and only when something asks past the built-ins.
fn loaded() -> &'static bear_theme::Loaded {
    USER.get_or_init(|| bear_theme::load_dir(&themes_dir(), THEMES))
}

fn user() -> &'static [Theme] {
    &loaded().themes
}

/// The `.theme` files in `themes_dir()` that are not offered, and why.
pub fn skipped() -> &'static [bear_theme::Skipped] {
    &loaded().skipped
}

/// Why no theme is called `name` although a file of that name is in the
/// themes directory: the file and what is wrong with it, on one line.
/// `None` when there is such a theme, or no such file.
pub fn why_not(name: &str) -> Option<String> {
    if lookup(name).is_some() {
        return None;
    }
    explain(name, skipped())
}

/// The line `why_not` gives, from the files set aside in `skipped`.
fn explain(name: &str, skipped: &[bear_theme::Skipped]) -> Option<String> {
    let wanted = bear_theme::slug(name);
    skipped
        .iter()
        .find(|s| s.name() == wanted)
        .map(|s| format!("{} did not load: {}", s.path.display(), s.problem))
}

/// The theme every drawing function reads.
#[inline]
pub fn current() -> &'static Theme {
    // The index only ever comes from `set`, which bounds it.
    let index = ACTIVE.load(Ordering::Relaxed);
    THEMES
        .get(index)
        .unwrap_or_else(|| &user()[index - THEMES.len()])
}

/// The theme called `name`, ignoring case, spacing and accents (`Rosé Pine`,
/// `rose-pine`); `None` when there is no such theme.
pub fn lookup(name: &str) -> Option<usize> {
    find(name, user)
}

/// `lookup` over the built-ins, then over `extra`, which is only called when
/// no built-in matches, so naming a built-in never reads the themes directory.
fn find(name: &str, extra: impl FnOnce() -> &'static [Theme]) -> Option<usize> {
    let wanted = bear_theme::slug(name);
    THEMES.iter().position(|t| t.name == wanted).or_else(|| {
        extra()
            .iter()
            .position(|t| t.name == wanted)
            .map(|i| THEMES.len() + i)
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
    THEMES.iter().chain(user()).map(|t| t.name)
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
        assert_eq!(THEMES[lookup("Rosé Pine").unwrap()].name, "rose-pine");
        assert_eq!(THEMES[lookup("D.Boring").unwrap()].name, "d-boring");
    }

    #[test]
    fn built_in_names_never_read_the_themes_directory() {
        let untouched = || -> &'static [Theme] { panic!("the themes directory was read") };
        for theme in THEMES {
            assert!(find(&theme.name.to_uppercase(), untouched).is_some());
        }
        assert_eq!(find("mauve", || &[]), None);
        assert_eq!(find("mauve", || &THEMES[..1]), None);
        assert_eq!(find("red-graphite-dark", || &[]), Some(0));
    }

    /// `--config` brings its own themes directory: `themes/` beside the file.
    #[test]
    fn the_themes_directory_sits_beside_the_config_file() {
        assert_eq!(
            themes_dir_for(None),
            crate::config::config_dir().join("themes")
        );
        assert_eq!(
            themes_dir_for(Some(Path::new("/etc/bjorn/work.toml"))),
            Path::new("/etc/bjorn/themes")
        );
        assert_eq!(
            themes_dir_for(Some(Path::new("~/dots/bjorn.toml"))),
            crate::util::home_dir().join("dots/themes")
        );
        assert_eq!(
            themes_dir_for(Some(Path::new("bjorn.toml"))),
            Path::new("themes")
        );
    }

    /// A theme that exists as a file but did not load is explained, by the
    /// file and the reason, under any spelling of its name.
    #[test]
    fn a_file_that_did_not_load_is_explained_by_name() {
        let skipped = [bear_theme::Skipped {
            path: PathBuf::from("/t/My Nord.theme"),
            problem: bear_theme::Problem::MissingColor("base.accent color".into()),
        }];
        assert_eq!(
            explain("MY NORD", &skipped).as_deref(),
            Some("/t/My Nord.theme did not load: missing color `base.accent color`")
        );
        assert_eq!(explain("my-nord", &skipped), explain("My Nord", &skipped));
        assert_eq!(explain("mauve", &skipped), None);
    }

    #[test]
    fn the_default_is_the_theme_drawn_before_the_config_is_read() {
        assert_eq!(THEMES[0].name, DEFAULT_THEME);
        assert_eq!(current().name, DEFAULT_THEME);
    }

    #[test]
    fn theme_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for theme in THEMES {
            assert!(seen.insert(theme.name), "duplicate theme {}", theme.name);
        }
    }

    /// Relative luminance per WCAG, for the contrast check below.
    fn luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            unreachable!()
        };
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (x, y) = (luminance(a) + 0.05, luminance(b) + 0.05);
        if x > y { x / y } else { y / x }
    }

    /// The generated palettes pick text over the accent by luminance; make
    /// sure that and the body text stay readable on every theme.
    #[test]
    fn text_is_legible_on_every_theme() {
        for theme in THEMES {
            for (what, fg, bg) in [
                ("body", theme.foreground, theme.surface),
                ("sidebar", theme.sidebar_fg, theme.sidebar_bg),
                ("cursor", theme.cursor_fg, theme.cursor_bg),
                ("blurred cursor", theme.cursor_blur_fg, theme.cursor_blur_bg),
                (
                    "sidebar blurred cursor",
                    theme.sidebar_cursor_blur_fg,
                    theme.sidebar_cursor_blur_bg,
                ),
                (
                    "focused header",
                    theme.header_focus_fg,
                    theme.header_focus_bg,
                ),
                ("footer key", theme.footer_key, theme.footer_bg),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 3.0, "{}: {what} contrast {ratio:.2}", theme.name);
            }
        }
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
