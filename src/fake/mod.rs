//! Stand-ins for `bearcli` and `remctl`, built as their own binaries.
//!
//! They implement the subset Bjorn drives with the real tools' contracts and
//! the same state files as the Python fakes, so `bjorn --demo` needs no Bear
//! and no Python, and the Python test suite can run against them.

pub mod bearcli;
pub mod remctl;

use std::path::Path;

/// Write `text` to `path` through a sibling temp file and a rename, so a
/// concurrent reader never sees a half-written file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}
