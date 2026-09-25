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

/// The note's body with everything a converter would fetch taken out.
///
/// A converter fetches what the page it is given points at: WeasyPrint has no
/// flag to stop it, and Chrome's `--host-resolver-rules` cannot help with
/// `file:`, which is the scheme the page itself is loaded from. A Bear note is
/// not always one you wrote — imports, web clips, a note someone shared — and
/// its inline HTML reaches the converter as written, so a remote image is a
/// note telling somebody it was printed, and a local one bakes a file off the
/// disk into a PDF that is usually about to be sent on.
///
/// So the body is parsed as a browser would parse it and rebuilt from an
/// allowlist: no `<script>`, `<style>`, `<link>`, `<svg>`, frame or object
/// survives, no `style` attribute, no `srcset`, and an image keeps its `src`
/// only when it is a `data:` URI. Parsing is what closes the gaps a pattern
/// over the text leaves (`<img/src=…>`, `alt=""src=…`, CSS escapes), and it
/// is also why the note's words come through untouched: "url(" in a sentence
/// or `<img>` in a code block is text, never markup.
///
/// Nothing is lost that was going to print: an attachment is already a
/// `data:` URI by the time the HTML is written. The page around the body —
/// the stylesheet and `@page` rules — is Bjorn's own and is left as it is.
pub fn only_embedded(html: &str) -> String {
    // Without the markers `render` writes, the whole page is filtered: it
    // loses its styling, which is better than printing an unfiltered note.
    let (Some(open), Some(close)) = (html.find("<body>\n"), html.rfind("</body>")) else {
        return SANITIZER.clean(html).to_string();
    };
    let body_at = open + "<body>\n".len();
    if close < body_at {
        return SANITIZER.clean(html).to_string();
    }
    format!(
        "{}{}{}",
        &html[..body_at],
        SANITIZER.clean(&html[body_at..close]),
        &html[close..]
    )
}

static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::default();
    builder
        // Bjorn's own marks: task boxes, tag pills, highlights.
        .add_tags(["input", "mark", "u"])
        .add_tag_attributes("input", ["type", "disabled", "checked"])
        .add_generic_attributes(["class"])
        // `data:` is allowed here for images; the filter below keeps it off
        // links and vets what an image carries. A relative link would point
        // into the temp directory the page was printed from, and a `bear:`
        // link would act on the Bear of whoever clicks it in the PDF.
        .add_url_schemes(["data"])
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .attribute_filter(|element, attribute, value| match (element, attribute) {
            ("img", "src") => printable_image(value).then(|| value.into()),
            ("a", "href") if scheme_is_data(value) => None,
            _ => Some(value.into()),
        });
    builder
});

/// A URL's scheme is `data` the way a parser reads it: leading control
/// characters and spaces are skipped, and tabs and newlines anywhere are
/// dropped, so `da&#9;ta:` is still `data:`.
fn scheme_is_data(value: &str) -> bool {
    let bare: String = value
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .take(5)
        .collect();
    bare.eq_ignore_ascii_case("data:")
}

/// Raster formats whose bytes cannot be read as anything else, by their
/// signature. SVG is not one: WeasyPrint fetches what an SVG image points at,
/// `<image href>` and `<use>` included, remote or `file:`, and it tries any
/// bytes Pillow cannot decode as SVG whatever the type says. Nor is anything
/// Pillow hands to Ghostscript (EPS). An attachment in another format (SVG,
/// HEIC, PDF) prints as an empty frame; neither converter drew HEIC anyway.
const IMAGE_SIGNATURES: [&[u8]; 8] = [
    b"\x89PNG\r\n\x1a\n",
    b"\xff\xd8\xff",
    b"GIF87a",
    b"GIF89a",
    b"RIFF", // WebP: RIFF, a length, then WEBP (checked below)
    b"BM",
    b"II*\0",
    b"MM\0*",
];

