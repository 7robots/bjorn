//! The palette. Named terminal colours only, so the user's terminal theme
//! decides the exact shades, as Textual's default theme did for the Python
//! Bjorn.

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Blue;
pub const SUCCESS: Color = Color::Green;
pub const MUTED: Color = Color::DarkGray;
pub const BORDER: Color = Color::DarkGray;
pub const WARNING: Color = Color::Yellow;
pub const ERROR: Color = Color::Red;

/// Column header at rest: bold green on the panel background.
pub fn header() -> Style {
    Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)
}

/// Column header of the focused pane: filled with the accent.
pub fn header_focused() -> Style {
    Style::default()
        .bg(ACCENT)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The highlighted row of a focused pane.
pub fn cursor_focused() -> Style {
    Style::default().bg(ACCENT).fg(Color::White)
}

/// The highlighted row of a pane without focus.
pub fn cursor_unfocused() -> Style {
    Style::default().bg(Color::DarkGray).fg(Color::White)
}

pub fn border() -> Style {
    Style::default().fg(BORDER)
}

/// Search matches: theme-independent, as `reverse bold` was in the Python Bjorn.
pub fn match_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}
