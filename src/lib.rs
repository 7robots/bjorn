//! Bjorn: a terminal front end for Bear, talking to Bear only through `bearcli`.
//!
//! Module names mirror the Python implementation (`~/GitHub/bjorn`) so the two
//! trees can be read side by side.

pub mod bear;
pub mod config;
pub mod fake;
pub mod model;
pub mod render;
pub mod util;
