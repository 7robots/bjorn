//! Themes in Bear's `.theme` format, read from a directory the user owns:
//! `~/.config/bjorn/themes/` (see `theme::themes_dir`).
//!
//! A theme file is JSON: sections (`base`, `sidebar`, `notes`, `editor`)
//! holding `"… color": "#RRGGBB"` values or `"$section.key"` references to
//! other values. A file may name another theme under `meta."base theme"`; its
//! values are laid over that theme's before any reference is resolved, so a
//! base theme's `$base.accent color` picks up the child's accent.
//!
//! A base theme is looked for in the same directory first, then among the
//! built-in themes (`built_in_document`), so a file copied out of Bear.app
//! works on its own even though Bear's own files are not shipped here.
//!
//! Each file becomes one `Theme`, named after the file (`Rosé Pine.theme` is
//! `rose-pine`). The mapping onto Bjorn's palette is the one
//! `tools/bear_theme.py` uses to generate `palettes.rs`, so a theme file
//! dropped in the directory draws exactly as it would built in; the test
//! `bear_theme_files_match_the_built_in_palettes` holds the two together
//! wherever Bear.app is installed.
//!
//! Loading only reads: files are opened with `read(true)` alone, only
//! `*.theme` entries at the top of the directory are touched (no recursion),
//! and each is capped at `MAX_BYTES`. A symlink is followed, since dotfile
//! managers such as stow and chezmoi link files into place, but what it
//! points at must be a regular file within the cap too. A file that does not
//! load is set aside with the reason (`Skipped`), for `--list-themes` and
//! `--theme` to report.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::ui::palettes;
use crate::ui::theme::Theme;

/// References and `base theme` chains deeper than this are treated as loops.
const MAX_DEPTH: usize = 16;

/// Bear's theme files are about 5 KB; anything past this is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// Why a theme file did not load, worded to follow its path on one line.
///
/// Most of what goes into one comes out of the file (a key, a value, a base
/// theme's name) and is printed straight to the terminal by `--list-themes`
/// and `--theme`, so every such piece goes through `escaped`: a value like
/// `"$\u{1b}]52;c;…"` would otherwise reach the terminal as an escape
/// sequence, here one that writes to the clipboard.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Problem {
    #[error("cannot read it: {}", escaped(.0))]
    Unreadable(String),
    #[error("not a regular file")]
    NotAFile,
    #[error("larger than {MAX_BYTES} bytes")]
    TooLarge,
    #[error("the file name gives no theme name")]
    NoName,
    #[error("bad JSON: {}", escaped(.0))]
    BadJson(String),
    #[error("bad JSON: the file is not a JSON object")]
    NotAnObject,
    #[error("missing color `{}`", escaped(.0))]
    MissingColor(String),
    #[error("`{}` refers to `${}`, which is not there", escaped(.key), escaped(.target))]
    DanglingReference { key: String, target: String },
    #[error("`{}` is not a color: {}", escaped(.0), escaped(.1))]
    BadColor(String, String),
    #[error("reference loop: `{}` never reaches a color", escaped(.0))]
    ReferenceLoop(String),
    #[error(
        "base theme \"{}\" not found (no such file here, and no built-in theme by that name)",
        escaped(.0)
    )]
    BaseNotFound(String),
    #[error("base theme \"{}\" did not load: {}", escaped(.0), .1)]
    BaseBroken(String, Box<Problem>),
    #[error("base theme \"{}\" leads back to this theme", escaped(.0))]
    BaseLoop(String),
    #[error("`{}` is already the name of {}, which comes first", .0, escaped(.1))]
    Duplicate(String, String),
    #[error("`{0}` is a built-in theme, and a built-in name always wins")]
    BuiltIn(String),
}

/// `text` safe to print to a terminal: control characters (C0, DEL, C1) and
/// the bidirectional overrides and isolates (U+202A-202E, U+2066-2069, and
/// the marks U+200E, U+200F, U+061C) written out as `\u{1b}`, so the reader
/// sees what the file holds instead of the terminal acting on it or
/// reordering the line. Everything else, `\` included, is left as it is.
pub fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let bidi = matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
        );
        if c.is_control() || bidi {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

impl Problem {
    /// True when the color simply is not there, as opposed to being broken:
    /// an optional key that is absent falls back to a color the theme has.
    fn is_absent(&self) -> bool {
        matches!(
            self,
            Problem::MissingColor(_) | Problem::DanglingReference { .. }
        )
    }
}

/// A `.theme` file that is not offered, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub path: PathBuf,
    pub problem: Problem,
}

impl Skipped {
    /// The theme name the file would have gone by.
    pub fn name(&self) -> String {
        slug(
            &self
                .path
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default(),
        )
    }

    /// The file's path, safe to print: a file name can hold control
    /// characters too (see `escaped`).
    pub fn shown_path(&self) -> String {
        escaped(&self.path.display().to_string())
    }
}

/// What a directory of theme files came to.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// Every theme that loaded, sorted by name.
    pub themes: Vec<Theme>,
    /// Every `.theme` file that did not, in file name order.
    pub skipped: Vec<Skipped>,
}

/// The name a theme file goes by: lowercase, words joined by `-`, accents
/// folded away (`Rosé Pine` is `rose-pine`, `D.Boring` is `d-boring`). macOS
/// stores file names decomposed (`e` + U+0301) while a typed `é` is one
/// character, so folding both to `e` is what lets the two meet.
///
/// This is `slug` in `tools/bear_theme.py`, which names the built-in themes:
/// NFKD, then whatever is not ASCII dropped. Rust's standard library has no
/// normalization, so `NFKD_ASCII` carries the part of that mapping a theme
/// name can plausibly hold, and `LETTERS` the letters NFKD leaves whole
/// (`ø`, `ł`, `ß`), which both sides fold by hand rather than drop.
pub fn slug(name: &str) -> String {
    let mut folded = String::new();
    for c in name.to_lowercase().chars() {
        let ascii = match c {
            c if c.is_ascii() => Some(c.to_string()),
            // Fullwidth ASCII (`Ｎｏｒｄ`) is ASCII shifted up by 0xFEE0.
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).map(String::from),
            '\u{3000}' => Some(" ".to_string()),
            c => LETTERS
                .iter()
                .chain(NFKD_ASCII)
                .find(|(from, _)| *from == c)
                .map(|(_, to)| to.to_string()),
        };
        // Anything else, combining accents included, is dropped.
        for c in ascii.iter().flat_map(|s| s.chars()) {
            folded.push(if c.is_ascii_alphanumeric() { c } else { ' ' });
        }
    }
    folded.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Letters NFKD does not decompose, so a plain NFKD fold would drop them:
/// `Bjørn` would be `bjrn`. `LETTERS` in `tools/bear_theme.py` is the same list.
const LETTERS: &[(char, &str)] = &[
    ('ß', "ss"),
    ('æ', "ae"),
    ('œ', "oe"),
    ('ø', "o"),
    ('ð', "d"),
    ('đ', "d"),
    ('ħ', "h"),
    ('ı', "i"),
    ('ł', "l"),
    ('ŧ', "t"),
    ('þ', "th"),
];

