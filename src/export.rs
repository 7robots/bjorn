//! Exporting a note to disk in one of several formats.
//!
//! `FORMATS` is what the picker offers; `export_note` dispatches to a writer
//! per format. Writers are pure: they take the note's text, title and any
//! attachment bytes and write files. Fetching content and attachments is the
//! app's job.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::render::{percent_decode, to_text};
use crate::render_html;
use crate::util::expand_tilde;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ExportError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub id: &'static str,
    pub label: &'static str,
    pub key: char,
    pub ext: &'static str,
    /// The writer wants the attachment bytes (images embedded or copied).
    pub needs_attachments: bool,
}

pub const FORMATS: [Format; 5] = [
    Format {
        id: "md",
        label: "Markdown",
        key: 'm',
        ext: "md",
        needs_attachments: false,
    },
    Format {
        id: "html",
        label: "HTML",
        key: 'h',
        ext: "html",
        needs_attachments: true,
    },
    Format {
        id: "txt",
        label: "Text",
        key: 't',
        ext: "txt",
        needs_attachments: false,
    },
    Format {
        id: "rtf",
        label: "RTF",
        key: 'r',
        ext: "rtf",
        needs_attachments: true,
    },
    Format {
        id: "textbundle",
        label: "TextBundle",
        key: 'b',
        ext: "textbundle",
        needs_attachments: true,
    },
];
/// What a TextBundle's info.json says about us.
pub const CREATOR_IDENTIFIER: &str = "org.7robots.bjorn";
pub const DEFAULT_FORMAT: &str = "md";

static UNSAFE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*\x00-\x1f]+"#).unwrap());
static LINK_TARGET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?\[[^\]]*\]\()([^)\s]+)(\))").unwrap());

pub fn format_by_id(format_id: &str) -> Format {
    FORMATS
        .iter()
        .copied()
        .find(|f| f.id == format_id)
        .unwrap_or_else(|| format_by_id(DEFAULT_FORMAT))
}

/// RTF with images is an `.rtfd` package so the pictures travel; textutil
/// drops them from a flat `.rtf`.
pub fn extension_for(fmt: Format, has_attachments: bool) -> &'static str {
    if fmt.id == "rtf" && has_attachments {
        "rtfd"
    } else {
        fmt.ext
    }
}

/// A filename from a note title: path separators and control characters
/// become spaces, whitespace collapses, leading dots go.
pub fn safe_filename(title: &str) -> String {
    let name = UNSAFE_RE.replace_all(title, " ");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.trim_matches([' ', '.']).chars().take(120).collect();
    if name.is_empty() {
        "note".to_string()
    } else {
        name
    }
}

