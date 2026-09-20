//! Exporting a note to disk in one of several formats.
//!
//! `FORMATS` is what the picker offers; `export_note` dispatches to a writer
//! per format. Writers are pure: they take the note's text, title and any
//! attachment bytes and write files. Fetching content and attachments is the
//! app's job.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

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

pub const FORMATS: [Format; 6] = [
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
        id: "pdf",
        label: "PDF",
        key: 'p',
        ext: "pdf",
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

/// Attributes a browser or WeasyPrint fetches on its own: an image, a frame,
/// a stylesheet. `href` is here for `<link>` only — an anchor's href is
/// followed by a reader, not by the converter.
static FETCHED_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\s(src|srcset|poster|background)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#).unwrap()
});
static LINK_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").unwrap());
static HREF_OR_DATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)\s(href|data)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#).unwrap());
static OBJECT_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<object\b[^>]*>").unwrap());
static CSS_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)url\(\s*([^)]*?)\s*\)"#).unwrap());
static CSS_IMPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)@import\b[^;]*;").unwrap());

fn is_embedded(value: &str) -> bool {
    value
        .trim()
        .trim_matches(['"', '\''])
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("data:")
}

/// Everything the note's own text points at, taken out.
///
/// A converter fetches what the page it is given points at: WeasyPrint has no
/// flag to stop it, and Chrome's `--host-resolver-rules` cannot help with
/// `file:`, which is the scheme the page itself is loaded from. A Bear note is
/// not always one you wrote — imports, web clips, a note someone shared — and
/// its inline HTML reaches the converter as written, so a remote image is a
/// note telling somebody it was printed, and a local one bakes a file off the
/// disk into a PDF that is usually about to be sent on.
///
/// Nothing is lost that was going to print: an attachment is already a
/// `data:` URI by the time the HTML is written, and a `data:` URI is what
/// this keeps. Anything else is dropped: the image leaves an empty frame, the
/// stylesheet does not apply.
pub fn only_embedded(html: &str) -> String {
    let html = FETCHED_ATTR_RE.replace_all(html, |caps: &regex::Captures| {
        if is_embedded(&caps[2]) {
            caps[0].to_string()
        } else {
            String::new()
        }
    });
    let html = LINK_TAG_RE.replace_all(&html, |caps: &regex::Captures| {
        HREF_OR_DATA_RE
            .replace_all(&caps[0], |inner: &regex::Captures| {
                if is_embedded(&inner[2]) {
                    inner[0].to_string()
                } else {
                    String::new()
                }
            })
            .into_owned()
    });
    let html = OBJECT_TAG_RE.replace_all(&html, |caps: &regex::Captures| {
        HREF_OR_DATA_RE
            .replace_all(&caps[0], |inner: &regex::Captures| {
                if is_embedded(&inner[2]) {
                    inner[0].to_string()
                } else {
                    String::new()
                }
            })
            .into_owned()
    });
    let html = CSS_IMPORT_RE.replace_all(&html, |caps: &regex::Captures| {
        if caps[0].to_ascii_lowercase().contains("data:") {
            caps[0].to_string()
        } else {
            String::new()
        }
    });
    CSS_URL_RE
        .replace_all(&html, |caps: &regex::Captures| {
            if is_embedded(&caps[1]) {
                caps[0].to_string()
            } else {
                "url(about:blank)".to_string()
            }
        })
        .into_owned()
}

/// What turns the HTML rendering into a PDF. macOS ships nothing that can:
/// `textutil` stops at RTF, and `cupsfilter` refuses HTML outright ("No filter
/// to convert from text/html to application/pdf"), so this is the one format
/// that asks for a tool of your own, the way RTF asks for `textutil`.
#[derive(Debug, Clone)]
pub enum Converter {
    /// `weasyprint in.html out.pdf`: no browser and no scripts.
    WeasyPrint(PathBuf),
    /// A Chromium browser's own printer, run headless.
    Chrome(PathBuf),
}

/// Chromium browsers that are app bundles rather than something on `PATH`.
/// They all print the same way; a user who has Brave and no Chrome should not
/// be told to install one.
const CHROME_BUNDLES: [&str; 10] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "~/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "~/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "~/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "~/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
    "~/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
];

