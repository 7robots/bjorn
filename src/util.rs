//! Small helpers with no better home: paths, `which`, timestamps.

use std::path::{Path, PathBuf};

/// `$HOME`, or `/` when unset. Bjorn is macOS software; there is always a home.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `~` and `~/x` expanded, anything else returned as written.
pub fn expand_tilde(text: &str) -> PathBuf {
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    PathBuf::from(text)
}

/// Is this an existing file the current user may execute?
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// `shutil.which`: a name with a slash is checked as given, otherwise `PATH`
/// is searched.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let path = expand_tilde(name);
        return is_executable(&path).then_some(path);
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// The first non-blank line, trimmed, or "".
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Text from a note, safe to put in a single-line row or a toast: control
/// characters (Unicode's Cc category, so C0 and C1 — `\r`, `\n`, `\u{1b}` and
/// the rest) are dropped, and a tab becomes a space.
///
/// A note's title and body are whatever the user, or whatever wrote into Bear,
/// put there; a stray carriage return or escape in a heading would otherwise
/// be drawn straight into the terminal. Printing characters are left alone,
/// zero-width ones included: a joiner is how an emoji or a script is spelled,
/// not a control code, and the renderer measures it correctly.
pub fn strip_control(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// bearcli's timestamp shape: `2026-09-08T13:33:55Z`.
pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_never_reach_a_row() {
        assert_eq!(strip_control("a\u{1b}[31mb\r\nc\td"), "a[31mbc d");
        assert_eq!(strip_control("plain text"), "plain text");
        assert_eq!(strip_control("résumé ☑"), "résumé ☑");
        // A zero-width joiner is text, not a control code: it is what holds a
        // family emoji together, and it stays.
        assert_eq!(strip_control("👩\u{200d}🚀"), "👩\u{200d}🚀");
    }

    #[test]
    fn tilde_expands_only_at_the_front() {
        assert_eq!(expand_tilde("~/x"), home_dir().join("x"));
        assert_eq!(expand_tilde("/a/~"), PathBuf::from("/a/~"));
        assert_eq!(expand_tilde("~"), home_dir());
    }

    #[test]
    fn which_finds_sh_and_not_nonsense() {
        assert!(which("sh").is_some());
        assert!(which("no-such-binary-xyz").is_none());
        assert!(which("/bin/sh").is_some());
    }

    #[test]
    fn first_line_skips_blanks() {
        assert_eq!(first_line("\n  \n  error here \nmore"), "error here");
        assert_eq!(first_line(""), "");
    }
}