/// `path`, or `stem (2).ext`, `stem (3).ext`... if it already exists.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut n = 2;
    loop {
        let candidate = path.with_file_name(format!("{stem} ({n}){suffix}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

pub fn default_export_path(export_dir: &Path, title: &str, ext: &str) -> PathBuf {
    let dir = expand_tilde(&export_dir.to_string_lossy());
    unique_path(&dir.join(format!("{}.{ext}", safe_filename(title))))
}

fn prepare(destination: &Path) -> Result<PathBuf, ExportError> {
    let destination = expand_tilde(&destination.to_string_lossy());
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ExportError(format!("{}: {e}", parent.display())))?;
    }
    Ok(destination)
}

fn write(path: &Path, text: &str) -> Result<(), ExportError> {
    std::fs::write(path, text).map_err(|e| ExportError(format!("{}: {e}", path.display())))
}

/// Write the raw note text. The parent directory is created if missing.
pub fn export_markdown(content: &str, destination: &Path) -> Result<PathBuf, ExportError> {
    let destination = prepare(destination)?;
    let text = if content.ends_with('\n') {
        content.to_string()
    } else {
        format!("{content}\n")
    };
    write(&destination, &text)?;
    Ok(destination)
}

pub fn export_text(content: &str, destination: &Path) -> Result<PathBuf, ExportError> {
    let destination = prepare(destination)?;
    write(&destination, &to_text(content))?;
    Ok(destination)
}

/// One self-contained HTML file, images embedded.
pub fn export_html(
    content: &str,
    title: &str,
    destination: &Path,
    images: &HashMap<String, Vec<u8>>,
) -> Result<PathBuf, ExportError> {
    let destination = prepare(destination)?;
    write(
        &destination,
        &render_html::render(content, title, images, &HashMap::new()),
    )?;
    Ok(destination)
}

/// RTF through macOS `textutil`, from the HTML rendering. A destination
/// ending in `.rtfd` becomes a package with the images inside (textutil names
/// them itself); a flat `.rtf` carries text and tables only.
pub fn export_rtf(
    content: &str,
    title: &str,
    destination: &Path,
    images: &HashMap<String, Vec<u8>>,
) -> Result<PathBuf, ExportError> {
    if crate::util::which("textutil").is_none() {
        return Err(ExportError(
            "RTF export needs textutil, which ships with macOS.".into(),
        ));
    }
    let destination = prepare(destination)?;
    let kind = if destination
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("rtfd"))
    {
        "rtfd"
    } else {
        "rtf"
    };
    let tmp = tempfile::Builder::new()
        .prefix("bjorn-rtf-")
        .tempdir()
        .map_err(|e| ExportError(e.to_string()))?;
    let source = tmp.path().join("note.html");
    let empty = HashMap::new();
    let html = render_html::render(
        content,
        title,
        if kind == "rtfd" { images } else { &empty },
        &HashMap::new(),
    );
    write(&source, &html)?;
    let result = std::process::Command::new("textutil")
        .args(["-convert", kind])
        .arg(&source)
        .arg("-output")
        .arg(&destination)
        .output()
        .map_err(|e| ExportError(format!("textutil: {e}")))?;
    // textutil exits 0 even when it fails; the output is the only reliable signal.
    if !result.status.success() || !destination.exists() {
        let detail = String::from_utf8_lossy(if result.stderr.is_empty() {
            &result.stdout
        } else {
            &result.stderr
        });
        let first = detail
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("textutil wrote nothing");
        return Err(ExportError(first.to_string()));
    }
    Ok(destination)
}

/// Percent-encode a filename the way Bear writes attachment links.
pub fn percent_encode(name: &str) -> String {
    const SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'_')
        .remove(b'.')
        .remove(b'-')
        .remove(b'/')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(name, SAFE).to_string()
}