/// An image the converter may draw: a base64 `data:` URI whose bytes start
/// with one of `IMAGE_SIGNATURES`. The declared type is not trusted.
fn printable_image(value: &str) -> bool {
    use base64::Engine;
    if !scheme_is_data(value) {
        return false;
    }
    let Some((head, payload)) = value.trim().split_once(',') else {
        return false;
    };
    if !head.to_ascii_lowercase().ends_with(";base64") {
        return false;
    }
    // Enough for the longest signature, rounded to whole base64 quanta.
    let prefix: String = payload
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .take(16)
        .collect();
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&prefix) else {
        return false;
    };
    IMAGE_SIGNATURES.iter().any(|sig| bytes.starts_with(sig))
        && (!bytes.starts_with(b"RIFF") || bytes.get(8..12) == Some(b"WEBP"))
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
                    // Nowhere to send anything, behind `only_embedded`: no
                    // hostname resolves, and a bare IP address — which never
                    // reaches the resolver — meets a dead proxy. Scripts are
                    // kept out by `only_embedded` alone. Chromium has no
                    // `--disable-javascript` (a page's script still ran with
                    // it), and `--blink-settings=scriptEnabled=false` breaks
                    // --print-to-pdf outright: empty file, exit code 0.
                    .arg("--host-resolver-rules=MAP * ~NOTFOUND")
                    .arg("--proxy-server=127.0.0.1:1")
                    .arg("--proxy-bypass-list=<-loopback>")
                    .arg("--disable-remote-fonts")
                    // A fresh profile would otherwise start its first-run and
                    // background work (component updates, sync, a Keychain
                    // prompt for the new profile's passwords).
                    .arg("--no-first-run")
                    .arg("--no-default-browser-check")
                    .arg("--disable-background-networking")
                    .arg("--disable-component-update")
                    .arg("--disable-sync")
                    .arg("--disable-extensions")
                    .arg("--use-mock-keychain")
                    .arg("--password-store=basic")
                    .arg("--no-pdf-header-footer")
                    .arg(format!("--print-to-pdf={}", destination.display()))
                    .arg(file_url(source));
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
    // Across devices a rename fails, so fall back to a copy — into a temp file
    // beside the destination, renamed over it once whole, so a full disk
    // leaves no stub and a symlink sitting at the destination is replaced
    // rather than followed.
    if std::fs::rename(&printed, &destination).is_err() {
        let failed =
            |e: &dyn std::fmt::Display| ExportError(format!("{}: {e}", destination.display()));
        let parent = destination.parent().unwrap_or(Path::new("."));
        let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| failed(&e))?;
        let mut pdf = std::fs::File::open(&printed).map_err(|e| failed(&e))?;
        std::io::copy(&mut pdf, &mut staged).map_err(|e| failed(&e))?;
        // A temp file is 0600; the PDF keeps the mode the converter gave it.
        if let Ok(meta) = pdf.metadata() {
            let _ = staged.as_file().set_permissions(meta.permissions());
        }
        staged.persist(&destination).map_err(|e| failed(&e.error))?;
    }
    Ok(destination)
}

/// The page as a `file://` URL. The temp directory's path comes from
/// `$TMPDIR`, and a `#`, `%` or `?` in it, left bare, would send Chrome
/// somewhere else — to its own error page, which it prints and exits 0.
fn file_url(path: &Path) -> String {
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}');
    format!(
        "file://{}",
        percent_encoding::utf8_percent_encode(&path.to_string_lossy(), PATH)
    )
}