/// Lowercase characters in Latin-1 Supplement, Latin Extended-A and -B,
/// Latin Extended Additional, General Punctuation and the Latin ligatures,
/// each with the ASCII its NFKD form keeps (Python's
/// `unicodedata.normalize("NFKD", c).encode("ascii", "ignore")`, Unicode 15.1).
/// Characters that keep nothing are left out, and so dropped.
#[rustfmt::skip]
const NFKD_ASCII: &[(char, &str)] = &[
    ('\u{a0}', " "), ('\u{a8}', " "), ('\u{aa}', "a"), ('\u{af}', " "), ('\u{b2}', "2"),
    ('\u{b3}', "3"), ('\u{b4}', " "), ('\u{b8}', " "), ('\u{b9}', "1"), ('\u{ba}', "o"),
    ('\u{bc}', "14"), ('\u{bd}', "12"), ('\u{be}', "34"), ('\u{e0}', "a"), ('\u{e1}', "a"),
    ('\u{e2}', "a"), ('\u{e3}', "a"), ('\u{e4}', "a"), ('\u{e5}', "a"), ('\u{e7}', "c"),
    ('\u{e8}', "e"), ('\u{e9}', "e"), ('\u{ea}', "e"), ('\u{eb}', "e"), ('\u{ec}', "i"),
    ('\u{ed}', "i"), ('\u{ee}', "i"), ('\u{ef}', "i"), ('\u{f1}', "n"), ('\u{f2}', "o"),
    ('\u{f3}', "o"), ('\u{f4}', "o"), ('\u{f5}', "o"), ('\u{f6}', "o"), ('\u{f9}', "u"),
    ('\u{fa}', "u"), ('\u{fb}', "u"), ('\u{fc}', "u"), ('\u{fd}', "y"), ('\u{ff}', "y"),
    ('\u{101}', "a"), ('\u{103}', "a"), ('\u{105}', "a"), ('\u{107}', "c"), ('\u{109}', "c"),
    ('\u{10b}', "c"), ('\u{10d}', "c"), ('\u{10f}', "d"), ('\u{113}', "e"), ('\u{115}', "e"),
    ('\u{117}', "e"), ('\u{119}', "e"), ('\u{11b}', "e"), ('\u{11d}', "g"), ('\u{11f}', "g"),
    ('\u{121}', "g"), ('\u{123}', "g"), ('\u{125}', "h"), ('\u{129}', "i"), ('\u{12b}', "i"),
    ('\u{12d}', "i"), ('\u{12f}', "i"), ('\u{133}', "ij"), ('\u{135}', "j"), ('\u{137}', "k"),
    ('\u{13a}', "l"), ('\u{13c}', "l"), ('\u{13e}', "l"), ('\u{140}', "l"), ('\u{144}', "n"),
    ('\u{146}', "n"), ('\u{148}', "n"), ('\u{149}', "n"), ('\u{14d}', "o"), ('\u{14f}', "o"),
    ('\u{151}', "o"), ('\u{155}', "r"), ('\u{157}', "r"), ('\u{159}', "r"), ('\u{15b}', "s"),
    ('\u{15d}', "s"), ('\u{15f}', "s"), ('\u{161}', "s"), ('\u{163}', "t"), ('\u{165}', "t"),
    ('\u{169}', "u"), ('\u{16b}', "u"), ('\u{16d}', "u"), ('\u{16f}', "u"), ('\u{171}', "u"),
    ('\u{173}', "u"), ('\u{175}', "w"), ('\u{177}', "y"), ('\u{17a}', "z"), ('\u{17c}', "z"),
    ('\u{17e}', "z"), ('\u{17f}', "s"), ('\u{1a1}', "o"), ('\u{1b0}', "u"), ('\u{1c6}', "dz"),
    ('\u{1c9}', "lj"), ('\u{1cc}', "nj"), ('\u{1ce}', "a"), ('\u{1d0}', "i"), ('\u{1d2}', "o"),
    ('\u{1d4}', "u"), ('\u{1d6}', "u"), ('\u{1d8}', "u"), ('\u{1da}', "u"), ('\u{1dc}', "u"),
    ('\u{1df}', "a"), ('\u{1e1}', "a"), ('\u{1e7}', "g"), ('\u{1e9}', "k"), ('\u{1eb}', "o"),
    ('\u{1ed}', "o"), ('\u{1f0}', "j"), ('\u{1f3}', "dz"), ('\u{1f5}', "g"), ('\u{1f9}', "n"),
    ('\u{1fb}', "a"), ('\u{201}', "a"), ('\u{203}', "a"), ('\u{205}', "e"), ('\u{207}', "e"),
    ('\u{209}', "i"), ('\u{20b}', "i"), ('\u{20d}', "o"), ('\u{20f}', "o"), ('\u{211}', "r"),
    ('\u{213}', "r"), ('\u{215}', "u"), ('\u{217}', "u"), ('\u{219}', "s"), ('\u{21b}', "t"),
    ('\u{21f}', "h"), ('\u{227}', "a"), ('\u{229}', "e"), ('\u{22b}', "o"), ('\u{22d}', "o"),
    ('\u{22f}', "o"), ('\u{231}', "o"), ('\u{233}', "y"), ('\u{1e01}', "a"), ('\u{1e03}', "b"),
    ('\u{1e05}', "b"), ('\u{1e07}', "b"), ('\u{1e09}', "c"), ('\u{1e0b}', "d"), ('\u{1e0d}', "d"),
    ('\u{1e0f}', "d"), ('\u{1e11}', "d"), ('\u{1e13}', "d"), ('\u{1e15}', "e"), ('\u{1e17}', "e"),
    ('\u{1e19}', "e"), ('\u{1e1b}', "e"), ('\u{1e1d}', "e"), ('\u{1e1f}', "f"), ('\u{1e21}', "g"),
    ('\u{1e23}', "h"), ('\u{1e25}', "h"), ('\u{1e27}', "h"), ('\u{1e29}', "h"), ('\u{1e2b}', "h"),
    ('\u{1e2d}', "i"), ('\u{1e2f}', "i"), ('\u{1e31}', "k"), ('\u{1e33}', "k"), ('\u{1e35}', "k"),
    ('\u{1e37}', "l"), ('\u{1e39}', "l"), ('\u{1e3b}', "l"), ('\u{1e3d}', "l"), ('\u{1e3f}', "m"),
    ('\u{1e41}', "m"), ('\u{1e43}', "m"), ('\u{1e45}', "n"), ('\u{1e47}', "n"), ('\u{1e49}', "n"),
    ('\u{1e4b}', "n"), ('\u{1e4d}', "o"), ('\u{1e4f}', "o"), ('\u{1e51}', "o"), ('\u{1e53}', "o"),
    ('\u{1e55}', "p"), ('\u{1e57}', "p"), ('\u{1e59}', "r"), ('\u{1e5b}', "r"), ('\u{1e5d}', "r"),
    ('\u{1e5f}', "r"), ('\u{1e61}', "s"), ('\u{1e63}', "s"), ('\u{1e65}', "s"), ('\u{1e67}', "s"),
    ('\u{1e69}', "s"), ('\u{1e6b}', "t"), ('\u{1e6d}', "t"), ('\u{1e6f}', "t"), ('\u{1e71}', "t"),
    ('\u{1e73}', "u"), ('\u{1e75}', "u"), ('\u{1e77}', "u"), ('\u{1e79}', "u"), ('\u{1e7b}', "u"),
    ('\u{1e7d}', "v"), ('\u{1e7f}', "v"), ('\u{1e81}', "w"), ('\u{1e83}', "w"), ('\u{1e85}', "w"),
    ('\u{1e87}', "w"), ('\u{1e89}', "w"), ('\u{1e8b}', "x"), ('\u{1e8d}', "x"), ('\u{1e8f}', "y"),
    ('\u{1e91}', "z"), ('\u{1e93}', "z"), ('\u{1e95}', "z"), ('\u{1e96}', "h"), ('\u{1e97}', "t"),
    ('\u{1e98}', "w"), ('\u{1e99}', "y"), ('\u{1e9a}', "a"), ('\u{1e9b}', "s"), ('\u{1ea1}', "a"),
    ('\u{1ea3}', "a"), ('\u{1ea5}', "a"), ('\u{1ea7}', "a"), ('\u{1ea9}', "a"), ('\u{1eab}', "a"),
    ('\u{1ead}', "a"), ('\u{1eaf}', "a"), ('\u{1eb1}', "a"), ('\u{1eb3}', "a"), ('\u{1eb5}', "a"),
    ('\u{1eb7}', "a"), ('\u{1eb9}', "e"), ('\u{1ebb}', "e"), ('\u{1ebd}', "e"), ('\u{1ebf}', "e"),
    ('\u{1ec1}', "e"), ('\u{1ec3}', "e"), ('\u{1ec5}', "e"), ('\u{1ec7}', "e"), ('\u{1ec9}', "i"),
    ('\u{1ecb}', "i"), ('\u{1ecd}', "o"), ('\u{1ecf}', "o"), ('\u{1ed1}', "o"), ('\u{1ed3}', "o"),
    ('\u{1ed5}', "o"), ('\u{1ed7}', "o"), ('\u{1ed9}', "o"), ('\u{1edb}', "o"), ('\u{1edd}', "o"),
    ('\u{1edf}', "o"), ('\u{1ee1}', "o"), ('\u{1ee3}', "o"), ('\u{1ee5}', "u"), ('\u{1ee7}', "u"),
    ('\u{1ee9}', "u"), ('\u{1eeb}', "u"), ('\u{1eed}', "u"), ('\u{1eef}', "u"), ('\u{1ef1}', "u"),
    ('\u{1ef3}', "y"), ('\u{1ef5}', "y"), ('\u{1ef7}', "y"), ('\u{1ef9}', "y"), ('\u{2000}', " "),
    ('\u{2001}', " "), ('\u{2002}', " "), ('\u{2003}', " "), ('\u{2004}', " "), ('\u{2005}', " "),
    ('\u{2006}', " "), ('\u{2007}', " "), ('\u{2008}', " "), ('\u{2009}', " "), ('\u{200a}', " "),
    ('\u{2017}', " "), ('\u{2024}', "."), ('\u{2025}', ".."), ('\u{2026}', "..."), ('\u{202f}', " "),
    ('\u{203c}', "!!"), ('\u{203e}', " "), ('\u{2047}', "??"), ('\u{2048}', "?!"), ('\u{2049}', "!?"),
    ('\u{205f}', " "), ('\u{fb00}', "ff"), ('\u{fb01}', "fi"), ('\u{fb02}', "fl"), ('\u{fb03}', "ffi"),
    ('\u{fb04}', "ffl"), ('\u{fb05}', "st"), ('\u{fb06}', "st"),
];