impl Converter {
    /// The first converter this machine has, WeasyPrint before a browser: it
    /// is the quieter of the two and runs no scripts at all.
    pub fn find() -> Option<Self> {
        if let Some(path) = crate::util::which("weasyprint") {
            return Some(Converter::WeasyPrint(path));
        }
        for name in ["chromium", "google-chrome", "google-chrome-stable"] {
            if let Some(path) = crate::util::which(name) {
                return Some(Converter::Chrome(path));
            }
        }
        CHROME_BUNDLES
            .iter()
            .map(|p| expand_tilde(p))
            .find(|p| p.is_file())
            .map(Converter::Chrome)
    }

    fn name(&self) -> &'static str {
        match self {
            Converter::WeasyPrint(_) => "weasyprint",
            Converter::Chrome(_) => "Chrome",
        }
    }

    fn command(&self, source: &Path, destination: &Path, profile: &Path) -> Command {
        match self {
            Converter::WeasyPrint(bin) => {
                let mut command = Command::new(bin);
                // `--` first: a destination the user typed can start with a
                // dash, and argparse would read it as an option.
                command.arg("--").arg(source).arg(destination);
                command
            }
            Converter::Chrome(bin) => {
                let mut command = Command::new(bin);
                command
                    // `--headless=new`, not plain `--headless`: the old one
                    // spins on this machine, looping
                    // `CVDisplayLinkCreateWithCGDisplay failed` on stderr
                    // forever instead of printing.
                    .arg("--headless=new")
                    // Its own profile: the render never sees your cookies, and
                    // the print does not fail because Chrome is already open.
                    .arg(format!("--user-data-dir={}", profile.display()))
                    // A note can carry inline HTML and a browser renders it, so
                    // this one is given nothing to run and nowhere to send
                    // anything: no hostname resolves, and a bare IP address —
                    // which never reaches the resolver — meets a dead proxy.
                    // (`--blink-settings=scriptEnabled=false` would be tidier,
                    // but it breaks --print-to-pdf outright: empty file, and
                    // the exit code still says 0.)
                    .arg("--disable-javascript")
                    .arg("--host-resolver-rules=MAP * ~NOTFOUND")
                    .arg("--proxy-server=127.0.0.1:1")
                    .arg("--proxy-bypass-list=<-loopback>")
                    .arg("--disable-remote-fonts")
                    .arg("--no-pdf-header-footer")
                    .arg(format!("--print-to-pdf={}", destination.display()))
                    .arg(format!("file://{}", source.display()));
                command
            }
        }
    }
}

/// How long a converter gets before it is killed. Short on purpose: the export
/// runs off the UI thread, but quitting Bjorn waits for it, so a wedged
/// browser must not hold the terminal for long.
const CONVERT_TIMEOUT: Duration = Duration::from_secs(60);

/// Bear prints a note on A4. WeasyPrint agrees; Chrome would use US Letter,
/// so the page is named here rather than left to whichever tool is installed.
/// The screen's own column and margins go with it.
const PRINT_PAGE: &str = "<style>@page { size: A4; margin: 18mm 16mm; }\n\
     @media print { body { max-width: none; margin: 0; padding: 0; color: #1a1a1a; background: #fff; } }\n\
     </style>\n</head>";