/// Wait for the PDF, not for the converter. WeasyPrint prints and exits, and
/// is waited for; it writes as it goes, so a pause in its output means
/// nothing. Headless Chrome has been seen to print in under a second and then
/// spin forever (looping `CVDisplayLinkCreateWithCGDisplay failed` on
/// stderr), so for Chrome alone a file that has stopped growing is taken as
/// done while the browser still runs. Either way the file has to be a whole
/// PDF (`whole_pdf`) before it counts, and the group is killed on the way
/// out, whether the converter exited on its own or not: a browser's helpers
/// can outlive it.
fn wait_for_pdf(
    child: &mut std::process::Child,
    converter: &Converter,
    printed: &Path,
    log: &Path,
) -> Result<(), ExportError> {
    let started = Instant::now();
    let mut settled: Option<(u64, Instant)> = None;
    let failed = |why: String| Err(ExportError(format!("{}: {why}", converter.name())));
    loop {
        match child.try_wait() {
            // It exited on its own: the file is the only honest signal, since
            // Chrome exits 0 after printing its own error page.
            Ok(Some(status)) => {
                kill_group(child);
                return if status.success() && whole_pdf(printed) {
                    Ok(())
                } else if status.success() && printed.exists() {
                    failed("wrote an incomplete PDF".into())
                } else {
                    failed(first_complaint(log))
                };
            }
            Ok(None) => {}
            Err(e) => {
                kill_group(child);
                return failed(e.to_string());
            }
        }
        if matches!(converter, Converter::Chrome(_))
            && let Ok(size) = std::fs::metadata(printed).map(|m| m.len())
            && size > 0
        {
            match settled {
                Some((seen, at)) if seen == size => {
                    if at.elapsed() >= SETTLE && whole_pdf(printed) {
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

/// A PDF that was finished: `%PDF-` at the front and `%%EOF` in its last
/// kilobyte (the spec allows a little trailing whitespace, and some writers
/// add more). An empty file, or one cut off mid-write, has no trailer.
fn whole_pdf(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let tail = &bytes[bytes.len().saturating_sub(1024)..];
    bytes.starts_with(b"%PDF-") && tail.windows(5).any(|w| w == b"%%EOF")
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

    fn page(body: &str) -> String {
        format!("<html><head><style>a {{ color: red; }}</style></head><body>\n{body}</body></html>")
    }

    #[test]
    fn only_embedded_keeps_the_note_and_drops_what_it_points_at() {
        let html = page(concat!(
            r#"<p><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg=="> "#,
            r#"<img src="https://tracker.test/p.png"> "#,
            r#"<img src="http://127.0.0.1:8080/x" srcset="http://a.test/2x 2x"> "#,
            r#"<img src="file:///Users/you/.ssh/id_rsa"> "#,
            r#"<a href="https://example.test/read">a link</a></p>"#,
            r#"<iframe src="file:///etc/passwd"></iframe> "#,
            r#"<object data="https://b.test/x.pdf"></object> "#,
            r#"<link rel="stylesheet" href="https://c.test/x.css"> "#,
            r#"<style>@import url("https://d.test/x.css"); body { background: url('https://bg-host.test/bg.png'); }</style>"#,
        ));
        let out = only_embedded(&html);
        assert!(
            out.contains(r#"<img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==">"#),
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
        // The images stay as empty frames; only the reaching stops.
        assert_eq!(out.matches("<img").count(), 4, "{out}");
        assert!(
            out.starts_with("<html><head><style>a { color: red; }</style></head><body>\n"),
            "the page around the body is Bjorn's own and is left alone\n{out}"
        );
    }

    #[test]
    fn only_embedded_parses_rather_than_pattern_matches() {
        // Each of these got past the patterns this used to be.
        let html = page(concat!(
            r#"<img alt=""src="https://one.test/p.png">"#,
            r#"<img/src="https://two.test/p.png">"#,
            r#"<svg><image href="https://three.test/i.png"/></svg>"#,
            r#"<svg><image xlink:href="file:///four/secret"/></svg>"#,
            r#"<p style="background: \75 rl(https://five.test/bg)">styled</p>"#,
            r#"<div style="background-image: url(https://six.test/bg)">x</div>"#,
            r#"<script>fetch("https://seven.test/")</script>"#,
            r#"<video poster="https://eight.test/p.png"></video>"#,
            r#"<a href="data:text/html,hi">nine</a>"#,
            r#"<img src=" data:image/png;base64,AA" onerror="fetch('https://ten.test')">"#,
        ));
        let out = only_embedded(&html);
        for gone in [
            "one.test",
            "two.test",
            "three.test",
            "four",
            "five.test",
            "six.test",
            "seven.test",
            "eight.test",
            "data:text",
            "ten.test",
            "<script",
            "<svg",
            "style=",
        ] {
            assert!(!out.contains(gone), "{gone} survived\n{out}");
        }
        assert!(out.contains("styled") && out.contains("nine"), "{out}");
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn an_image_prints_only_when_its_bytes_are_a_raster_format() {
        // WeasyPrint fetches what an SVG image points at, and reads bytes
        // Pillow cannot decode as SVG whatever the declared type; EPS goes to
        // Ghostscript. Only a known raster signature is let through.
        let svg = b64(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://svg.test/p"/></svg>"#,
        );
        let eps = b64(b"%!PS-Adobe-3.0 EPSF-3.0\n");
        let webp = b64(b"RIFF\x1a\0\0\0WEBPVP8 ");
        let riff = b64(b"RIFF\x1a\0\0\0AVI LIST");
        let cases = [
            (
                format!(
                    "data:image/png;base64,{}",
                    b64(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
                ),
                true,
            ),
            (
                format!(
                    "data:image/jpeg;base64,{}",
                    b64(b"\xff\xd8\xff\xe0\0\x10JFIF")
                ),
                true,
            ),
            (
                format!("data:image/gif;base64,{}", b64(b"GIF89a\x01\0\x01\0")),
                true,
            ),
            (format!("data:image/webp;base64,{webp}"), true),
            (
                format!(
                    "DATA:image/png;BASE64,{}",
                    b64(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
                ),
                true,
            ),
            (format!("data:image/svg+xml;base64,{svg}"), false),
            (format!("data:image/png;base64,{svg}"), false),
            (format!("data:image/png;base64,{eps}"), false),
            (format!("data:image/webp;base64,{riff}"), false),
            (
                "data:image/svg+xml,<svg><image href='https://svg.test/q'/></svg>".into(),
                false,
            ),
            ("data:image/png;base64,iVBOR".into(), false),
            ("data:text/html;base64,PHNjcmlwdD4=".into(), false),
        ];
        for (src, prints) in cases {
            let out = only_embedded(&page(&format!(r#"<img src="{src}">"#)));
            assert_eq!(out.contains("src="), prints, "{src}\n{out}");
        }
    }

    #[test]
    fn a_link_in_the_pdf_goes_nowhere_it_should_not() {
        let out = only_embedded(&page(concat!(
            r#"<a href="&#1;data:text/html,one">one</a>"#,
            r#"<a href="da&#9;ta:text/html,two">two</a>"#,
            r#"<a href="bear://x-callback-url/trash?id=X">three</a>"#,
            r#"<a href="plan.pdf">four</a>"#,
            r#"<a href="https://example.test/">five</a>"#,
        )));
        for gone in ["data:", "da\tta", "bear:", "plan.pdf"] {
            assert!(!out.contains(gone), "{gone} survived\n{out}");
        }
        assert!(
            out.contains(r#"<a href="https://example.test/">five</a>"#),
            "{out}"
        );
        assert!(
            ["one", "two", "three", "four"]
                .iter()
                .all(|t| out.contains(t)),
            "{out}"
        );
    }

    #[test]
    fn a_page_without_the_body_markers_is_filtered_whole() {
        let out = only_embedded(
            r#"<body class="x"><img src="https://tracker.test/p"><p>text</p></body>"#,
        );
        assert!(
            !out.contains("tracker.test") && out.contains("text"),
            "{out}"
        );
    }

    #[test]
    fn a_markdown_image_is_held_to_the_same_rule() {
        let svg = b64(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://svg.test/p"/></svg>"#,
        );
        let note = format!(
            "![](data:image/svg+xml;base64,{svg})\n\n![](https://md.test/p.png)\n\n![](file:///etc/hosts)\n"
        );
        let html = render_html::render(&note, "N", &HashMap::new(), &HashMap::new());
        let out = only_embedded(&html);
        // Only the note's body: Bjorn's own stylesheet may carry an SVG of its
        // own (a checkbox's tick), which fetches nothing.
        let body = &out[out.find("<body>").expect("a body")..];
        for gone in ["svg+xml", "md.test", "/etc/hosts"] {
            assert!(!body.contains(gone), "{gone} survived\n{out}");
        }
    }

    #[test]
    fn the_page_url_survives_a_temp_path_with_url_characters() {
        let url = file_url(Path::new("/tmp/a b#c%d?e/note.html"));
        assert_eq!(url, "file:///tmp/a%20b%23c%25d%3Fe/note.html");
    }

    #[test]
    fn only_embedded_leaves_the_notes_own_words_alone() {
        let note = "Some url(s) here.\n\nAlso @import foo; bar.\n\n\
                    ```\n<img src=\"a.png\">\n```\n\n\
                    #tag\n\n- [x] done ==marked== ~under~\n";
        let html = render_html::render(note, "Words", &HashMap::new(), &HashMap::new());
        let out = only_embedded(&html);
        assert!(out.contains("Some url(s) here."), "{out}");
        assert!(out.contains("Also @import foo; bar."), "{out}");
        assert!(out.contains("&lt;img src=\"a.png\"&gt;"), "{out}");
        assert!(
            out.contains(r#"<input type="checkbox" disabled="" checked="">"#),
            "{out}"
        );
        assert!(
            out.contains(">marked</mark>") && out.contains("<u>under</u>"),
            "{out}"
        );
        assert!(out.contains(r#"<span class="tag">"#), "{out}");
        assert_eq!(
            out.split("</style>").next(),
            html.split("</style>").next(),
            "the stylesheet is not the note's and is not filtered"
        );
    }

    #[test]
    fn a_pdf_carries_no_thread_back_to_whoever_wrote_the_note() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Note.pdf");
        let note =
            "# Note\n\n<img src=\"https://tracker.test/pixel.png\">\n\n![](Front%20bed.png)\n";
        let images = HashMap::from([(
            "Front bed.png".to_string(),
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
        )]);
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
    fn only_a_whole_pdf_counts_as_printed() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        assert!(whole_pdf(&file("whole.pdf", b"%PDF-1.7\n1 0 obj\n%%EOF\n")));
        assert!(!whole_pdf(&file("empty.pdf", b"")));
        assert!(!whole_pdf(&file("cut.pdf", b"%PDF-1.7\n1 0 obj\n<< /Ty")));
        assert!(!whole_pdf(&file("page.pdf", b"<html>%%EOF")));
        assert!(!whole_pdf(&dir.path().join("missing.pdf")));
        // A trailer the size of a kilobyte after %%EOF is not one it wrote.
        let mut padded = b"%PDF-1.7\n%%EOF".to_vec();
        padded.extend(std::iter::repeat_n(b'\n', 2048));
        assert!(!whole_pdf(&file("padded.pdf", &padded)));
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