/// Point every markdown link or image whose target is one of `filenames`
/// (compared after percent-decoding, as Bear writes them) at `prefix` + the
/// same encoded name. Other links are left alone.
pub fn rewrite_attachment_links(content: &str, filenames: &[String], prefix: &str) -> String {
    LINK_TARGET_RE
        .replace_all(content, |caps: &regex::Captures| {
            let target = &caps[2];
            let decoded = percent_decode(target);
            if filenames.contains(&decoded) {
                format!(
                    "{}{prefix}{}{}",
                    &caps[1],
                    percent_encode(&decoded),
                    &caps[3]
                )
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

/// A `.textbundle` folder (TextBundle 2.0): `info.json`, the raw Bear markdown
/// as `text.md` with attachment links pointing into `assets/`, and every
/// attachment copied there.
pub fn export_textbundle(
    content: &str,
    destination: &Path,
    images: &HashMap<String, Vec<u8>>,
) -> Result<PathBuf, ExportError> {
    let destination = prepare(destination)?;
    std::fs::create_dir_all(&destination)
        .map_err(|e| ExportError(format!("{}: {e}", destination.display())))?;
    let info = serde_json::json!({"version": 2, "type": "net.daringfireball.markdown", "transient": false, "creatorIdentifier": CREATOR_IDENTIFIER});
    write(
        &destination.join("info.json"),
        &format!("{}\n", serde_json::to_string_pretty(&info).unwrap()),
    )?;
    let names: Vec<String> = images.keys().cloned().collect();
    let text = rewrite_attachment_links(content, &names, "assets/");
    let text = if text.ends_with('\n') {
        text
    } else {
        format!("{text}\n")
    };
    write(&destination.join("text.md"), &text)?;
    if !images.is_empty() {
        let assets = destination.join("assets");
        std::fs::create_dir_all(&assets).map_err(|e| ExportError(e.to_string()))?;
        for (name, data) in images {
            std::fs::write(assets.join(name), data)
                .map_err(|e| ExportError(format!("{name}: {e}")))?;
        }
    }
    Ok(destination)
}

/// Write `content` as `fmt` to `destination`; returns what was written.
pub fn export_note(
    fmt: Format,
    content: &str,
    title: &str,
    destination: &Path,
    images: &HashMap<String, Vec<u8>>,
) -> Result<PathBuf, ExportError> {
    match fmt.id {
        "md" => export_markdown(content, destination),
        "txt" => export_text(content, destination),
        "html" => export_html(content, title, destination, images),
        "rtf" => export_rtf(content, title, destination, images),
        "textbundle" => export_textbundle(content, destination, images),
        other => Err(ExportError(format!("unknown export format {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_and_unique_path() {
        assert_eq!(
            safe_filename("Dave Conversation: 6/Nov/2025?"),
            "Dave Conversation 6 Nov 2025"
        );
        assert_eq!(safe_filename("..."), "note");
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Note.md");
        assert_eq!(unique_path(&target), target);
        std::fs::write(&target, "x").unwrap();
        assert_eq!(unique_path(&target), dir.path().join("Note (2).md"));
        std::fs::write(dir.path().join("Note (2).md"), "x").unwrap();
        assert_eq!(
            default_export_path(dir.path(), "Note", "md"),
            dir.path().join("Note (3).md")
        );
    }

    #[test]
    fn formats_and_extensions() {
        assert_eq!(format_by_id("docx").id, "md");
        assert_eq!(format_by_id("html").ext, "html");
        assert_eq!(extension_for(format_by_id("rtf"), false), "rtf");
        assert_eq!(extension_for(format_by_id("rtf"), true), "rtfd");
        assert_eq!(extension_for(format_by_id("html"), true), "html");
    }

    #[test]
    fn rewrite_attachment_links_touches_only_attachments() {
        let src =
            "![](Front%20bed.png) [plan](plan.pdf) [REV](https://rev.example) ![x](other.png)";
        let names = vec!["Front bed.png".to_string(), "plan.pdf".to_string()];
        assert_eq!(
            rewrite_attachment_links(src, &names, "assets/"),
            "![](assets/Front%20bed.png) [plan](assets/plan.pdf) [REV](https://rev.example) ![x](other.png)"
        );
    }

    #[test]
    fn textbundle_without_attachments_has_no_assets_folder() {
        let dir = tempfile::tempdir().unwrap();
        let out = export_note(
            format_by_id("textbundle"),
            "# T\n\nbody",
            "T",
            &dir.path().join("T.textbundle"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("text.md")).unwrap(),
            "# T\n\nbody\n"
        );
        assert!(!out.join("assets").exists());
        let info: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("info.json")).unwrap()).unwrap();
        assert_eq!(info["version"], 2);
    }

    #[test]
    fn markdown_and_text_writers() {
        let dir = tempfile::tempdir().unwrap();
        let md = export_note(
            format_by_id("md"),
            "# T\n\n- [ ] a",
            "T",
            &dir.path().join("sub/T.md"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(md).unwrap(), "# T\n\n- [ ] a\n");
        let txt = export_note(
            format_by_id("txt"),
            "# T\n\n- [ ] a",
            "T",
            &dir.path().join("T.txt"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(txt).unwrap(), "T\n\n- ☐ a\n");
    }

    #[test]
    fn rtf_via_textutil_when_present() {
        if crate::util::which("textutil").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = export_note(
            format_by_id("rtf"),
            "# T\n\nrelease notes",
            "T",
            &dir.path().join("T.rtf"),
            &HashMap::new(),
        )
        .unwrap();
        let rtf = std::fs::read_to_string(out).unwrap();
        assert!(rtf.starts_with("{\\rtf1") && rtf.contains("release notes"));
    }
}
