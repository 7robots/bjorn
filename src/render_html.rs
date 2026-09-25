//! Bear markdown to a self-contained HTML document, for export.
//!
//! The pre-pass mirrors the reader's but emits HTML: task boxes become
//! disabled checkboxes, highlights `<mark>`, underline `<u>`, the tag line a
//! row of `.tag` spans. pulldown-cmark (tables, strikethrough, inline HTML)
//! does the rest. Attachment images are embedded as `data:` URIs when their
//! bytes are given, or pointed at paths.

use std::collections::HashMap;
use std::sync::LazyLock;

use base64::Engine;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::Color;
use regex::Regex;

use crate::render::{
    HIGHLIGHT_RE, TASK_DONE_RE, TASK_OPEN_RE, UNDERLINE_RE, is_fence, is_tag_line, percent_decode,
    tags_in_line,
};
use crate::ui::theme::Theme;

/// A color as plain channels, the form the contrast maths wants.
type Rgb = (u8, u8, u8);

/// A color as `#rrggbb`.
fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Relative luminance per WCAG.
fn luminance((r, g, b): Rgb) -> f64 {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
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

/// `color` mixed toward black or white until it reads at 4.5:1 on `page`.
/// A theme's accent is picked for a terminal, not for paper: pale ones would
/// all but vanish on the white page a printed note gets, and dark ones on the
/// dark page the browser gives at night.
fn readable(color: Color, page: Rgb) -> String {
    // Every theme color is `Color::Rgb`; a terminal palette entry has no hex,
    // so the page's own ink stands in rather than a color nobody can print.
    let Color::Rgb(r, g, b) = color else {
        return hex(if luminance(page) > 0.5 {
            (0x11, 0x11, 0x11)
        } else {
            (0xdd, 0xdd, 0xdd)
        });
    };
    let toward: f64 = if luminance(page) > 0.5 { 0.0 } else { 255.0 };
    let mut mixed = (r, g, b);
    for step in 0..=20 {
        let t = step as f64 / 20.0;
        let mix = |v: u8| (v as f64 + (toward - v as f64) * t).round() as u8;
        mixed = (mix(r), mix(g), mix(b));
        if contrast(mixed, page) >= 4.5 {
            break;
        }
    }
    hex(mixed)
}

/// The light page an exported note is read and printed on, and the dark one
/// a browser set to dark mode gives it instead.
const LIGHT_PAGE: Rgb = (0xff, 0xff, 0xff);
const DARK_PAGE: Rgb = (0x1e, 0x1e, 0x1e);

/// The document's stylesheet, with `theme`'s accent on the links and the list
/// markers the way Bear puts it on its own. The rest of the page stays
/// neutral, light or dark as the browser asks, and always light on paper:
/// `@media print` is what a note exported as HTML and printed to PDF gets,
/// and it is laid out to come out like Bear's own PDF.
pub fn stylesheet(theme: &Theme) -> String {
    let on_light = readable(theme.link, LIGHT_PAGE);
    let on_dark = readable(theme.link, DARK_PAGE);
    let bullet_light = readable(theme.bullet, LIGHT_PAGE);
    let bullet_dark = readable(theme.bullet, DARK_PAGE);
    format!(
        r#":root {{ color-scheme: light dark; --accent: {on_light}; --bullet: {bullet_light}; }}
body {{ max-width: 44em; margin: 2em auto; padding: 0 1.5em; font: 16px/1.55 -apple-system, "Helvetica Neue", Helvetica, Arial, sans-serif; color: #222; background: #fff; }}
h1, h2, h3, h4, h5, h6 {{ line-height: 1.25; margin: 1.4em 0 0.5em; }}
h1 {{ font-size: 1.8em; margin-top: 0; }}
h2 {{ font-size: 1.4em; }}
h3 {{ font-size: 1.15em; }}
/* Bear sets h4 to h6 at body size, bold; browsers shrink h5 and h6 below it. */
h4, h5, h6 {{ font-size: 1em; }}
a {{ color: var(--accent); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
/* A note link, `[[Another note]]`: Bear shows the target, colored, not brackets. */
.note-link {{ color: var(--accent); }}
u {{ text-decoration-color: var(--bullet); }}
del {{ color: #888; }}
/* Bear's highlighter palette, from its own theme files: a color Bear writes
   as an emoji at the front of the run, and the ink it puts on each. */
mark {{ padding: 0 0.15em; border-radius: 2px; background: #d3ffa4; color: #1a3200; }}
mark.red {{ background: #ffd5d5; color: #321a00; }}
mark.green {{ background: #cdf7bd; color: #102d05; }}
mark.blue {{ background: #c9e5ff; color: #001a32; }}
mark.yellow {{ background: #fcf195; color: #312c01; }}
mark.purple {{ background: #fedaff; color: #310032; }}
pre, code {{ font: 0.92em/1.45 ui-monospace, "SF Mono", Menlo, monospace; background: #f4f4f4; border-radius: 4px; }}
code {{ padding: 0.1em 0.3em; }}
pre {{ padding: 0.8em 1em; overflow-x: auto; }}
pre code {{ padding: 0; background: none; }}
blockquote {{ margin: 1em 0; padding: 0 1em; border-left: 3px solid var(--bullet); }}
/* `> [!NOTE]` and its four siblings: Bear's panels, a bar and a tint each. The
   ink is set here because the tint stays light on a dark page. */
.callout {{ margin: 1em 0; padding: 0.7em 1em; border-left: 4px solid; border-radius: 4px; color: #1a1a1a; }}
.callout > :first-child {{ margin-top: 0; }}
.callout > p:first-child {{ font-weight: 600; }}
.callout > :last-child {{ margin-bottom: 0; }}
.callout.note {{ border-color: #3b78b1; background: #f6fbff; }}
.callout.tip {{ border-color: #5bb13a; background: #f7fdf5; }}
.callout.important {{ border-color: #af3db2; background: #fef9ff; }}
.callout.warning {{ border-color: #fabd05; background: #fefcef; }}
.callout.caution {{ border-color: #ff8500; background: #fef7f0; }}
table {{ border-collapse: collapse; margin: 1em 0; }}
th, td {{ border: 1px solid #ddd; padding: 0.35em 0.7em; text-align: left; }}
th {{ background: #f7f7f9; }}
img {{ max-width: 100%; height: auto; }}
ul, ol {{ padding-left: 1.5em; }}
/* Bear alternates filled and hollow markers by depth; browsers reach a square. */
ul {{ list-style: disc; }}
ul ul, ul ul ul ul, ul ul ul ul ul ul, ul ul ul ul ul ul ul ul, ul ul ul ul ul ul ul ul ul ul {{ list-style: circle; }}
ul ul ul, ul ul ul ul ul, ul ul ul ul ul ul ul, ul ul ul ul ul ul ul ul ul {{ list-style: disc; }}
li {{ margin: 0.15em 0; }}
li::marker {{ color: var(--bullet); }}
li.task::marker {{ color: transparent; }}
/* A ticked task is grayed out in Bear, box and words together. */
li.task.done {{ color: #999; }}
/* display: some converters lay a bare checkbox out as a block, which drops the task's text onto its own line. */
input[type=checkbox] {{ appearance: none; -webkit-appearance: none; display: inline-block; width: 0.95em; height: 0.95em; margin: 0 0.4em 0 0; vertical-align: -0.15em; border: 1.5px solid #c3c3c7; border-radius: 4px; background: transparent; }}
input[type=checkbox]:checked {{ background: #ececec url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Cpath d='M3.5 8.5l3 3 6-6' fill='none' stroke='%23999' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E") center/0.8em no-repeat; }}
.tags {{ margin: -0.5em 0 1.5em; }}
.tag {{ display: inline-block; background: #eee; color: #555; border-radius: 1em; padding: 0.05em 0.7em; margin-right: 0.3em; font-size: 0.85em; }}
hr {{ border: 0; border-top: 1px solid #ddd; margin: 2em 0; }}
/* A browser set to dark mode. This block sits below the rules it overrides:
   a media query adds no specificity, so only its place in the sheet decides,
   and the print block below it has the last word on paper. */
@media (prefers-color-scheme: dark) {{
  :root {{ --accent: {on_dark}; --bullet: {bullet_dark}; }}
  body {{ color: #ddd; background: #1e1e1e; }}
  del {{ color: #999; }}
  pre, code {{ background: #2a2a2a; }}
  th, td {{ border-color: #444; }}
  th {{ background: #2a2a2a; }}
  .tag {{ background: #3a3a3a; color: #ccc; }}
  /* A callout keeps its light tint, so everything on it keeps dark ink. */
  .callout pre, .callout code {{ background: #00000010; color: #1a1a1a; }}
  .callout a, .callout .note-link, .callout li::marker {{ color: #1a3d5c; }}
}}
@page {{ size: A4; margin: 18mm 16mm; }}
@media print {{
  :root {{ --accent: {on_light}; --bullet: {bullet_light}; }}
  body {{ max-width: none; margin: 0; padding: 0; font-size: 11pt; color: #1a1a1a; background: #fff; }}
  h1 {{ font-size: 2em; }}
  h1, h2, h3, h4, h5, h6 {{ color: #111; break-after: avoid; }}
  pre, code {{ background: #f5f5f5; color: #1a1a1a; }}
  pre {{ white-space: pre-wrap; }}
  th, td {{ border-color: #e0e0e0; }}
  th {{ background: #f7f7f9; }}
  .tag {{ background: #ececec; color: #6b6b6b; }}
  /* A quote or a code block longer than the page has to be allowed to split. */
  li, tr, img {{ break-inside: avoid; }}
  /* The pills, the highlights and the code ground are the note, not decoration. */
  mark, .tag, pre, code, th, .callout, input[type=checkbox] {{ print-color-adjust: exact; -webkit-print-color-adjust: exact; }}
}}"#
    )
}

static NOTE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]\n]+)\]\]").unwrap());
static LI_TASK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<li>\s*(<p>)?\s*(<input type="checkbox"[^>]*>)"#).unwrap());

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

fn task_inputs(line: &str) -> String {
    let line = TASK_OPEN_RE.replace(line, |caps: &regex::Captures| {
        format!(
            "{}<input type=\"checkbox\" disabled>{}",
            &caps[1],
            caps.get(2)
                .map(|m| m.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(" ")
        )
    });
    TASK_DONE_RE
        .replace(&line, |caps: &regex::Captures| {
            format!(
                "{}<input type=\"checkbox\" disabled checked>{}",
                &caps[1],
                caps.get(2)
                    .map(|m| m.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(" ")
            )
        })
        .into_owned()
}

/// Bear writes a highlight's color as a colored circle at the front of the
/// run: `==🟢green==`. The emoji is the color, not text, so it comes out of
/// the words and goes into a class. An unknown leading emoji is left alone and
/// the highlight takes Bear's default, as does a run that is nothing but a
/// circle: the character stays rather than the highlight emptying out.
fn highlight_color(text: &str) -> (&'static str, &str) {
    for (emoji, name) in [
        ("🔴", "red"),
        ("🟢", "green"),
        ("🔵", "blue"),
        ("🟡", "yellow"),
        ("🟣", "purple"),
    ] {
        if let Some(rest) = text.strip_prefix(emoji) {
            // An emoji presentation selector rides along behind the circle.
            let rest = rest.strip_prefix('\u{fe0f}').unwrap_or(rest).trim_start();
            if rest.is_empty() {
                break;
            }
            return (name, rest);
        }
    }
    ("default", text)
}

/// The five callouts Bear draws as a panel, by the marker that opens one.
const CALLOUTS: [&str; 5] = ["note", "tip", "important", "warning", "caution"];

/// `> [!NOTE] …` and the quote lines under it. `None` when the line opens no
/// callout.
fn callout_kind(line: &str) -> Option<&'static str> {
    // Only at column 0: an indented one belongs to whatever holds it — a list
    // item, say — where a panel of raw HTML would break the list in two.
    let rest = line.strip_prefix('>')?.trim_start();
    let marker = rest.strip_prefix("[!")?;
    let end = marker.find(']')?;
    let name = marker[..end].to_ascii_lowercase();
    CALLOUTS.iter().copied().find(|k| *k == name)
}

/// The text of a quote line, without its `>`.
fn unquote(line: &str) -> &str {
    let rest = line.trim_start().strip_prefix('>').unwrap_or(line);
    rest.strip_prefix(' ').unwrap_or(rest)
}

/// Bear markdown -> markdown with inline HTML for Bear's own marks.
pub fn prepare(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut in_callout = false;
    for line in content.lines() {
        // A callout runs to the first line that is not a quote line; a plain
        // quote under one is part of it, the way Bear keeps the panel open.
        // A fence closes it too, and before it opens: otherwise the code block
        // is drawn inside a panel it was never in, and its `</div>` is left
        // to come out as text somewhere in the middle of the code.
        if in_callout && !in_fence && !line.trim_start().starts_with('>') {
            out.push("</div>".to_string());
            out.push(String::new());
            in_callout = false;
        }
        if is_fence(line) {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }
        if let Some(kind) = callout_kind(line) {
            // The blank line ends the HTML block, so what follows is still
            // markdown; the closing div is a block of its own.
            out.push(format!("<div class=\"callout {kind}\">"));
            out.push(String::new());
            in_callout = true;
            let rest = unquote(line);
            let text = rest[rest.find(']').map(|at| at + 1).unwrap_or(0)..].trim_start();
            if !text.is_empty() {
                out.push(quoted_line(text));
                // The title is a paragraph of its own; without the break the
                // body would run into it and be bolded with it.
                out.push(String::new());
            }
            continue;
        }
        if in_callout {
            out.push(quoted_line(unquote(line)));
            continue;
        }
        out.push(quoted_line(line));
    }
    if in_callout {
        out.push("</div>".to_string());
    }
    format!("{}\n", out.join("\n"))
}

/// A line of body text: the tag line becomes pills, anything else its marks.
fn quoted_line(line: &str) -> String {
    if is_tag_line(line) {
        let spans: String = tags_in_line(line)
            .iter()
            .map(|t| format!("<span class=\"tag\">{}</span>", escape(t)))
            .collect();
        // An HTML block runs to the next blank line: without one, a heading or
        // list right under the tag line is swallowed into it.
        return format!("<p class=\"tags\">{spans}</p>\n");
    }
    // A setext underline (`=====`) is a heading marker, not a highlight.
    if line.trim().chars().all(|c| c == '=') && !line.trim().is_empty() {
        return line.to_string();
    }
    inline(line)
}

/// One line's Bear marks: task boxes, highlights, underline and the note links
/// Bear writes as `[[Another note]]`. Inline code is left alone — a mark
/// between backticks is a character the note is showing, not one it is using.
fn inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for (at, segment) in code_spans(line).into_iter().enumerate() {
        if at % 2 == 1 {
            out.push('`');
            out.push_str(segment);
            out.push('`');
        } else {
            // Only the first segment is the start of the line, which is where
            // a task box has to be.
            out.push_str(&marks(segment, at == 0));
        }
    }
    out
}

/// A line cut on the backticks that pair up into inline code: even segments
/// are outside code, odd ones inside. A backtick with no partner opens
/// nothing, so it comes back as part of the text.
fn code_spans(line: &str) -> Vec<&str> {
    let ticks: Vec<usize> = line.match_indices('`').map(|(at, _)| at).collect();
    let mut parts = Vec::new();
    let mut from = 0;
    for &[open, close] in ticks.as_chunks::<2>().0 {
        parts.push(&line[from..open]);
        parts.push(&line[open + 1..close]);
        from = close + 1;
    }
    parts.push(&line[from..]);
    parts
}

/// Bear's marks in one stretch of ordinary text.
fn marks(text: &str, line_start: bool) -> String {
    let text = if line_start {
        task_inputs(text)
    } else {
        text.to_string()
    };
    let text = HIGHLIGHT_RE.replace_all(&text, |caps: &fancy_regex::Captures<'_, str>| {
        let (color, inner) = highlight_color(&caps[1]);
        format!("<mark class=\"{color}\">{inner}</mark>")
    });
    let text = UNDERLINE_RE.replace_all(&text, "<u>$1</u>");
    let owned = text.into_owned();
    NOTE_LINK_RE
        .replace_all(&owned, |caps: &regex::Captures| {
            // `[[x]](y)` is an ordinary link whose text happens to be
            // bracketed; Bear's note link stands on its own.
            let after = caps.get(0).map(|m| m.end()).unwrap_or(0);
            if owned[after..].starts_with('(') {
                return caps[0].to_string();
            }
            format!("<span class=\"note-link\">{}</span>", escape(&caps[1]))
        })
        .into_owned()
}

pub fn mime_type(filename: &str) -> &'static str {
    match filename
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("heic") => "image/heic",
        Some("tif") | Some("tiff") => "image/tiff",
        Some("bmp") => "image/bmp",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("md") => "text/markdown",
        _ => "application/octet-stream",
    }
}

pub fn data_uri(filename: &str, data: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        mime_type(filename),
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

/// Whether a link target would run something rather than go somewhere.
/// Browsers drop leading blanks and controls and any tab or newline inside a
/// URL before they read its scheme, so `java\tscript:` is still one; the check
/// does the same.
fn is_script_url(url: &str) -> bool {
    let scheme: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .take_while(|&c| c != ':')
        .collect::<String>()
        .to_ascii_lowercase();
    url.contains(':') && matches!(scheme.as_str(), "javascript" | "vbscript" | "data")
}

/// The note as an HTML fragment. `images` maps attachment filenames to bytes
/// for embedding; `image_src` maps them to URLs to reference instead.
pub fn render_body(
    content: &str,
    images: &HashMap<String, Vec<u8>>,
    image_src: &HashMap<String, String>,
) -> String {
    let prepared = prepare(content);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    // A markdown link to `javascript:` and its kin keeps its text and loses
    // the link. Links do not nest, so one flag pairs a dropped start with its
    // end.
    let mut dropped_link = false;
    let events = Parser::new_ext(&prepared, options).filter_map(move |event| match event {
        Event::Start(Tag::Link { ref dest_url, .. }) if is_script_url(dest_url) => {
            dropped_link = true;
            None
        }
        Event::End(TagEnd::Link) if dropped_link => {
            dropped_link = false;
            None
        }
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let name = percent_decode(&dest_url);
            let src = if let Some(path) = image_src.get(&name) {
                path.clone().into()
            } else if let Some(bytes) = images.get(&name) {
                data_uri(&name, bytes).into()
            } else {
                dest_url
            };
            Some(Event::Start(Tag::Image {
                link_type,
                dest_url: src,
                title,
                id,
            }))
        }
        other => Some(other),
    });
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, events);
    LI_TASK_RE
        .replace_all(&body, |caps: &regex::Captures| {
            // Bear grays a ticked task out; the class is what carries that.
            let done = if caps[2].contains("checked") {
                " done"
            } else {
                ""
            };
            format!(
                "<li class=\"task{done}\">{}{}",
                caps.get(1).map(|m| m.as_str()).unwrap_or(""),
                &caps[2]
            )
        })
        .into_owned()
}

/// The page's Content-Security-Policy. The note's own HTML reaches the page
/// as written — Bear keeps it and so does the export — so the page itself has
/// to refuse to run it: no script, no fetch, nothing loaded from anywhere.
/// Images are `data:` because attachments are embedded; styles are inline
/// because the sheet is. The checkbox tick is a `data:` SVG in the sheet,
/// which `img-src` covers. `base-uri` and `form-action` do not fall back to
/// `default-src`, so they are closed by name.
pub const CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
                       base-uri 'none'; form-action 'none'";

/// A complete, self-contained HTML document. The policy comes first in the
/// head: a meta policy only governs what the parser meets after it.
pub fn render(
    content: &str,
    title: &str,
    images: &HashMap<String, Vec<u8>>,
    image_src: &HashMap<String, String>,
    theme: &Theme,
) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>\n{}\n</style>\n</head>\n<body>\n{}</body>\n</html>\n",
        escape(title),
        stylesheet(theme),
        render_body(content, images, image_src)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::to_text;
    use crate::ui::theme;

    const NOTE: &str = "# Greenhouse\n#garden/greenhouse #multi word#\n\n\
## Frame\nThe ==ridge== beam is a single ~12'~ piece, *braced*.\n\n\
- [ ] hang the door\n- [x] level the footings\n  - [ ] nested\n\n\
| Panel | Qty |\n|---|---|\n| Roof | 6 |\n\n\
> a quote with `code`\n\n```\n- [ ] not a task ==nor a highlight==\n```\n\n\
See [REV](https://www.revrobotics.com) and ![the frame](Front%20bed.png).\n";

    #[test]
    fn prepare_turns_bear_marks_into_inline_html_outside_fences() {
        let out = prepare(NOTE);
        assert!(out.contains("<p class=\"tags\"><span class=\"tag\">#garden/greenhouse</span><span class=\"tag\">#multi word#</span></p>"));
        assert!(out.contains("<mark class=\"default\">ridge</mark>") && out.contains("<u>12'</u>"));
        assert!(out.contains("- <input type=\"checkbox\" disabled> hang the door"));
        assert!(out.contains("- <input type=\"checkbox\" disabled checked> level the footings"));
        assert!(
            out.contains("- [ ] not a task ==nor a highlight=="),
            "fenced code is left alone"
        );
    }

    #[test]
    fn render_body_is_gfm_with_tasks_tables_and_links() {
        let body = render_body(NOTE, &HashMap::new(), &HashMap::new());
        assert!(
            body.contains("<h1>Greenhouse</h1>") && body.contains("<h2>Frame</h2>"),
            "{body}"
        );
        assert!(
            body.contains("<li class=\"task\"><input type=\"checkbox\" disabled> hang the door"),
            "{body}"
        );
        assert!(
            body.contains("<table>") && body.contains("<td>Roof</td>"),
            "{body}"
        );
        assert!(body.contains("<blockquote>") && body.contains("<code>code</code>"));
        assert!(body.contains("<a href=\"https://www.revrobotics.com\">REV</a>"));
        assert!(
            body.contains("<img src=\"Front%20bed.png\" alt=\"the frame\" />"),
            "no bytes given: the link stays\n{body}"
        );
        assert!(body.contains("<em>braced</em>"));
    }

    #[test]
    fn render_embeds_attachments_and_is_a_full_document() {
        let images = HashMap::from([("Front bed.png".to_string(), b"\x89PNG\r\n".to_vec())]);
        let doc = render(
            NOTE,
            "Greenhouse",
            &images,
            &HashMap::new(),
            theme::current(),
        );
        assert!(doc.starts_with("<!DOCTYPE html>") && doc.trim_end().ends_with("</html>"));
        assert!(doc.contains("<title>Greenhouse</title>") && doc.contains("<style>"));
        assert!(
            doc.contains("src=\"data:image/png;base64,iVBORw0K\""),
            "{doc}"
        );
        assert!(doc.contains("alt=\"the frame\""));
        let paths = HashMap::from([(
            "Front bed.png".to_string(),
            "assets/Front bed.png".to_string(),
        )]);
        let body = render_body("![](Front%20bed.png)", &HashMap::new(), &paths);
        // pulldown-cmark percent-encodes the URL, where markdown-it passed it through raw.
        assert!(
            body.contains("<img src=\"assets/Front%20bed.png\" alt=\"\" />"),
            "{body}"
        );
        assert!(
            render(
                "# x",
                "A <b> & c",
                &HashMap::new(),
                &HashMap::new(),
                theme::current()
            )
            .contains("<title>A &lt;b&gt; &amp; c</title>")
        );
    }

    #[test]
    fn the_page_policy_is_the_first_thing_in_the_head() {
        let doc = render(
            "<script>alert(1)</script>\n\n<img src=x onerror=\"alert(1)\">\n",
            "<script>",
            &HashMap::new(),
            &HashMap::new(),
            theme::current(),
        );
        let head = doc.split("<head>\n").nth(1).expect("a head");
        assert!(
            head.starts_with(&format!(
                "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
            )),
            "{doc}"
        );
        // Nothing may run or load: the note's own HTML is still in the page.
        for directive in [
            "default-src 'none'",
            "img-src data:",
            "style-src 'unsafe-inline'",
            "base-uri 'none'",
            "form-action 'none'",
        ] {
            assert!(CSP.contains(directive), "{directive}");
        }
        assert!(!CSP.contains("script-src") && !CSP.contains('"'));
        // The sheet loads nothing the policy would refuse.
        let sheet = stylesheet(theme::current());
        assert!(!sheet.contains("@import") && !sheet.contains("@font-face"));
        for url in sheet.split("url(").skip(1) {
            assert!(url.trim_start_matches('"').starts_with("data:"), "{url}");
        }
        assert!(doc.contains("<title>&lt;script&gt;</title>"));
    }

    #[test]
    fn a_link_that_would_run_a_script_keeps_only_its_text() {
        for target in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "java&#9;script:alert(1)",
            "javascript&colon;alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html,<script>alert(1)</script>",
        ] {
            let body = render_body(
                &format!("Click [here]({target}) now."),
                &HashMap::new(),
                &HashMap::new(),
            );
            assert_eq!(body, "<p>Click here now.</p>\n", "{target}");
        }
        let body = render_body("<javascript:alert(1)>", &HashMap::new(), &HashMap::new());
        assert!(!body.contains("href"), "{body}");
        // An ordinary link, and the link after a dropped one, are untouched.
        let body = render_body(
            "[a](javascript:x) [b](https://example.com) [c](mailto:x@example.com) [d](#top)",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            body,
            "<p>a <a href=\"https://example.com\">b</a> \
             <a href=\"mailto:x@example.com\">c</a> <a href=\"#top\">d</a></p>\n"
        );
        // A note may still say the word.
        assert!(!is_script_url("notes/javascript") && !is_script_url("javascript"));
    }

    #[test]
    fn a_heading_or_list_under_the_tag_line_survives() {
        let body = render_body(
            "# Klara\n#log/klara\n### TODO\n* one\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(body.contains("<h3>TODO</h3>"), "{body}");
        assert!(body.contains("<li>one</li>"), "{body}");
    }

    #[test]
    fn a_highlight_takes_its_color_from_the_circle_bear_writes() {
        let body = render_body(
            "==🟢green== ==🔵blue== ==plain==\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            body.contains("<mark class=\"green\">green</mark>"),
            "{body}"
        );
        assert!(body.contains("<mark class=\"blue\">blue</mark>"), "{body}");
        assert!(
            body.contains("<mark class=\"default\">plain</mark>"),
            "no circle, Bear's own color\n{body}"
        );
        // The trailing space is Bear's own rule: it makes the run no highlight
        // at all, circle and all.
        let loose = render_body("==🔴red ==\n", &HashMap::new(), &HashMap::new());
        assert!(!loose.contains("<mark"), "{loose}");
    }

    #[test]
    fn a_callout_becomes_a_panel_with_its_markdown_intact() {
        let body = render_body(
            "> [!NOTE] Mind the **gap**\n> and the second line\n\nafter\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(body.contains("<div class=\"callout note\">"), "{body}");
        assert!(
            body.contains("<strong>gap</strong>"),
            "the panel holds markdown, not raw text\n{body}"
        );
        assert!(body.contains("and the second line"), "{body}");
        assert!(body.contains("</div>"), "{body}");
        assert!(
            body.contains("<p>after</p>"),
            "the panel closes at the first line that is not a quote\n{body}"
        );
        for kind in ["tip", "important", "warning", "caution"] {
            let marker = kind.to_uppercase();
            let body = render_body(
                &format!("> [!{marker}] hi\n"),
                &HashMap::new(),
                &HashMap::new(),
            );
            assert!(
                body.contains(&format!("class=\"callout {kind}\"")),
                "{body}"
            );
        }
        // An ordinary quote is still an ordinary quote.
        let quote = render_body("> just a quote\n", &HashMap::new(), &HashMap::new());
        assert!(
            quote.contains("<blockquote>") && !quote.contains("callout"),
            "{quote}"
        );
        // And one Bear does not know stays as it was written.
        let unknown = render_body("> [!SIDEBAR] hm\n", &HashMap::new(), &HashMap::new());
        assert!(
            unknown.contains("<blockquote>") && unknown.contains("[!SIDEBAR]"),
            "{unknown}"
        );
    }

    #[test]
    fn a_note_link_loses_its_brackets_and_keeps_its_target() {
        let body = render_body(
            "See [[Feb 19, 2021 (Friday)/Meeting]] today\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            body.contains("<span class=\"note-link\">Feb 19, 2021 (Friday)/Meeting</span>"),
            "{body}"
        );
        let fenced = prepare("```\n[[not a link]]\n```\n");
        assert!(fenced.contains("[[not a link]]"), "{fenced}");
    }

    #[test]
    fn a_task_keeps_its_box_and_loses_its_bullet_loose_or_tight() {
        let tight = render_body("- [ ] one\n- [x] two\n", &HashMap::new(), &HashMap::new());
        let loose = render_body("- [ ] one\n\n- [x] two\n", &HashMap::new(), &HashMap::new());
        for (body, shape) in [(tight, "tight"), (loose, "loose")] {
            // A loose list wraps each item in a paragraph; the marker is
            // hidden by the class, so it has to survive that wrapping.
            assert_eq!(
                body.matches("<li class=\"task").count(),
                2,
                "{shape}\n{body}"
            );
            assert_eq!(
                body.matches("<li class=\"task done\">").count(),
                1,
                "a ticked task is grayed out by its class\n{shape}\n{body}"
            );
            assert_eq!(
                body.matches("<input type=\"checkbox\"").count(),
                2,
                "{shape}\n{body}"
            );
            assert!(!body.contains("<li>\n<p><input"), "{shape}\n{body}");
        }
    }

    #[test]
    fn a_fence_closes_a_callout_instead_of_falling_into_it() {
        let body = render_body(
            "> [!NOTE] hi\n```\ncode\n```\nafter\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        let panel = body.find("<div class=\"callout note\">").expect("a panel");
        let close = body.find("</div>").expect("a close");
        let code = body.find("<pre>").expect("a code block");
        assert!(close < code, "the code block is not in the panel\n{body}");
        assert!(panel < close, "{body}");
        assert_eq!(body.matches("</div>").count(), 1, "{body}");
        // An unterminated fence leaves nothing of the panel inside the code.
        let unterminated = render_body(
            "> [!NOTE] hi\n```\ncode\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            !unterminated.contains("&lt;/div&gt;"),
            "the close is markup, not text\n{unterminated}"
        );
        assert_eq!(unterminated.matches("</div>").count(), 1, "{unterminated}");
    }

    #[test]
    fn a_callout_title_is_its_own_paragraph() {
        let body = render_body(
            "> [!NOTE] Title\n> body line\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        // `.callout > p:first-child` is bold: the body must not join the title.
        assert!(body.contains("<p>Title</p>"), "{body}");
        assert!(body.contains("<p>body line</p>"), "{body}");
    }

    #[test]
    fn an_indented_callout_stays_a_quote_inside_its_list() {
        let body = render_body(
            "- item\n  > [!NOTE] inner\n- next\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            !body.contains("callout"),
            "a panel would split the list\n{body}"
        );
        assert_eq!(body.matches("<ul>").count(), 1, "{body}");
        assert_eq!(body.matches("<li>").count(), 2, "{body}");
    }

    #[test]
    fn a_tag_line_inside_a_callout_still_becomes_pills() {
        let body = render_body(
            "> [!TIP] hi\n> #garden/beds\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            body.contains("<span class=\"tag\">#garden/beds</span>"),
            "{body}"
        );
    }

    #[test]
    fn inline_code_keeps_bears_marks_as_characters() {
        let body = render_body(
            "`[[x]]` and `==🟢y==` and `~z~` and [[real]]\n",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(body.contains("<code>[[x]]</code>"), "{body}");
        assert!(body.contains("<code>==🟢y==</code>"), "{body}");
        assert!(body.contains("<code>~z~</code>"), "{body}");
        assert!(
            body.contains("<span class=\"note-link\">real</span>"),
            "outside the backticks it is still a note link\n{body}"
        );
        // A backtick with no partner opens no code span.
        let lone = render_body("a ` [[x]]\n", &HashMap::new(), &HashMap::new());
        assert!(lone.contains("note-link"), "{lone}");
    }

    #[test]
    fn a_bracketed_link_text_is_left_to_markdown() {
        let body = render_body("[[x]](https://y.test)\n", &HashMap::new(), &HashMap::new());
        assert!(
            body.contains("<a href=\"https://y.test\">"),
            "`[[x]](y)` is a link, not a note link\n{body}"
        );
        assert!(!body.contains("note-link"), "{body}");
    }

    #[test]
    fn a_highlight_of_nothing_but_a_circle_keeps_the_circle() {
        let body = render_body("==🔴== ==🔴️red==\n", &HashMap::new(), &HashMap::new());
        assert!(
            body.contains("<mark class=\"default\">🔴</mark>"),
            "an empty highlight would lose the character\n{body}"
        );
        assert!(
            body.contains("<mark class=\"red\">red</mark>"),
            "the emoji presentation selector is part of the circle\n{body}"
        );
    }

    #[test]
    fn the_dark_rules_come_after_what_they_override() {
        let css = stylesheet(&theme::RED_GRAPHITE_DARK);
        // A media query adds no specificity, so a dark rule above the base
        // rule it means to override simply never applies.
        let dark = css.find("prefers-color-scheme").expect("a dark block");
        for base in ["body {", "pre, code {", "th {", ".tag {"] {
            assert!(
                css.find(base).expect(base) < dark,
                "{base} must come before the dark block\n{css}"
            );
        }
        assert!(css.find("@media print").expect("print") > dark, "{css}");
        // The highlight palette is Bear's on screen and on paper alike.
        let print = css.split("@media print").nth(1).expect("a print block");
        assert!(!print.contains("mark {"), "{print}");
    }

    #[test]
    fn a_page_printed_on_a_dark_machine_has_no_dark_ground_left() {
        // The dark block applies to print media too, so each ground it darkens
        // has to be set back in the print block, or a PDF made on a machine in
        // dark mode prints a charcoal header row under near-black text.
        let css = stylesheet(&theme::RED_GRAPHITE_DARK);
        let print = css.split("@media print").nth(1).expect("a print block");
        for rule in ["body {", "pre, code {", "th {", ".tag {"] {
            let set = print
                .split(rule)
                .nth(1)
                .and_then(|rest| rest.split('}').next())
                .unwrap_or_else(|| panic!("{rule} is not reset for print\n{print}"));
            assert!(set.contains("background: #"), "{rule}{set}");
        }
    }

    #[test]
    fn the_stylesheet_wears_the_theme_colors_legibly_on_either_page() {
        let css = stylesheet(&theme::RED_GRAPHITE_DARK);
        assert!(css.contains("a { color: var(--accent);"), "{css}");
        assert!(
            css.contains("li::marker { color: var(--bullet); }"),
            "{css}"
        );
        // The link and the list markers are two colors in the app; they stay
        // two here, and the page they land on decides how dark each one is.
        let (light, dark, print) = (declared(&css, 0), declared(&css, 1), declared(&css, 2));
        assert_ne!(light.0, light.1, "link and bullet\n{css}");
        assert_ne!(light, dark, "a dark page wants lighter ink\n{css}");
        assert_eq!(light, print, "paper is the light page\n{css}");
        // The print block has to come last: it and the dark-mode block declare
        // the same variables at the same specificity, so order is all that
        // keeps a printed note light on a machine set to dark mode.
        assert!(
            css.find("@media print") > css.find("prefers-color-scheme"),
            "{css}"
        );
        for theme in theme::THEMES {
            let css = stylesheet(theme);
            for (declared, page) in [
                (declared(&css, 0), LIGHT_PAGE),
                (declared(&css, 1), DARK_PAGE),
                (declared(&css, 2), LIGHT_PAGE),
            ] {
                for color in [declared.0, declared.1] {
                    assert!(
                        contrast(color, page) >= 4.5,
                        "{}: {color:?} on {page:?}",
                        theme.name
                    );
                }
            }
        }
    }

    /// The `--accent` and `--bullet` the `n`th `:root` declares: light, then
    /// the `prefers-color-scheme` one, then print.
    fn declared(css: &str, n: usize) -> (Rgb, Rgb) {
        let roots: Vec<&str> = css
            .match_indices("--accent: #")
            .map(|(at, _)| &css[at..])
            .collect();
        assert_eq!(roots.len(), 3, "light, dark and print\n{css}");
        let root = roots[n];
        let parse = |at: usize| {
            let hex = &root[at..][..6];
            (
                u8::from_str_radix(&hex[0..2], 16).unwrap(),
                u8::from_str_radix(&hex[2..4], 16).unwrap(),
                u8::from_str_radix(&hex[4..6], 16).unwrap(),
            )
        };
        let bullet = root.find("--bullet: #").expect("a bullet beside it") + "--bullet: #".len();
        (parse("--accent: #".len()), parse(bullet))
    }

    #[test]
    fn the_stylesheet_carries_a_print_page() {
        let css = stylesheet(&theme::RED_GRAPHITE);
        assert!(
            css.contains("@page { size: A4; margin: 18mm 16mm; }"),
            "{css}"
        );
        let print = css.split("@media print").nth(1).expect("a print block");
        assert!(
            print.contains("background: #fff"),
            "paper is white\n{print}"
        );
        assert!(print.contains("break-after: avoid"), "{print}");
        let doc = render(
            "# x",
            "x",
            &HashMap::new(),
            &HashMap::new(),
            &theme::RED_GRAPHITE,
        );
        assert!(doc.contains("@media print"), "the export carries it");
    }

    #[test]
    fn to_text_strips_markup_and_keeps_structure() {
        let text = to_text(NOTE);
        assert!(
            text.starts_with("Greenhouse\n#garden/greenhouse #multi word#\n\nFrame\n"),
            "{text}"
        );
        assert!(text.contains("The ridge beam is a single 12' piece, braced."));
        assert!(text.contains("- ☐ hang the door\n- ☑ level the footings\n  - ☐ nested\n"));
        assert!(text.contains("| Panel | Qty |\n|---|---|\n| Roof | 6 |"));
        assert!(text.contains("a quote with code"));
        assert!(text.contains("- [ ] not a task ==nor a highlight=="));
        assert!(text.contains("See REV <https://www.revrobotics.com> and [image: Front bed.png]."));
        assert!(text.ends_with('\n') && !text.contains("```"));
        assert_eq!(
            to_text("* star\n+ plus\n- dash\n1. one"),
            "- star\n- plus\n- dash\n1. one\n"
        );
        assert_eq!(to_text("[https://x.y](https://x.y)"), "https://x.y\n");
    }
}
