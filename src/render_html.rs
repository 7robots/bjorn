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
use pulldown_cmark::{Event, Options, Parser, Tag};
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
@media (prefers-color-scheme: dark) {{ :root {{ --accent: {on_dark}; --bullet: {bullet_dark}; }} body {{ color: #ddd; background: #1e1e1e; }} mark {{ background: #6b5b00; color: inherit; }} pre, code {{ background: #2a2a2a; }} th, td {{ border-color: #444; }} blockquote {{ border-color: #444; color: #aaa; }} .tag {{ background: #3a3a3a; color: #ccc; }} }}
h1, h2, h3, h4 {{ line-height: 1.25; margin: 1.4em 0 0.5em; }}
h1 {{ font-size: 1.8em; margin-top: 0; }}
a {{ color: var(--accent); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
mark {{ background: #fde68a; padding: 0 0.15em; border-radius: 2px; }}
pre, code {{ font: 0.92em/1.45 ui-monospace, "SF Mono", Menlo, monospace; background: #f4f4f4; border-radius: 4px; }}
code {{ padding: 0.1em 0.3em; }}
pre {{ padding: 0.8em 1em; overflow-x: auto; }}
pre code {{ padding: 0; background: none; }}
blockquote {{ margin: 1em 0; padding: 0 1em; border-left: 3px solid #ddd; color: #666; }}
table {{ border-collapse: collapse; margin: 1em 0; }}
th, td {{ border: 1px solid #ddd; padding: 0.35em 0.7em; text-align: left; }}
img {{ max-width: 100%; height: auto; }}
ul, ol {{ padding-left: 1.5em; }}
li {{ margin: 0.15em 0; }}
li::marker {{ color: var(--bullet); }}
li.task::marker {{ color: transparent; }}
/* display: some converters lay a bare checkbox out as a block, which drops the task text onto its own line. */
input[type=checkbox] {{ display: inline-block; margin: 0 0.4em 0 0; vertical-align: -0.1em; }}
.tags {{ margin: -0.5em 0 1.5em; }}
.tag {{ display: inline-block; background: #eee; color: #555; border-radius: 1em; padding: 0.05em 0.7em; margin-right: 0.3em; font-size: 0.85em; }}
hr {{ border: 0; border-top: 1px solid #ddd; margin: 2em 0; }}
@page {{ size: A4; margin: 18mm 16mm; }}
@media print {{
  :root {{ --accent: {on_light}; --bullet: {bullet_light}; }}
  body {{ max-width: none; margin: 0; padding: 0; font-size: 11pt; color: #1a1a1a; background: #fff; }}
  h1 {{ font-size: 2em; }}
  h1, h2, h3, h4, h5, h6 {{ color: #111; break-after: avoid; }}
  mark {{ background: #fde68a; color: #1a1a1a; }}
  pre, code {{ background: #f5f5f5; color: #1a1a1a; }}
  pre {{ white-space: pre-wrap; }}
  blockquote {{ border-color: #e0e0e0; color: #555; }}
  th, td {{ border-color: #e0e0e0; }}
  .tag {{ background: #ececec; color: #6b6b6b; }}
  /* A quote or a code block longer than the page has to be allowed to split. */
  li, tr, img {{ break-inside: avoid; }}
  /* The pills, the highlights and the code ground are the note, not decoration. */
  mark, .tag, pre, code {{ print-color-adjust: exact; -webkit-print-color-adjust: exact; }}
}}"#
    )
}

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

/// Bear markdown -> markdown with inline HTML for Bear's own marks.
pub fn prepare(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in content.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }
        if is_tag_line(line) {
            let spans: String = tags_in_line(line)
                .iter()
                .map(|t| format!("<span class=\"tag\">{}</span>", escape(t)))
                .collect();
            out.push(format!("<p class=\"tags\">{spans}</p>"));
            // An HTML block runs to the next blank line: without one, a
            // heading or list right under the tag line is swallowed into it.
            out.push(String::new());
            continue;
        }
        let line = task_inputs(line);
        let line = HIGHLIGHT_RE.replace_all(&line, "<mark>$1</mark>");
        let line = UNDERLINE_RE.replace_all(&line, "<u>$1</u>");
        out.push(line.into_owned());
    }
    format!("{}\n", out.join("\n"))
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

/// The note as an HTML fragment. `images` maps attachment filenames to bytes
/// for embedding; `image_src` maps them to URLs to reference instead.
pub fn render_body(
    content: &str,
    images: &HashMap<String, Vec<u8>>,
    image_src: &HashMap<String, String>,
) -> String {
    let prepared = prepare(content);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let events = Parser::new_ext(&prepared, options).map(|event| match event {
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
            Event::Start(Tag::Image {
                link_type,
                dest_url: src,
                title,
                id,
            })
        }
        other => other,
    });
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, events);
    LI_TASK_RE
        .replace_all(&body, "<li class=\"task\">$1$2")
        .into_owned()
}

/// A complete, self-contained HTML document.
pub fn render(
    content: &str,
    title: &str,
    images: &HashMap<String, Vec<u8>>,
    image_src: &HashMap<String, String>,
    theme: &Theme,
) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
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
        assert!(out.contains("<mark>ridge</mark>") && out.contains("<u>12'</u>"));
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
    fn a_task_keeps_its_box_and_loses_its_bullet_loose_or_tight() {
        let tight = render_body("- [ ] one\n- [x] two\n", &HashMap::new(), &HashMap::new());
        let loose = render_body("- [ ] one\n\n- [x] two\n", &HashMap::new(), &HashMap::new());
        for (body, shape) in [(tight, "tight"), (loose, "loose")] {
            // A loose list wraps each item in a paragraph; the marker is
            // hidden by the class, so it has to survive that wrapping.
            assert_eq!(
                body.matches("<li class=\"task\">").count(),
                2,
                "{shape}\n{body}"
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