/// One `.theme` file, read but not yet merged with its base.
struct File {
    stem: String,
    path: PathBuf,
    doc: Result<Value, Problem>,
}

/// Every theme file in `dir`, loaded against `built_ins`: a file named like a
/// built-in is set aside (the built-in wins), and a base theme the directory
/// does not have is looked up among them. A missing directory is no themes.
///
/// Files are taken in file name order, so when two slug to the same name the
/// first of them that loads is the one offered, whatever order the directory
/// lists them in; the other is reported.
pub fn load_dir(dir: &Path, built_ins: &[Theme]) -> Loaded {
    let mut loaded = Loaded::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return loaded;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "theme"))
        .collect();
    paths.sort();

    let mut files = Vec::new();
    for path in paths {
        match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) if !slug(stem).is_empty() => files.push(File {
                stem: stem.to_string(),
                doc: read_json(&path),
                path,
            }),
            _ => loaded.skipped.push(Skipped {
                path,
                problem: Problem::NoName,
            }),
        }
    }

    // Name -> the file offered under it.
    let mut offered: HashMap<String, String> = HashMap::new();
    for file in &files {
        let name = slug(&file.stem);
        let theme = if built_ins.iter().any(|t| t.name == name) {
            Err(Problem::BuiltIn(name.clone()))
        } else if let Some(first) = offered.get(&name) {
            Err(Problem::Duplicate(name.clone(), first.clone()))
        } else {
            file.doc
                .clone()
                .and_then(|doc| merged(&files, built_ins, &file.stem, &doc, &mut Vec::new()))
                .and_then(|doc| to_theme(&name, &doc))
        };
        match theme {
            Ok(theme) => {
                offered.insert(name, file_name(&file.path));
                loaded.themes.push(theme);
            }
            Err(problem) => loaded.skipped.push(Skipped {
                path: file.path.clone(),
                problem,
            }),
        }
    }
    loaded.themes.sort_by(|a, b| a.name.cmp(b.name));
    loaded
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One theme file, opened read-only. A symlink is followed; what it leads to
/// must be a regular file under `MAX_BYTES`, checked before it is opened (a
/// FIFO would block the open) and again on the open file, so that what is
/// read is what was checked even if the path was swapped in between.
///
/// What that second check cannot catch is a FIFO swapped in during the gap:
/// the open itself then waits for a writer. `O_NONBLOCK` would close it, but
/// it needs `libc` for the flag, which this crate does not depend on, and
/// the window is only open to someone who can already write to the user's
/// own themes directory. The cost is a hang at startup, not a wrong read.
fn read_json(path: &Path) -> Result<Value, Problem> {
    let unreadable = |e: std::io::Error| Problem::Unreadable(e.to_string());
    let checked = |meta: std::fs::Metadata| {
        if !meta.is_file() {
            Err(Problem::NotAFile)
        } else if meta.len() > MAX_BYTES {
            Err(Problem::TooLarge)
        } else {
            Ok(())
        }
    };
    checked(std::fs::metadata(path).map_err(unreadable)?)?;
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(unreadable)?;
    checked(file.metadata().map_err(unreadable)?)?;
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(unreadable)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Problem::TooLarge);
    }
    match serde_json::from_str(&text) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err(Problem::NotAnObject),
        Err(e) => Err(Problem::BadJson(e.to_string())),
    }
}

/// `doc` laid over its `base theme`, recursively. `chain` holds the themes
/// already on the way here, so a base that leads back is caught.
///
/// The base is looked for among `files` by file name, then by slug, and only
/// then among `built_ins`: a file in the directory is the real thing, while a
/// built-in has to be turned back into a theme file (`built_in_document`).
fn merged(
    files: &[File],
    built_ins: &[Theme],
    name: &str,
    doc: &Value,
    chain: &mut Vec<String>,
) -> Result<Value, Problem> {
    let Some(parent) = doc.pointer("/meta/base theme").and_then(Value::as_str) else {
        return Ok(doc.clone());
    };
    let wanted = slug(parent);
    // A file naming itself as its base has no base.
    if wanted == slug(name) {
        return Ok(doc.clone());
    }
    if chain.iter().any(|n| slug(n) == wanted) || chain.len() >= MAX_DEPTH {
        return Err(Problem::BaseLoop(parent.to_string()));
    }
    let file = files
        .iter()
        .find(|f| f.stem == parent)
        .or_else(|| files.iter().find(|f| slug(&f.stem) == wanted));
    let base = match file {
        Some(file) => {
            let base_doc = file
                .doc
                .as_ref()
                .map_err(|p| Problem::BaseBroken(parent.to_string(), Box::new(p.clone())))?;
            chain.push(name.to_string());
            let base = merged(files, built_ins, &file.stem, base_doc, chain);
            chain.pop();
            base.map_err(|p| match p {
                // Already names the theme it loops through.
                Problem::BaseLoop(_) => p,
                p => Problem::BaseBroken(parent.to_string(), Box::new(p)),
            })?
        }
        None => match BEAR_BASES
            .iter()
            .chain(built_ins)
            .find(|t| t.name == wanted)
        {
            Some(theme) => built_in_document(theme),
            None => return Err(Problem::BaseNotFound(parent.to_string())),
        },
    };
    Ok(overlay(&base, doc))
}

/// Bear's own colors for the themes Bjorn offers hand-tuned instead
/// (`tools/bear_theme.py` generates them, `THEMES` leaves them out). Seven of
/// Bear's themes name Red Graphite as their base, and were written against
/// these colors, not the hand-tuned ones.
const BEAR_BASES: &[Theme] = &[palettes::BEAR_RED_GRAPHITE];