/// The HTML rendering, printed by whichever converter the machine has.
pub fn export_pdf(
    content: &str,
    title: &str,
    destination: &Path,
    images: &HashMap<String, Vec<u8>>,
) -> Result<PathBuf, ExportError> {
    let Some(converter) = Converter::find() else {
        return Err(ExportError(
            "PDF export needs weasyprint or Google Chrome; macOS ships neither.".into(),
        ));
    };
    let destination = prepare(destination)?;
    let tmp = tempfile::Builder::new()
        .prefix("bjorn-pdf-")
        .tempdir()
        .map_err(|e| ExportError(e.to_string()))?;
    // Plain names inside the temp directory: `#` and `%` are legal in a note's
    // title and mean something else inside the `file://` URL Chrome is handed.
    // The PDF is written here too, and moved into place only once it is whole,
    // so a failure never leaves a stub where the note should be.
    let source = tmp.path().join("note.html");
    let printed = tmp.path().join("note.pdf");
    let html = render_html::render(content, title, images, &HashMap::new());
    let html = only_embedded(&html.replacen("</head>", PRINT_PAGE, 1));
    write(&source, &html)?;
    let log = tmp.path().join("converter.log");
    let mut command = converter.command(&source, &printed, &tmp.path().join("profile"));
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // To a file, not a pipe: a converter that outruns the pipe buffer
        // while nothing is reading it would block there until the timeout.
        .stderr(std::fs::File::create(&log).map_err(|e| ExportError(e.to_string()))?)
        .process_group(0)
        .spawn()
        .map_err(|e| ExportError(format!("{}: {e}", converter.name())))?;
    wait_for_pdf(&mut child, &converter, &printed, &log)?;
    // Across devices a rename fails, so fall back to a copy.
    if std::fs::rename(&printed, &destination).is_err() {
        std::fs::copy(&printed, &destination)
            .map_err(|e| ExportError(format!("{}: {e}", destination.display())))?;
    }
    Ok(destination)
}

