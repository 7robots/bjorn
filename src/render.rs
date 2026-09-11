//! Bear markdown as text: tag lines, task boxes, plain-text and preview forms.
//!
//! The reader's styled rendering lives in `ui`; this module holds the pure
//! text transformations shared by the notes list, the exports and the fakes.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;

pub const OPEN_BOX: &str = "☐";
pub const DONE_BOX: &str = "☑";

/// A Bear tag token: `#word`, `#nested/child`, `#multi word#`. Tags never carry
/// a space after the `#`, which is what separates them from headings.
pub const TAG_TOKEN_PATTERN: &str = r"#[^\s#][^#\n]*?#(?=\s|$)|#[^\s#]+";

pub static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(```|~~~)").unwrap());
pub static TAG_TOKEN_RE: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(TAG_TOKEN_PATTERN).unwrap());
static TAG_LINE_RE: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(&format!(
        r"^\s*(?:{TAG_TOKEN_PATTERN})(?:\s+(?:{TAG_TOKEN_PATTERN}))*\s*$"
    ))
    .unwrap()
});
pub static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s+\S").unwrap());
pub static TASK_OPEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:[-*+]|\d+[.)])\s+)\[ \](\s+|$)").unwrap());
pub static TASK_DONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:[-*+]|\d+[.)])\s+)\[[xX]\](\s+|$)").unwrap());
pub static HIGHLIGHT_RE: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"==(?=\S)(.+?)(?<=\S)==").unwrap());
pub static UNDERLINE_RE: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"(?<![~\w])~(?=\S)([^~\n]+?)(?<=\S)~(?![~\w])").unwrap());

static INLINE_MARKUP_RE: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"\*|__|==|`|~~|(?<![~\w])~(?=\S)|(?<=\S)~(?![~\w])").unwrap()
});
static BLOCK_MARKUP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)\[([ xX])\]|^\s*(?:[-*+]|\d+[.)])\s+|^#{1,6}\s+|^>\s*").unwrap()
});
static IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)\s]+)\)").unwrap());
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap());
static HEADING_MARK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})\s+").unwrap());
static QUOTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)>\s?").unwrap());
static BULLET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)[*+](\s+)").unwrap());

/// A line made only of Bear tags, such as the one under the title.
pub fn is_tag_line(line: &str) -> bool {
    !line.trim().is_empty()
        && !HEADING_RE.is_match(line)
        && TAG_LINE_RE.is_match(line).unwrap_or(false)
}