/// A built-in theme written back out as a Bear theme file, to serve as the
/// base of a file that names it. Bear's own files are not shipped, so this is
/// what makes a theme copied out of Bear.app (Academia, over Dark Graphite)
/// load on its own.
///
/// Every key `to_theme` reads is given the built-in's color. What matters for
/// a child, though, is less a base's colors than its references: Bear's
/// bases say `"list marker color": "$base.accent color"`, so a child that
/// sets only its accent recolors its list markers too. A palette has lost
/// those references, so each key that could be one is written as the
/// reference Bear's bases use for it whenever the built-in's color is what
/// that reference gives, and as a plain color otherwise. For the palettes
/// generated from Bear's files this reproduces the base's behavior wherever
/// its children rely on it; `a_copied_bear_theme_loads_over_its_built_in_base`
/// checks each of Bear's own children against its palette.
///
/// The toast colors go back as highlighter colors, the keys they are derived
/// from. `recolor` sets a lightness and keeps the hue, so a toast color comes
/// back as itself, and moves to the child's lightness if the child is dark
/// where its base is light.
fn built_in_document(theme: &Theme) -> Value {
    let keys: [(&str, Color, &[&str]); 23] = [
        ("base.background color", theme.background, &[]),
        ("base.text color", theme.foreground, &[]),
        ("base.accent color", theme.accent, &[]),
        ("base.text secondary color", theme.muted, &[]),
        ("base.background secondary color", theme.header_bg, &[]),
        // Only ever reached as a reference; Bear's tag background uses it.
        ("base.background tertiary color", theme.tag_bg, &[]),
        ("base.stroke color", theme.border, &[]),
        ("sidebar.background color", theme.sidebar_bg, &[]),
        ("sidebar.text color", theme.sidebar_fg, &[]),
        (
            "sidebar.text secondary color",
            theme.sidebar_cursor_blur_fg,
            &[],
        ),
        (
            "sidebar.background secondary color",
            theme.sidebar_cursor_blur_bg,
            &[],
        ),
        (
            "sidebar.stroke color",
            theme.sidebar_border,
            &["sidebar.background color", "base.stroke color"],
        ),
        (
            "notes.selection background color",
            theme.cursor_blur_bg,
            &["base.background secondary color"],
        ),
        (
            "editor.headers.text color",
            theme.heading,
            &["base.text color"],
        ),
        ("editor.link color", theme.link, &["base.accent color"]),
        (
            "editor.list marker color",
            theme.bullet,
            &["base.accent color"],
        ),
        (
            "editor.code.text color",
            theme.code_fg,
            &["base.text color"],
        ),
        (
            "editor.code.background color",
            theme.code_bg,
            &["base.background secondary color"],
        ),
        ("editor.tag.text color", theme.tag_fg, &["base.text color"]),
        (
            "editor.tag.background color",
            theme.tag_bg,
            &["base.background tertiary color"],
        ),
        (
            "editor.highlighter.green.background color",
            theme.success,
            &[],
        ),
        (
            "editor.highlighter.yellow.background color",
            theme.warning,
            &[],
        ),
        ("editor.highlighter.red.background color", theme.error, &[]),
    ];

    let mut colors: HashMap<&str, Color> = HashMap::new();
    let mut doc = Value::Object(Map::new());
    for (key, color, references) in keys {
        let Color::Rgb(r, g, b) = color else {
            continue;
        };
        let value = match references.iter().find(|r| colors.get(*r) == Some(&color)) {
            Some(reference) => format!("${reference}"),
            None => format!("#{r:02X}{g:02X}{b:02X}"),
        };
        colors.insert(key, color);

        let mut node = &mut doc;
        let mut segments = key.split('.').peekable();
        while let Some(segment) = segments.next() {
            let Value::Object(map) = node else {
                break;
            };
            if segments.peek().is_none() {
                map.insert(segment.to_string(), Value::String(value.clone()));
                break;
            }
            node = map
                .entry(segment)
                .or_insert_with(|| Value::Object(Map::new()));
        }
    }
    doc
}

/// `over` laid on `base`: objects merge key by key, anything else replaces.
fn overlay(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out: Map<String, Value> = b.clone();
            for (key, value) in o {
                let next = match b.get(key) {
                    Some(existing) => overlay(existing, value),
                    None => value.clone(),
                };
                out.insert(key.clone(), next);
            }
            Value::Object(out)
        }
        _ => over.clone(),
    }
}

type Rgb = (u8, u8, u8);

/// The color at a dotted `section.key` path, following `$` references. An
/// absent key (or a reference to one) is `MissingColor` or
/// `DanglingReference`, which an optional key shrugs off; a value that is not
/// a color, or references that never reach one, is an error for any key.
fn lookup(root: &Value, key: &str) -> Result<Rgb, Problem> {
    let mut path = key.to_string();
    for _ in 0..=MAX_DEPTH {
        let mut node = Some(root);
        for segment in path.split('.') {
            node = node.and_then(|n| n.get(segment));
        }
        let Some(node) = node else {
            return Err(if path == key {
                Problem::MissingColor(path)
            } else {
                Problem::DanglingReference {
                    key: key.to_string(),
                    target: path,
                }
            });
        };
        let Some(text) = node.as_str().map(str::trim) else {
            return Err(Problem::BadColor(path, node.to_string()));
        };
        match text.strip_prefix('$') {
            Some(reference) => path = reference.to_string(),
            None => {
                return parse_hex(text).ok_or_else(|| Problem::BadColor(path, format!("{text:?}")));
            }
        }
    }
    Err(Problem::ReferenceLoop(key.to_string()))
}