/// Wait for the PDF, not for the converter. WeasyPrint prints and exits;
/// headless Chrome has been seen to print in under a second and then spin
/// forever (looping `CVDisplayLinkCreateWithCGDisplay failed` on stderr), so
/// a finished file that has stopped growing is success even while the browser
/// is still running — and the group is killed on the way out either way.
fn wait_for_pdf(
    child: &mut std::process::Child,
    converter: &Converter,
    printed: &Path,
    log: &Path,
) -> Result<(), ExportError> {
    let started = Instant::now();
    let mut settled: Option<(u64, Instant)> = None;
    loop {
        match child.try_wait() {
            // It exited on its own: the file is the only honest signal, since
            // Chrome exits 0 after printing its own error page.
            Ok(Some(status)) => {
                return if status.success() && printed.exists() {
                    Ok(())
                } else {
                    Err(ExportError(format!(
                        "{}: {}",
                        converter.name(),
                        first_complaint(log)
                    )))
                };
            }
            Ok(None) => {}
            Err(e) => {
                kill_group(child);
                return Err(ExportError(format!("{}: {e}", converter.name())));
            }
        }
        if let Ok(size) = std::fs::metadata(printed).map(|m| m.len())
            && size > 0
        {
            match settled {
                Some((seen, at)) if seen == size => {
                    if at.elapsed() >= SETTLE {
                        kill_group(child);
                        return Ok(());
                    }
                }
                _ => settled = Some((size, Instant::now())),
            }
        }
        if started.elapsed() >= CONVERT_TIMEOUT {
            kill_group(child);
            return Err(ExportError(format!(
                "{} took longer than {}s",
                converter.name(),
                CONVERT_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// How long the PDF has to stop growing before a converter that has not
/// exited is taken at its word.
const SETTLE: Duration = Duration::from_millis(400);

fn kill_group(child: &mut std::process::Child) {
    // The whole group, not just the one process: a browser leaves helpers
    // behind, and they are still writing to a profile that is about to be
    // deleted. `process_group(0)` made the child its own leader, so the
    // negative pid names them all. `/bin/kill` rather than a `libc`
    // dependency for the one call.
    let _ = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(format!("-{}", child.id()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

/// The line of a converter's log that says what went wrong. Chrome writes a
/// `ERROR:` line about the display link on every run, headless or not, so a
/// line without that prefix is preferred — but a run whose only output is
/// those lines still has to report one of them rather than nothing.
fn first_complaint(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|l| !l.contains("ERROR:"))
        .or_else(|| lines.first())
        .map(|l| l.to_string())
        .unwrap_or_else(|| "it wrote nothing".to_string())
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
        "pdf" => export_pdf(content, title, destination, images),
        "textbundle" => export_textbundle(content, destination, images),
        other => Err(ExportError(format!("unknown export format {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_embedded_keeps_the_note_and_drops_what_it_points_at() {
        let html = concat!(
            r#"<img src="data:image/png;base64,iVBOR"> "#,
            r#"<img src="https://tracker.test/p.png"> "#,
            r#"<img src="http://127.0.0.1:8080/x" srcset="http://a.test/2x 2x"> "#,
            r#"<img src="file:///Users/you/.ssh/id_rsa"> "#,
            r#"<iframe src="file:///etc/passwd"></iframe> "#,
            r#"<object data="https://b.test/x.pdf"></object> "#,
            r#"<link rel="stylesheet" href="https://c.test/x.css"> "#,
            r#"<a href="https://example.test/read">a link</a> "#,
            r#"<style>@import url("https://d.test/x.css"); body { background: url('https://bg-host.test/bg.png'); }</style>"#,
        );
        let out = only_embedded(html);
        assert!(
            out.contains(r#"<img src="data:image/png;base64,iVBOR">"#),
            "an attachment is already embedded and stays\n{out}"
        );
        for gone in [
            "tracker.test",
            "127.0.0.1:8080",
            "a.test",
            "id_rsa",
            "/etc/passwd",
            "b.test",
            "c.test",
            "d.test",
            "bg-host.test",
        ] {
            assert!(!out.contains(gone), "{gone} still reachable\n{out}");
        }
        assert!(
            out.contains(r#"<a href="https://example.test/read">a link</a>"#),
            "a reader's link is not something the converter fetches\n{out}"
        );
        assert!(out.contains("url(about:blank)"), "{out}");
        // The elements themselves stay; only the reaching stops.
        assert_eq!(out.matches("<img").count(), 4, "{out}");
        assert!(out.contains("<iframe") && out.contains("<object"), "{out}");
    }

    #[test]
    fn a_pdf_carries_no_thread_back_to_whoever_wrote_the_note() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Note.pdf");
        let note =
            "# Note\n\n<img src=\"https://tracker.test/pixel.png\">\n\n![](Front%20bed.png)\n";
        let images = HashMap::from([("Front bed.png".to_string(), b"\x89PNG\r\n".to_vec())]);
        let Some(_) = Converter::find() else { return };
        let written = export_pdf(note, "Note", &target, &images).expect("a PDF");
        let bytes = std::fs::read(&written).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        // The URL cannot survive into the PDF, because it never reached the
        // page the converter was handed.
        assert!(
            !String::from_utf8_lossy(&bytes).contains("tracker.test"),
            "the tracker's URL is in the PDF"
        );
    }

    #[test]
    fn pdf_sits_in_the_picker_with_a_key_of_its_own() {
        let pdf = format_by_id("pdf");
        assert_eq!((pdf.label, pdf.key, pdf.ext), ("PDF", 'p', "pdf"));
        assert!(pdf.needs_attachments, "a note's images belong in its PDF");
        assert_eq!(extension_for(pdf, true), "pdf");
        let mut keys: Vec<char> = FORMATS.iter().map(|f| f.key).collect();
        keys.sort_unstable();
        let unique = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), unique, "two formats on one key");
    }

    #[test]
    fn export_pdf_writes_a_pdf_or_says_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Note.pdf");
        let result = export_pdf("# Note\n\nA line.\n", "Note", &target, &HashMap::new());
        match Converter::find() {
            // Nothing to print with is the ordinary case on a machine that has
            // not installed one, and it has to say so rather than fail blankly.
            None => {
                let message = result.expect_err("no converter, no PDF").0;
                assert!(message.contains("weasyprint"), "{message}");
                assert!(message.contains("Chrome"), "{message}");
                assert!(!target.exists(), "it says so before it writes anything");
            }
            Some(converter) => {
                let written = result.unwrap_or_else(|e| panic!("{converter:?}: {}", e.0));
                let bytes = std::fs::read(&written).unwrap();
                assert!(bytes.starts_with(b"%PDF"), "{converter:?} wrote no PDF");
                assert!(
                    bytes.len() > 1000,
                    "{converter:?} wrote {} bytes",
                    bytes.len()
                );
            }
        }
    }

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