pub fn tags_in_line(line: &str) -> Vec<String> {
    TAG_TOKEN_RE
        .find_iter(line)
        .filter_map(Result::ok)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Is this line a fence opener/closer (``` or ~~~)?
pub fn is_fence(line: &str) -> bool {
    FENCE_RE.is_match(line)
}

/// Body text without the H1 and the tag line under it.
pub fn strip_title_and_tags(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    if i < lines.len() && lines[i].starts_with("# ") {
        i += 1;
    }
    while i < lines.len() && (lines[i].trim().is_empty() || is_tag_line(lines[i])) {
        i += 1;
    }
    lines[i..].join("\n")
}

pub fn percent_decode(name: &str) -> String {
    percent_encoding::percent_decode_str(name)
        .decode_utf8_lossy()
        .into_owned()
}

/// Emphasis, highlight, code, strike and underline markers removed; links
/// kept as `text <url>`, images as `[image: file]`.
pub fn strip_inline_markup(line: &str) -> String {
    let line = IMAGE_RE.replace_all(line, |caps: &regex::Captures| {
        format!("[image: {}]", percent_decode(&caps[2]))
    });
    let line = LINK_RE.replace_all(&line, |caps: &regex::Captures| {
        if caps[2] == caps[1] {
            caps[1].to_string()
        } else {
            format!("{} <{}>", &caps[1], &caps[2])
        }
    });
    INLINE_MARKUP_RE.replace_all(&line, "").into_owned()
}

fn task_boxes(line: &str) -> String {
    let line = TASK_OPEN_RE.replace(line, |caps: &regex::Captures| {
        format!(
            "{}{}{}",
            &caps[1],
            OPEN_BOX,
            caps.get(2)
                .map(|m| m.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(" ")
        )
    });
    TASK_DONE_RE
        .replace(&line, |caps: &regex::Captures| {
            format!(
                "{}{}{}",
                &caps[1],
                DONE_BOX,
                caps.get(2)
                    .map(|m| m.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(" ")
            )
        })
        .into_owned()
}

/// The note as plain text: headings without their `#`s, task boxes as glyphs,
/// `- ` bullets kept, quotes unmarked, inline markers gone, fenced code and
/// tables as written, the tag line kept.
pub fn to_text(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in content.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || is_tag_line(line) || line.trim_start().starts_with('|') {
            out.push(line.to_string());
            continue;
        }
        let line = HEADING_MARK_RE.replace(line, "");
        let line = QUOTE_RE.replace(&line, "$1");
        let line = task_boxes(&line);
        let line = BULLET_RE.replace(&line, "$1-$2");
        out.push(strip_inline_markup(&line).trim_end().to_string());
    }
    let mut text = out.join("\n");
    text = text.trim_matches('\n').to_string();
    text.push('\n');
    text
}

/// Body text flattened to one line for the notes list, the way Bear previews a
/// note under its title: no H1, no tag line, no markdown markers.
pub fn preview(content: &str, limit: usize) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut length = 0;
    for line in strip_title_and_tags(content).lines() {
        if is_fence(line) {
            continue;
        }
        let stripped = BLOCK_MARKUP_RE.replace_all(line, "");
        let text = strip_inline_markup(&stripped)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        length += text.chars().count() + 1;
        words.push(text);
        if length >= limit {
            break;
        }
    }
    words.join(" ").chars().take(limit).collect()
}

pub const PREVIEW_LIMIT: usize = 240;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_line_is_recognised() {
        assert!(is_tag_line("#robotics/Coding"));
        assert!(is_tag_line("#a #b/c #multi word#"));
        assert!(!is_tag_line("# Heading"));
        assert!(!is_tag_line("text with #tag inside"));
        assert_eq!(
            tags_in_line("#a #b/c #multi word#"),
            vec!["#a", "#b/c", "#multi word#"]
        );
    }

    #[test]
    fn strip_title_and_tags_leaves_the_body() {
        let body = "# Title\n#tag\n\n## Section\nFirst real line\nmore";
        assert!(strip_title_and_tags(body).starts_with("## Section"));
    }

    #[test]
    fn preview_flattens_body_without_title_tags_or_markers() {
        let body = "# Title\n#work/sprint #home\n\n## Frame\n- [ ] Brace the *north* wall\n> quoted `code`\n```\nfenced\n```\nend";
        assert_eq!(
            preview(body, PREVIEW_LIMIT),
            "Frame Brace the north wall quoted code fenced end"
        );
        assert_eq!(preview("# Only title\n#tag\n", PREVIEW_LIMIT), "");
        assert_eq!(preview(&"word ".repeat(100), 40).chars().count(), 40);
    }

    #[test]
    fn inline_markup_is_stripped_and_links_kept() {
        assert_eq!(
            strip_inline_markup("a *b* ==c== `d` ~~e~~ ~f~"),
            "a b c d e f"
        );
        assert_eq!(strip_inline_markup("path ~/foo/bar"), "path /foo/bar"); // as Python: a lone opening ~ is dropped
        assert_eq!(
            strip_inline_markup("[Bear](https://bear.app)"),
            "Bear <https://bear.app>"
        );
        assert_eq!(
            strip_inline_markup("![](Front%20bed.png)"),
            "[image: Front bed.png]"
        );
    }

    #[test]
    fn to_text_unmarks_structure() {
        let src = "# Title\n#tag\n\n## Section\n> quote\n- [ ] task\n* bullet\n```\ncode **kept**\n```\n| a | b |\n";
        let out = to_text(src);
        assert_eq!(
            out,
            format!(
                "Title\n#tag\n\nSection\nquote\n- {OPEN_BOX} task\n- bullet\ncode **kept**\n| a | b |\n"
            )
        );
    }
}
