//! Bjorn: a terminal front end for Bear, talking to Bear only through `bearcli`.
//!
//! Module names mirror the Python implementation (`~/GitHub/bjorn`) so the two
//! trees can be read side by side.

pub mod app;
pub mod bear;
pub mod config;
pub mod fake;
pub mod harness;
pub mod icons;
pub mod model;
pub mod render;
pub mod ui;
pub mod util;