/// `#RGB`, `#RRGGBB`, or `#RRGGBBAA` with the alpha dropped.
fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.is_ascii() {
        return None;
    }
    let digits = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits[..6].to_string(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

fn color((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

// The color arithmetic below mirrors `tools/bear_theme.py` step for step,
// down to Python's round-half-to-even, so both produce the same bytes.

/// `a` moved a fraction `t` of the way to `b`.
fn blend(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let mix = |x: u8, y: u8| {
        let (x, y) = (f64::from(x), f64::from(y));
        (x + (y - x) * t).round_ties_even() as u8
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// WCAG relative luminance, 0 (black) to 1 (white).
fn luminance((r, g, b): Rgb) -> f64 {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a) + 0.05, luminance(b) + 0.05);
    if x > y { x / y } else { y / x }
}

/// The candidate that reads best over `bg` (the first, on a tie).
fn best(bg: Rgb, candidates: &[Rgb]) -> Rgb {
    candidates.iter().copied().fold(candidates[0], |kept, c| {
        if contrast(c, bg) > contrast(kept, bg) {
            c
        } else {
            kept
        }
    })
}

/// Text over `bg`: the theme's own color that reads best, when one reads at
/// 3:1; plain white or near-black otherwise.
fn on(bg: Rgb, candidates: &[Rgb]) -> Rgb {
    let pick = best(bg, candidates);
    if contrast(pick, bg) >= 3.0 {
        pick
    } else {
        best(bg, &[(0xFF, 0xFF, 0xFF), (0x11, 0x11, 0x11)])
    }
}

/// `fg` pushed toward `toward` until it reads at 3:1 on `bg`.
fn legible(fg: Rgb, bg: Rgb, toward: Rgb) -> Rgb {
    let mut t = 0.0;
    while contrast(blend(fg, toward, t), bg) < 3.0 && t < 1.0 {
        t += 0.05;
    }
    blend(fg, toward, t)
}

/// A highlighter background's hue as a foreground: same hue, full enough
/// saturation, and a lightness that sits on the page. Python's `colorsys`.
fn recolor((r, g, b): Rgb, dark: bool) -> Rgb {
    let (r, g, b) = (
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    );
    let (maxc, minc) = (r.max(g).max(b), r.min(g).min(b));
    let (sumc, rangec) = (maxc + minc, maxc - minc);
    let (h, s) = if minc == maxc {
        (0.0, 0.0)
    } else {
        let s = if sumc / 2.0 <= 0.5 {
            rangec / sumc
        } else {
            rangec / (2.0 - maxc - minc)
        };
        let (rc, gc, bc) = (
            (maxc - r) / rangec,
            (maxc - g) / rangec,
            (maxc - b) / rangec,
        );
        let h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        ((h / 6.0).rem_euclid(1.0), s)
    };
    let l: f64 = if dark { 0.68 } else { 0.36 };
    let s = s.max(0.45);
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let m1 = 2.0 * l - m2;
    let v = |hue: f64| {
        let hue = hue.rem_euclid(1.0);
        if hue < 1.0 / 6.0 {
            m1 + (m2 - m1) * hue * 6.0
        } else if hue < 0.5 {
            m2
        } else if hue < 2.0 / 3.0 {
            m1 + (m2 - m1) * (2.0 / 3.0 - hue) * 6.0
        } else {
            m1
        }
    };
    let byte = |x: f64| (x * 255.0).round_ties_even() as u8;
    (byte(v(h + 1.0 / 3.0)), byte(v(h)), byte(v(h - 1.0 / 3.0)))
}

/// Map a merged theme file onto Bjorn's palette. Only the page background,
/// the text and the accent are required; a hand-written file that leaves the
/// rest out gets the nearest color it does have.
fn to_theme(name: &str, root: &Value) -> Result<Theme, Problem> {
    let get = |path: &str| lookup(root, path);
    let bg = get("base.background color")?;
    let text = get("base.text color")?;
    let accent = get("base.accent color")?;
    let or = |path: &str, fallback: Rgb| match get(path) {
        Err(problem) if problem.is_absent() => Ok(fallback),
        found => found,
    };

    let dark = luminance(bg) < 0.5;
    let muted = or("base.text secondary color", text)?;
    let bg2 = or("base.background secondary color", bg)?;
    let sb_bg = or("sidebar.background color", bg2)?;
    let sb_text = or("sidebar.text color", text)?;
    let headers = or("editor.headers.text color", text)?;
    let blur_bg = or("notes.selection background color", bg2)?;
    let sb_blur_bg = or("sidebar.background secondary color", sb_bg)?;
    let sb_text_2 = or("sidebar.text secondary color", sb_text)?;
    let over_accent = on(accent, &[bg, text, sb_bg]);

    let highlighter = |hue: &str, fallback: Rgb| {
        let key = format!("editor.highlighter.{hue}.background color");
        match get(&key) {
            Ok(c) => Ok(recolor(c, dark)),
            Err(problem) if problem.is_absent() => Ok(fallback),
            Err(problem) => Err(problem),
        }
    };
    let fixed = if dark {
        [(0x6F, 0xB9, 0x8F), (0xE0, 0xA4, 0x58), (0xE0, 0x5C, 0x5C)]
    } else {
        [(0x3F, 0x9D, 0x63), (0xB7, 0x79, 0x1F), (0xC0, 0x39, 0x2B)]
    };
    let [success, warning, error] = [
        highlighter("green", fixed[0])?,
        highlighter("yellow", fixed[1])?,
        highlighter("red", fixed[2])?,
    ]
    .map(|c| legible(c, bg, text));

    Ok(Theme {
        name: Box::leak(name.to_string().into_boxed_str()),
        dark,

        background: color(bg),
        surface: color(bg),
        surface_focus: color(blend(bg, text, 0.04)),
        sidebar_bg: color(sb_bg),
        sidebar_focus: color(blend(sb_bg, sb_text, 0.04)),

        foreground: color(text),
        muted: color(muted),
        sidebar_fg: color(sb_text),
        sidebar_muted: color(blend(sb_text, sb_bg, 0.35)),

        border: color(or("base.stroke color", muted)?),
        sidebar_border: color(or("sidebar.stroke color", sb_bg)?),
        header_bg: color(bg2),
        header_fg: color(headers),
        sidebar_header_bg: color(blend(sb_bg, sb_text, 0.06)),
        sidebar_header_fg: color(sb_text),
        header_focus_bg: color(accent),
        header_focus_fg: color(over_accent),
        cursor_bg: color(accent),
        cursor_fg: color(over_accent),
        cursor_blur_bg: color(blur_bg),
        cursor_blur_fg: color(on(blur_bg, &[text, headers])),
        sidebar_cursor_blur_bg: color(sb_blur_bg),
        sidebar_cursor_blur_fg: color(on(sb_blur_bg, &[sb_text_2, sb_text, text])),
        footer_bg: color(bg2),
        footer_fg: color(muted),
        footer_key: color(legible(accent, bg2, text)),

        accent: color(accent),
        primary: color(accent),
        success: color(success),
        warning: color(warning),
        error: color(error),

        heading: color(headers),
        heading_alt: color(text),
        link: color(or("editor.link color", accent)?),
        bullet: color(or("editor.list marker color", accent)?),
        code_fg: color(or("editor.code.text color", text)?),
        code_bg: color(or("editor.code.background color", bg2)?),
        tag_fg: color(or("editor.tag.text color", text)?),
        tag_bg: color(or("editor.tag.background color", bg2)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::THEMES;

    /// Where Bear keeps its theme files; the same path `tools/bear_theme.py`
    /// reads. Only tests look here: at runtime Bjorn never reads Bear.app.
    const BEAR: &str =
        "/Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources";

    /// Hand-tuned in theme.rs rather than generated.
    const HAND_TUNED: &[&str] = &["red-graphite", "red-graphite-dark", "textual-dark"];

    /// The generator names these themes' toast colors from the palette they
    /// are named after (`SEMANTICS` in `tools/bear_theme.py`); a theme file
    /// has no such table, so the parser derives them and only those differ.
    const NAMED_SEMANTICS: &[&str] = &[
        "atom",
        "ayu",
        "ayu-mirage",
        "catppuccin-latte",
        "catppuccin-macchiato",
        "cobalt",
        "dracula",
        "everforest-dark",
        "everforest-light",
        "gruvbox",
        "nord",
        "rose-pine",
        "rose-pine-dawn",
        "shibuya-jazz",
        "shibuya-lo-fi",
        "solarized-dark",
        "solarized-light",
        "tokyo-night",
        "tokyo-night-light",
    ];

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    const PLAIN: &str = r##"{"base": {"text color": "#111111", "background color": "#FFFFFF",
                            "accent color": "#DD4C4F"}}"##;

    fn find<'a>(themes: &'a [Theme], name: &str) -> &'a Theme {
        themes
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }

    fn names(loaded: &Loaded) -> Vec<&str> {
        loaded.themes.iter().map(|t| t.name).collect()
    }

    /// File name -> why it was skipped.
    fn skipped(loaded: &Loaded) -> Vec<(String, Problem)> {
        loaded
            .skipped
            .iter()
            .map(|s| (file_name(&s.path), s.problem.clone()))
            .collect()
    }

    fn rgb(hex: u32) -> Color {
        crate::ui::theme::rgb(hex)
    }

    /// Loading must leave a read-only theme directory exactly as it was,
    /// symlinks included.
    #[cfg(unix)]
    #[test]
    fn loading_reads_without_touching_the_directory() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(dir.path(), "Plain.theme", PLAIN);
        write(outside.path(), "Elsewhere.theme", PLAIN);
        symlink(
            outside.path().join("Elsewhere.theme"),
            dir.path().join("Linked.theme"),
        )
        .unwrap();

        let snapshot = |dir: &Path| {
            let mut files: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| {
                    let meta = std::fs::symlink_metadata(e.path()).unwrap();
                    let bytes = std::fs::read(e.path()).unwrap_or_default();
                    (e.file_name(), bytes, meta.modified().unwrap(), meta.len())
                })
                .collect();
            files.sort();
            files
        };
        let set_mode = |path: &Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        set_mode(&dir.path().join("Plain.theme"), 0o444);
        set_mode(dir.path(), 0o555);
        let before = (snapshot(dir.path()), snapshot(outside.path()));

        let loaded = load_dir(dir.path(), &[]);

        let after = (snapshot(dir.path()), snapshot(outside.path()));
        set_mode(dir.path(), 0o755);
        assert_eq!(before, after);
        assert_eq!(names(&loaded), ["linked", "plain"]);
    }

    /// Dotfile managers (stow, chezmoi) link theme files into place, so a
    /// link is followed; what it points at still has to be a regular file
    /// under the size cap, and a FIFO is refused before it could block.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_followed_to_a_regular_file_only() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = |name: &str| outside.path().join(name);
        write(outside.path(), "Real.theme", PLAIN);
        write(
            outside.path(),
            "Huge.theme",
            &" ".repeat(MAX_BYTES as usize + 1),
        );
        std::fs::create_dir(target("Folder.theme")).unwrap();
        let fifo = std::process::Command::new("mkfifo")
            .arg(target("Pipe.theme"))
            .status()
            .unwrap();
        assert!(fifo.success());
        for (link, to) in [
            ("Stowed.theme", "Real.theme"),
            ("Huge.theme", "Huge.theme"),
            ("Folder.theme", "Folder.theme"),
            ("Pipe.theme", "Pipe.theme"),
            ("Dangling.theme", "Gone.theme"),
        ] {
            symlink(target(to), dir.path().join(link)).unwrap();
        }

        let loaded = load_dir(dir.path(), &[]);
        assert_eq!(names(&loaded), ["stowed"]);
        let problems = skipped(&loaded);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(matches!(&problems[0], (f, Problem::Unreadable(_)) if f == "Dangling.theme"));
        assert_eq!(problems[1], ("Folder.theme".into(), Problem::NotAFile));
        assert_eq!(problems[2], ("Huge.theme".into(), Problem::TooLarge));
        assert_eq!(problems[3], ("Pipe.theme".into(), Problem::NotAFile));
    }

    #[test]
    fn names_are_slugs_of_the_file_name() {
        assert_eq!(slug("Rosé Pine Dawn"), "rose-pine-dawn");
        // A macOS file name: `e` followed by a combining acute accent.
        assert_eq!(slug("Rose\u{301} Pine"), "rose-pine");
        assert_eq!(slug("Rose\u{301} Pine"), slug("Rosé Pine"));
        assert_eq!(slug("  Red   Graphite "), "red-graphite");
        assert_eq!(slug("D.Boring"), "d-boring");
        assert_eq!(slug("Shibuya Lo-fi"), "shibuya-lo-fi");
    }

    /// NFKD folds past Latin-1, as `tools/bear_theme.py` does; the letters
    /// NFKD leaves whole fold by hand on both sides.
    #[test]
    fn names_fold_accents_beyond_latin_1() {
        assert_eq!(slug("Šibenik"), "sibenik");
        assert_eq!(slug("Erdős"), "erdos");
        assert_eq!(slug("Łódź Nights"), "lodz-nights");
        assert_eq!(slug("Bjørn"), "bjorn");
        assert_eq!(slug("Straße"), "strasse");
        assert_eq!(slug("Ærø"), "aero");
        assert_eq!(slug("Việt Nam"), "viet-nam");
        assert_eq!(slug("ﬁre"), "fire");
        assert_eq!(slug("Ｎｏｒｄ"), "nord");
        assert_eq!(slug("x²"), "x2");
        // A no-break space and an ellipsis part words; a script NFKD cannot
        // fold to ASCII is dropped, as the tool drops it.
        assert_eq!(slug("Rosé\u{a0}Pine…Dawn"), "rose-pine-dawn");
        assert_eq!(slug("Ночь"), "");
    }

    /// Every file the built-ins were generated from slugs to the name its
    /// palette carries.
    #[test]
    fn every_bear_file_name_slugs_to_its_built_in_name() {
        let mut want: Vec<&str> = THEMES
            .iter()
            .filter(|t| !HAND_TUNED.contains(&t.name))
            .chain(BEAR_BASES)
            .map(|t| t.name)
            .collect();
        want.sort();
        let manifest = manifest();
        let mut got: Vec<&str> = manifest.keys().map(String::as_str).collect();
        got.sort();
        assert_eq!(got.len(), 39);
        assert_eq!(got, want);
    }

    /// `slug` agrees with `tools/bear_theme.py` over every character its
    /// table covers, plus the hand-folded letters. Where `python3` is not
    /// installed this checks nothing.
    #[test]
    fn slug_agrees_with_the_python_tool() {
        let mut samples: Vec<String> = NFKD_ASCII
            .iter()
            .chain(LETTERS)
            .map(|(c, _)| format!("a{c}b"))
            .collect();
        for c in NFKD_ASCII.iter().chain(LETTERS).map(|(c, _)| *c) {
            samples.extend(c.to_uppercase().map(|u| format!("a{u}b")));
        }
        samples.extend(
            ["Rosé Pine Dawn", "Ｎｏｒｄ　Ｌｉｇｈｔ", "Rose\u{301} Pine"].map(String::from),
        );
        let tool = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools");
        let script = "import json, sys\n\
                      sys.path.insert(0, sys.argv[1])\n\
                      from bear_theme import slug\n\
                      print(json.dumps([slug(s) for s in json.load(sys.stdin)]))";
        let child = std::process::Command::new("python3")
            // `-B`: importing the tool must not leave `__pycache__` in the repo.
            .args(["-B", "-c", script])
            .arg(&tool)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            eprintln!("note: no python3; slug not compared with the tool.");
            return;
        };
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&samples).unwrap().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "the tool's slug did not run");
        let theirs: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
        for (sample, theirs) in samples.iter().zip(&theirs) {
            assert_eq!(&slug(sample), theirs, "{sample:?}");
        }
        assert_eq!(samples.len(), theirs.len());
    }

    /// A theme file's keys and values reach the terminal through a
    /// `Problem`, so control characters and bidi overrides come out
    /// escaped, where the reader can see them.
    #[test]
    fn problems_escape_what_the_file_holds() {
        let osc52 = "$\u{1b}]52;c;aGVsbG8=\u{7}";
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Sneaky.theme",
            &format!(
                r##"{{"base": {{"text color": "#111111", "background color": "#FFFFFF",
                    "accent color": {}}}}}"##,
                serde_json::to_string(osc52).unwrap()
            ),
        );
        let loaded = load_dir(dir.path(), &[]);
        let line = loaded.skipped[0].problem.to_string();
        assert!(
            !line.contains('\u{1b}') && !line.contains('\u{7}'),
            "{line}"
        );
        assert!(line.contains(r"\u{1b}]52;c;aGVsbG8=\u{7}"), "{line}");

        let problems = [
            Problem::BadColor("base.accent color".into(), "\"\u{9b}31m\"".into()),
            Problem::MissingColor("a\u{202e}b".into()),
            Problem::BaseNotFound("x\u{2066}y\u{7f}".into()),
            Problem::Duplicate("d".into(), "D\u{1b}[2J.theme".into()),
        ];
        for problem in problems {
            let line = problem.to_string();
            assert!(
                !line.chars().any(|c| c.is_control()
                    || ('\u{202a}'..='\u{202e}').contains(&c)
                    || ('\u{2066}'..='\u{2069}').contains(&c)),
                "{line:?}"
            );
        }
        assert_eq!(escaped("a\u{202e}b\\c\té"), r"a\u{202e}b\c\u{9}é");
    }

    #[test]
    fn references_resolve_after_the_base_theme_is_merged() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Parent.theme",
            r##"{"base": {"text color": "#111111", "background color": "#FFFFFF",
                 "accent color": "#0000FF"},
                 "editor": {"link color": "$base.accent color"}}"##,
        );
        write(
            dir.path(),
            "Child Theme.theme",
            r##"{"meta": {"base theme": "Parent"},
                 "base": {"accent color": "#FF0000"}}"##,
        );
        write(dir.path(), "notes.txt", "{}");

        let loaded = load_dir(dir.path(), &[]);
        assert_eq!(names(&loaded), ["child-theme", "parent"]);
        assert!(loaded.skipped.is_empty());
        let child = find(&loaded.themes, "child-theme");
        assert_eq!(child.link, Color::Rgb(0xFF, 0, 0));
        assert_eq!(child.foreground, Color::Rgb(0x11, 0x11, 0x11));
        assert!(!child.dark);
        // Keys the file leaves out fall back to the nearest color it has.
        assert_eq!(child.bullet, child.accent);
        assert_eq!(child.code_bg, child.background);
        assert_eq!(find(&loaded.themes, "parent").link, Color::Rgb(0, 0, 0xFF));
    }

    /// Every way a file can be broken is skipped with its own reason, which
    /// `--list-themes` and `--theme` print.
    #[test]
    fn a_broken_file_is_skipped_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let theme = |base: &str| {
            format!(
                r##"{{"base": {{"text color": "#111111", "background color": "#FFFFFF",
                    "accent color": "#DD4C4F", {base}}}}}"##
            )
        };
        write(dir.path(), "Bad Json.theme", "{ not json");
        write(dir.path(), "Array.theme", "[]");
        write(
            dir.path(),
            "No Accent.theme",
            r##"{"base": {"text color": "#111111", "background color": "#FFFFFF"}}"##,
        );
        write(
            dir.path(),
            "Loop.theme",
            r##"{"base": {"text color": "$base.text color",
                 "background color": "#000000", "accent color": "#FF0000"}}"##,
        );
        write(
            dir.path(),
            "Optional Loop.theme",
            &theme(r#""stroke color": "$base.stroke color""#),
        );
        write(
            dir.path(),
            "Dangling.theme",
            r##"{"base": {"text color": "$base.ink", "background color": "#FFFFFF",
                 "accent color": "#DD4C4F"}}"##,
        );
        write(
            dir.path(),
            "Not A Color.theme",
            &theme(r#""stroke color": "blue""#),
        );
        write(
            dir.path(),
            "Orphan.theme",
            r#"{"meta": {"base theme": "Mauve"}, "base": {}}"#,
        );
        write(
            dir.path(),
            "Heir.theme",
            r#"{"meta": {"base theme": "Bad Json"}, "base": {}}"#,
        );
        write(
            dir.path(),
            "Cycle.theme",
            r#"{"meta": {"base theme": "Cycle 2"}, "base": {}}"#,
        );
        write(
            dir.path(),
            "Cycle 2.theme",
            r#"{"meta": {"base theme": "Cycle"}, "base": {}}"#,
        );
        write(dir.path(), "___.theme", PLAIN);
        // An optional key that is simply absent, or refers to something
        // absent, is no problem: it falls back.
        write(
            dir.path(),
            "Fine.theme",
            &theme(r#""stroke color": "$base.nothing""#),
        );

        let loaded = load_dir(dir.path(), &[]);
        assert_eq!(names(&loaded), ["fine"]);
        let problem = |file: &str| {
            skipped(&loaded)
                .into_iter()
                .find(|(f, _)| f == file)
                .unwrap_or_else(|| panic!("{file} was not skipped"))
                .1
        };
        assert!(matches!(problem("Bad Json.theme"), Problem::BadJson(_)));
        assert_eq!(problem("Array.theme"), Problem::NotAnObject);
        assert_eq!(
            problem("No Accent.theme"),
            Problem::MissingColor("base.accent color".into())
        );
        assert_eq!(
            problem("Loop.theme"),
            Problem::ReferenceLoop("base.text color".into())
        );
        assert_eq!(
            problem("Optional Loop.theme"),
            Problem::ReferenceLoop("base.stroke color".into())
        );
        assert_eq!(
            problem("Dangling.theme"),
            Problem::DanglingReference {
                key: "base.text color".into(),
                target: "base.ink".into()
            }
        );
        assert_eq!(
            problem("Not A Color.theme"),
            Problem::BadColor("base.stroke color".into(), "\"blue\"".into())
        );
        assert_eq!(
            problem("Orphan.theme"),
            Problem::BaseNotFound("Mauve".into())
        );
        assert!(matches!(
            problem("Heir.theme"),
            Problem::BaseBroken(base, inner) if base == "Bad Json"
                && matches!(*inner, Problem::BadJson(_))
        ));
        assert_eq!(problem("Cycle.theme"), Problem::BaseLoop("Cycle".into()));
        assert_eq!(
            problem("Cycle 2.theme"),
            Problem::BaseLoop("Cycle 2".into())
        );
        assert_eq!(problem("___.theme"), Problem::NoName);
        assert_eq!(loaded.skipped.len(), 12);

        // Each reason reads as one line after the file's path.
        for skipped in &loaded.skipped {
            let line = skipped.problem.to_string();
            assert!(!line.is_empty() && !line.contains('\n'), "{line:?}");
        }
        assert_eq!(
            problem("Heir.theme").to_string().split(':').next(),
            Some(r#"base theme "Bad Json" did not load"#)
        );
        assert!(load_dir(&dir.path().join("absent"), &[]).themes.is_empty());
    }

    /// Bear's own files are not shipped, so a theme copied out of Bear.app
    /// finds its base among the built-ins: Academia over Dark Graphite.
    #[test]
    fn a_base_theme_not_in_the_directory_comes_from_the_built_ins() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Nord Ember.theme",
            r##"{"meta": {"base theme": "Nord"}, "base": {"accent color": "#D08770"}}"##,
        );
        let nord = crate::ui::palettes::NORD;
        let loaded = load_dir(dir.path(), THEMES);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        let ember = find(&loaded.themes, "nord-ember");
        assert_eq!(ember.accent, rgb(0xD08770));
        assert_eq!(ember.background, nord.background);
        assert_eq!(ember.sidebar_bg, nord.sidebar_bg);
        assert_eq!(ember.tag_bg, nord.tag_bg);
        // Nord's list markers are its accent, so they follow the new one;
        // its links are a color of their own, so they stay.
        assert_eq!(ember.bullet, rgb(0xD08770));
        assert_eq!(ember.link, nord.link);

        // Base names are matched like theme names.
        write(
            dir.path(),
            "Spelled.theme",
            r#"{"meta": {"base theme": "rosé  PINE"}}"#,
        );
        let loaded = load_dir(dir.path(), THEMES);
        let mut spelled = *find(&loaded.themes, "spelled");
        spelled.name = "rose-pine";
        assert_eq!(
            spelled.background,
            crate::ui::palettes::ROSE_PINE.background
        );
    }

    /// A base in the directory is the real thing and is used over the
    /// built-in of that name, while the file itself is not offered: a
    /// built-in name always wins.
    #[test]
    fn a_base_file_in_the_directory_comes_before_a_built_in() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Nord.theme", PLAIN);
        write(
            dir.path(),
            "Mine.theme",
            r#"{"meta": {"base theme": "Nord"}}"#,
        );
        let loaded = load_dir(dir.path(), THEMES);
        assert_eq!(names(&loaded), ["mine"]);
        assert_eq!(find(&loaded.themes, "mine").background, rgb(0xFFFFFF));
        assert_eq!(
            skipped(&loaded),
            [("Nord.theme".into(), Problem::BuiltIn("nord".into()))]
        );
    }

    /// Two files with one name: the first in file name order that loads is
    /// offered, whatever order the directory lists them in.
    #[test]
    fn files_with_one_name_resolve_in_file_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let with_bg = |bg: &str| {
            format!(
                r##"{{"base": {{"text color": "#111111", "background color": "{bg}",
                    "accent color": "#DD4C4F"}}}}"##
            )
        };
        // Distinct on a case-insensitive file system too.
        write(dir.path(), "My_Theme.theme", &with_bg("#000002"));
        write(dir.path(), "My-Theme.theme", &with_bg("#000001"));
        write(dir.path(), "My Theme.theme", "{ broken");
        let loaded = load_dir(dir.path(), &[]);
        assert_eq!(names(&loaded), ["my-theme"]);
        // `My Theme` sorts first but is broken, so `My-Theme` is offered.
        assert_eq!(loaded.themes[0].background, rgb(0x000001));
        let problems = skipped(&loaded);
        assert!(matches!(&problems[0], (f, Problem::BadJson(_)) if f == "My Theme.theme"));
        assert_eq!(
            problems[1],
            (
                "My_Theme.theme".into(),
                Problem::Duplicate("my-theme".into(), "My-Theme.theme".into())
            )
        );
    }

    /// A built-in written back out as a theme file draws as itself, apart
    /// from the toast colors, which pass through `recolor` again and land
    /// within a few steps of where they were.
    #[test]
    fn a_built_in_written_out_as_a_base_draws_as_itself() {
        let generated = THEMES
            .iter()
            .filter(|t| !HAND_TUNED.contains(&t.name))
            .chain(BEAR_BASES);
        for built_in in generated {
            let mut rebuilt = to_theme(built_in.name, &built_in_document(built_in)).unwrap();
            if !NAMED_SEMANTICS.contains(&built_in.name) {
                assert_toasts_close(&rebuilt, built_in);
            }
            rebuilt.success = built_in.success;
            rebuilt.warning = built_in.warning;
            rebuilt.error = built_in.error;
            assert_eq!(rebuilt, *built_in, "{}", built_in.name);
        }
    }

    fn assert_toasts_close(a: &Theme, b: &Theme) {
        let pairs = [
            (a.success, b.success),
            (a.warning, b.warning),
            (a.error, b.error),
        ];
        for (x, y) in pairs {
            let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (x, y) else {
                panic!("not RGB");
            };
            let close = r1.abs_diff(r2) <= 4 && g1.abs_diff(g2) <= 4 && b1.abs_diff(b2) <= 4;
            assert!(close, "{}: {x:?} vs {y:?}", b.name);
        }
    }

    /// The hand-written themes in `config/themes/` load on CI, where Bear is
    /// not installed: one resolving `$section.key` references, one over a
    /// base in the same directory, one over a built-in base.
    #[test]
    fn hand_written_theme_files_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/themes");
        let loaded = load_dir(&dir, THEMES);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        assert_eq!(names(&loaded), ["nord-ember", "paper", "paper-night"]);

        let paper = find(&loaded.themes, "paper");
        assert!(!paper.dark);
        assert_eq!(paper.link, rgb(0x2F6FA3));
        assert_eq!(paper.bullet, rgb(0x2F6FA3));
        assert_eq!(paper.code_bg, rgb(0xF1ECE0));
        assert_eq!(paper.cursor_blur_bg, rgb(0xF1ECE0));
        assert_eq!(paper.tag_bg, rgb(0xE6DFCF));
        assert_eq!(paper.sidebar_border, rgb(0x2E2A24));

        // Paper's references resolve against Paper Night's own values.
        let night = find(&loaded.themes, "paper-night");
        assert!(night.dark);
        assert_eq!(night.link, rgb(0xE0A458));
        assert_eq!(night.code_fg, rgb(0xE4DDCF));
        assert_eq!(night.tag_bg, rgb(0x3A362F));
        assert_eq!(night.sidebar_bg, paper.sidebar_bg);

        let ember = find(&loaded.themes, "nord-ember");
        assert_eq!(ember.background, crate::ui::palettes::NORD.background);
        assert_eq!(ember.accent, rgb(0xD08770));
    }

    /// Bear's files, as `config/themes/bear-themes.sha256` records them:
    /// slug -> (file name, SHA-256).
    fn manifest() -> HashMap<String, (String, String)> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/themes/bear-themes.sha256");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| {
                let (hash, file) = line.split_once("  ").expect("`<sha256>  <file>`");
                let stem = file.strip_suffix(".theme").expect(".theme");
                (slug(stem), (file.to_string(), hash.to_string()))
            })
            .collect()
    }

    fn sha256(path: &Path) -> Option<String> {
        let out = std::process::Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        out.status
            .success()
            .then(|| text.split_whitespace().next().map(str::to_string))?
    }

    fn have_shasum() -> bool {
        std::process::Command::new("shasum")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// Bear's theme files that match the manifest, and whether every file the
    /// manifest lists did (nothing added, changed or gone).
    struct Verified {
        files: Vec<(String, PathBuf)>,
        complete: bool,
    }

    /// Bear's theme files in the installed Bear.app whose SHA-256 is the one
    /// the manifest records, by slug. Anything else is noted, never failed:
    /// a changed hash means Bear updated its themes, which is a reason to
    /// rerun `tools/bear_theme.py`, not a broken build. `None` without Bear.
    fn verified_bear_files() -> Option<Verified> {
        let dir = Path::new(BEAR);
        if !dir.is_dir() {
            return None;
        }
        if !have_shasum() {
            eprintln!(
                "note: no `shasum` on PATH, so Bear's theme files cannot be verified; none compared."
            );
            return Some(Verified {
                files: Vec::new(),
                complete: false,
            });
        }
        Some(verify(dir, manifest(), sha256))
    }

    /// The `.theme` files in `dir` whose hash (by `hash`) is the one
    /// `manifest` records for their slug; the rest are noted on stderr.
    fn verify(
        dir: &Path,
        mut manifest: HashMap<String, (String, String)>,
        hash: impl Fn(&Path) -> Option<String>,
    ) -> Verified {
        let mut verified = Verified {
            files: Vec::new(),
            complete: true,
        };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "theme"))
            .collect();
        paths.sort();
        for path in paths {
            let name = slug(&path.file_stem().unwrap().to_string_lossy());
            match (manifest.remove(&name), hash(&path)) {
                (Some((_, want)), Some(got)) if want == got => verified.files.push((name, path)),
                (Some((file, _)), _) => {
                    verified.complete = false;
                    eprintln!(
                        "note: Bear's {file} is not the file palettes.rs was generated from \
                         (its SHA-256 differs); not compared. Rerun tools/bear_theme.py."
                    )
                }
                (None, _) => {
                    verified.complete = false;
                    eprintln!(
                        "note: Bear ships {}, which bear-themes.sha256 does not list; \
                         not compared. Rerun tools/bear_theme.py.",
                        path.display()
                    )
                }
            }
        }
        for (file, _) in manifest.values() {
            verified.complete = false;
            eprintln!("note: Bear no longer ships {file}.");
        }
        verified
    }

    /// Bear's own theme files, read from Bear.app where it is installed,
    /// parse, and each draws exactly as the palette generated from it. Bear's
    /// files are not in the repo, so without Bear.app (CI) this checks
    /// nothing.
    #[test]
    fn bear_theme_files_match_the_built_in_palettes() {
        let Some(verified) = verified_bear_files() else {
            return;
        };
        let loaded = load_dir(Path::new(BEAR), &[]);
        for (name, path) in &verified.files {
            assert!(
                !loaded.skipped.iter().any(|s| &s.path == path),
                "{name}: {:?}",
                loaded.skipped
            );
            let built_in = THEMES
                .iter()
                .filter(|t| !HAND_TUNED.contains(&t.name))
                .chain(BEAR_BASES)
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("no palette for {name}"));
            let mut theme = *find(&loaded.themes, name);
            if NAMED_SEMANTICS.contains(&name.as_str()) {
                theme.success = built_in.success;
                theme.warning = built_in.warning;
                theme.error = built_in.error;
            }
            assert_eq!(theme, *built_in, "{name}");
        }
    }

    /// What the review found: a Bear theme with a `base theme`, copied alone
    /// into the themes directory under a new name, loads over its built-in
    /// base and draws as that theme's own palette.
    ///
    /// Two things cannot come back exactly. Toast colors go through `recolor`
    /// a second time and land within a few steps. And a palette cannot tell a
    /// reference from a plain color that happens to match it, so a key is
    /// taken to be the reference Bear's bases normally use: Dieci's tag
    /// background is a plain color where the others refer to the tertiary
    /// background, so the two Shibuya themes, which set their own, pick that
    /// up instead.
    #[test]
    fn a_copied_bear_theme_loads_over_its_built_in_base() {
        let Some(verified) = verified_bear_files() else {
            return;
        };
        compare_copied_children(&verified);
    }

    /// The check behind `a_copied_bear_theme_loads_over_its_built_in_base`,
    /// over whichever of Bear's files were verified; how many were compared.
    /// None compared is a note when Bear's files have changed (the children
    /// may be among those set aside) and a failure only when every file
    /// matched the manifest, since Bear's themes as generated do have
    /// children and a check that finds none has stopped looking.
    fn compare_copied_children(verified: &Verified) -> usize {
        let mut children = 0;
        for (name, path) in &verified.files {
            let text = std::fs::read_to_string(path).unwrap();
            let doc: Value = serde_json::from_str(&text).unwrap();
            if doc.pointer("/meta/base theme").is_none() {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), &format!("My {name}.theme"), &text);
            let loaded = load_dir(dir.path(), THEMES);
            assert!(loaded.skipped.is_empty(), "{name}: {:?}", loaded.skipped);

            let built_in = find(THEMES, name);
            let mut theme = *find(&loaded.themes, &format!("my-{name}"));
            theme.name = built_in.name;
            if !NAMED_SEMANTICS.contains(&name.as_str()) {
                assert_toasts_close(&theme, built_in);
            }
            theme.success = built_in.success;
            theme.warning = built_in.warning;
            theme.error = built_in.error;
            if name.starts_with("shibuya-") {
                assert_ne!(theme.tag_bg, built_in.tag_bg, "{name}: now exact?");
                theme.tag_bg = built_in.tag_bg;
            }
            assert_eq!(theme, *built_in, "{name}");
            children += 1;
        }
        if children == 0 {
            assert!(
                !verified.complete,
                "no Bear theme with a base theme was compared"
            );
            eprintln!(
                "note: none of Bear's verified theme files has a base theme; \
                 copied themes not compared."
            );
        }
        children
    }

    /// After a Bear update no file may match the manifest; the copied-theme
    /// check then notes it and passes rather than failing the build.
    #[test]
    fn nothing_verified_after_a_bear_update_is_a_note_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Nord.theme", PLAIN);
        write(dir.path(), "Brand New.theme", PLAIN);
        let verified = verify(dir.path(), manifest(), |_| Some("changed".into()));
        assert!(verified.files.is_empty());
        assert!(!verified.complete);
        assert_eq!(compare_copied_children(&verified), 0);

        // A missing `shasum` hashes nothing, with the same outcome.
        let verified = verify(dir.path(), manifest(), |_| None);
        assert!(verified.files.is_empty() && !verified.complete);
        assert_eq!(compare_copied_children(&verified), 0);
    }

    /// With every file as the manifest has it, finding no child is the
    /// check broken, not Bear changed.
    #[test]
    #[should_panic(expected = "no Bear theme with a base theme was compared")]
    fn nothing_compared_with_every_file_verified_still_fails() {
        compare_copied_children(&Verified {
            files: Vec::new(),
            complete: true,
        });
    }
}
