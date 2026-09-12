//! Bjorn: a terminal front end for Bear, talking to Bear only through `bearcli`.
//!
//! Module names mirror the Python implementation (`~/GitHub/bjorn`) so the two
//! trees can be read side by side.

pub mod app;
pub mod bear;
pub mod config;
pub mod editor;
pub mod export;
pub mod fake;
pub mod harness;
pub mod icons;
pub mod model;
pub mod pty;
pub mod reminders;
pub mod render;
pub mod render_html;
pub mod search;
pub mod search_box;
pub mod todos;
pub mod ui;
pub mod util;
