//! The man page must not rot.
//!
//! `docs/bjorn.1` is written by hand, which means it can drift away from the
//! code the day someone adds a key or a config key and forgets it. These two
//! tests read the authorities — `src/ui/help.rs` for the bindings, the parser
//! in `src/config.rs` for the config keys — and fail with the missing name
//! when the page has not caught up.
//!
//! Both are text checks, not renders: they say a name is *written down*, not
//! that what is written is right. Keeping the page honest past that is a
//! reading job.

use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let path = repo(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {rel}: {err}"))
}

// ---------------------------------------------------------------------------
// Key bindings
// ---------------------------------------------------------------------------

/// Bindings the page is allowed not to list, with the reason. Empty today:
/// every row of the in-app help has an entry in the page's KEYS section.
const UNDOCUMENTED_KEYS: &[(&str, &str)] = &[];

/// The body of `const SECTIONS` in `src/ui/help.rs`, from its opening `&[`
/// to the `];` that closes it at column 0.
fn sections_block(source: &str) -> &str {
    let start = source
        .find("const SECTIONS")
        .expect("src/ui/help.rs no longer declares SECTIONS");
    let rest = &source[start..];
    let open = rest.find("&[").expect("SECTIONS is not an array literal");
    let end = rest.find("\n];").expect("SECTIONS is never closed");
    &rest[open + 2..end]
}

/// Read one Rust string literal, the opening quote already consumed. Escapes
/// are unwrapped only far enough to find the closing quote; no key uses one.
fn string_literal(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            _ => out.push(c),
        }
    }
    out
}

/// Every key in `SECTIONS`, in order. The table is a list of
/// `(heading, &[(key, text)])`, so a key is the first string inside a
/// second-level paren; a row whose key is empty is a note under the row above
/// it rather than a binding, and is skipped.
fn help_bindings(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut depth = 0usize;
    // The paren depth whose first string literal has not been seen yet.
    let mut first_of: Option<usize> = None;
    let mut chars = sections_block(source).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                depth += 1;
                first_of = Some(depth);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                first_of = None;
            }
            '"' => {
                let literal = string_literal(&mut chars);
                if depth == 2 && first_of == Some(2) && !literal.is_empty() {
                    keys.push(literal);
                }
                first_of = None;
            }
            _ => {}
        }
    }
    assert!(
        keys.len() > 20,
        "only {} bindings parsed out of src/ui/help.rs — the table's shape changed",
        keys.len()
    );
    keys
}

/// The page gives a binding its own `.TP` tag line, so the check is for the
/// exact roff line rather than a substring: a single-letter key would match
/// anywhere in the prose otherwise.
fn tagged(page: &str, key: &str) -> bool {
    let wanted = format!(".B {key}");
    page.lines().any(|line| line.trim_end() == wanted)
}

#[test]
fn every_key_binding_is_in_the_man_page() {
    let page = read("docs/bjorn.1");
    let help = read("src/ui/help.rs");
    let mut missing = Vec::new();
    for key in help_bindings(&help) {
        if UNDOCUMENTED_KEYS.iter().any(|(k, _)| *k == key) {
            continue;
        }
        if !tagged(&page, &key) {
            missing.push(key);
        }
    }
    assert!(
        missing.is_empty(),
        "docs/bjorn.1 does not document these keys from src/ui/help.rs: {}\n\
         Each one wants a `.TP` / `.B <key>` pair in the KEYS section, or an \
         entry in UNDOCUMENTED_KEYS saying why not.",
        missing.join(", ")
    );
}

// ---------------------------------------------------------------------------
// Config keys
// ---------------------------------------------------------------------------

/// Config keys the page is allowed not to list, with the reason. Empty today:
/// every key `Config::load` reads has an entry in CONFIGURATION.
const UNDOCUMENTED_CONFIG_KEYS: &[(&str, &str)] = &[];

/// `src/config.rs` up to its test module: the parser only.
fn config_parser(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}

/// Every TOML key the parser looks up. The parser reads each key straight off
/// a table (`data.get("theme")`, `entry.get("command")`). A helper that wraps
/// the lookup and takes the key as its first argument has to be named in
/// `CALLS` too, which is why the count is asserted below.
fn config_keys(source: &str) -> Vec<String> {
    const CALLS: &[&str] = &["get"];
    let source = config_parser(source);
    let bytes = source.as_bytes();
    let mut keys = Vec::new();
    for call in CALLS {
        let needle = format!("{call}(\"");
        let mut from = 0usize;
        while let Some(at) = source[from..].find(&needle) {
            let start = from + at;
            from = start + needle.len();
            // Not `budget("…")`: the call name must stand on its own.
            let before = bytes[..start].iter().next_back().copied();
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                continue;
            }
            let Some(end) = source[from..].find('"') else {
                continue;
            };
            let key = &source[from..from + end];
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                keys.push(key.to_string());
            }
        }
    }
    keys.sort();
    keys.dedup();
    assert!(
        keys.len() > 20,
        "only {} config keys parsed out of src/config.rs — has a new helper \
         been added that CALLS does not name?",
        keys.len()
    );
    keys
}

/// A config key is documented when its name appears as a whole word somewhere
/// in the page: the CONFIGURATION section writes each one as `.I <key>`, and
/// block names such as `reminders` appear as `[reminders]`.
fn mentions(page: &str, key: &str) -> bool {
    let is_part = |c: char| c.is_ascii_alphanumeric() || c == '_';
    page.match_indices(key).any(|(at, _)| {
        let before = page[..at].chars().next_back();
        let after = page[at + key.len()..].chars().next();
        !before.is_some_and(is_part) && !after.is_some_and(is_part)
    })
}

#[test]
fn every_config_key_is_in_the_man_page() {
    let page = read("docs/bjorn.1");
    let config = read("src/config.rs");
    let mut missing = Vec::new();
    for key in config_keys(&config) {
        if UNDOCUMENTED_CONFIG_KEYS.iter().any(|(k, _)| *k == key) {
            continue;
        }
        if !mentions(&page, &key) {
            missing.push(key);
        }
    }
    assert!(
        missing.is_empty(),
        "docs/bjorn.1 does not document these config keys from src/config.rs: {}\n\
         Each one wants an entry under CONFIGURATION with its default, or an \
         entry in UNDOCUMENTED_CONFIG_KEYS saying why not.",
        missing.join(", ")
    );
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// Cheap guards against the two ways a hand-written roff file goes wrong
/// without anyone noticing: a trailing space (invisible, and it changes how a
/// line is filled) and a missing section.
#[test]
fn the_page_has_its_sections_and_no_trailing_whitespace() {
    let page = read("docs/bjorn.1");
    for (number, line) in page.lines().enumerate() {
        assert!(
            line.trim_end() == line,
            "docs/bjorn.1:{}: trailing whitespace",
            number + 1
        );
    }
    for heading in [
        "NAME",
        "SYNOPSIS",
        "DESCRIPTION",
        "OPTIONS",
        "KEYS",
        "VIEWS AND THE WORKSPACE",
        "SEARCH",
        "WIKI LINKS",
        "TODO TRIAGE",
        "EXPORT",
        "ACTIONS",
        "THEMES",
        "CONFIGURATION",
        "ENVIRONMENT",
        "FILES",
        "EXAMPLES",
        "EXIT STATUS",
        "SEE ALSO",
        "LIMITATIONS",
    ] {
        let wanted = format!(".SH {heading}");
        assert!(
            page.lines().any(|line| line.trim_end() == wanted),
            "docs/bjorn.1 has no {wanted}"
        );
    }
}
