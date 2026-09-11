//! Bear markdown -> styled terminal lines.
//!
//! The Python Bjorn handed markdown to Textual's `Markdown` widget, which
//! mounts one widget per block and so had to render long notes in two halves.
//! Here `pulldown-cmark` events become plain `ratatui` lines that carry the
//! index of the block they belong to, so the whole note is rendered once,
//! scrolling is free, and search can count and jump by block as before.

use std::sync::LazyLock;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use crate::render::{
    DONE_BOX, HIGHLIGHT_RE, OPEN_BOX, UNDERLINE_RE, is_fence, is_tag_line, tags_in_line,
};
use crate::ui::theme;

/// One rendered line: its spans, the block it belongs to, and the prefix a
/// wrapped continuation of it starts with (list indent, quote bar).
#[derive(Debug, Clone, PartialEq)]
pub struct RLine {
    pub spans: Vec<Span<'static>>,
    pub block: usize,
    pub cont: Vec<Span<'static>>,
}

impl RLine {
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.plain().trim().is_empty()
    }
}

static MARK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^</?(mark|u|tags)>$").unwrap());

/// Bear-only syntax rewritten into inline HTML the renderer understands, with
/// fenced code left alone. Tag lines become `<tags>…</tags>` rows.
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
            out.push(format!("<tags>{}</tags>", tags_in_line(line).join(" ")));
            continue;
        }
        let line = HIGHLIGHT_RE.replace_all(line, "<mark>$1</mark>");
        let line = UNDERLINE_RE.replace_all(&line, "<u>$1</u>");
        out.push(line.into_owned());
    }
    out.join("\n")
}

struct ListLevel {
    next: Option<u64>,
    depth: usize,
}

struct Renderer {
    lines: Vec<RLine>,
    block: usize,
    spans: Vec<Span<'static>>,
    cont: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListLevel>,
    quote_depth: usize,
    in_code: bool,
    in_image: bool,
    image_alt: String,
    item_open: bool,
    table: Option<TableState>,
}

struct TableState {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    header_rows: usize,
    in_head: bool,
    cell: Vec<Span<'static>>,
    row: Vec<Vec<Span<'static>>>,
}

fn code_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn tag_style() -> Style {
    Style::default().fg(theme::ACCENT)
}

impl Renderer {
    fn new() -> Renderer {
        Renderer {
            lines: Vec::new(),
            block: 0,
            spans: Vec::new(),
            cont: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            in_code: false,
            in_image: false,
            image_alt: String::new(),
            item_open: false,
            table: None,
        }
    }

    fn style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::default(), |acc, s| acc.patch(*s))
    }

    fn push(&mut self, style: Style) {
        self.styles.push(style);
    }

    fn pop(&mut self) {
        self.styles.pop();
    }

    fn quote_prefix(&self) -> Vec<Span<'static>> {
        (0..self.quote_depth)
            .map(|_| Span::styled("▎ ", theme::muted()))
            .collect()
    }

    fn indent(&self) -> String {
        " ".repeat(self.lists.last().map(|l| l.depth * 2).unwrap_or(0))
    }

    /// Start a new block: bump the id and begin a fresh line with the quote bar.
    fn new_block(&mut self) {
        self.flush();
        self.block += 1;
        self.begin_line();
    }

    fn begin_line(&mut self) {
        self.spans = self.quote_prefix();
        if !self.indent().is_empty() && !self.item_open {
            let indent = self.indent();
            self.spans.push(Span::raw(indent));
        }
        self.cont = self.spans.clone();
    }

    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let cont = std::mem::take(&mut self.cont);
        self.lines.push(RLine {
            spans,
            block: self.block,
            cont,
        });
    }

    /// One blank line between blocks, never two, never at the top.
    fn blank(&mut self) {
        self.flush();
        if let Some(last) = self.lines.last()
            && !last.is_blank()
        {
            let prefix = self.quote_prefix();
            self.lines.push(RLine {
                spans: prefix,
                block: self.block,
                cont: Vec::new(),
            });
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_image {
            self.image_alt.push_str(text);
            return;
        }
        if self.table.is_some() {
            let style = self.style();
            if let Some(table) = self.table.as_mut() {
                table.cell.push(Span::styled(text.to_string(), style));
            }
            return;
        }
        if self.in_code {
            let style = self.style();
            let mut first = true;
            for piece in text.split('\n') {
                if !first {
                    self.flush();
                    self.begin_line();
                }
                first = false;
                if !piece.is_empty() || !first {
                    self.spans.push(Span::styled(format!("  {piece}"), style));
                }
            }
            return;
        }
        if self.spans.is_empty() {
            self.begin_line();
        }
        self.spans
            .push(Span::styled(text.to_string(), self.style()));
    }

    fn html(&mut self, raw: &str) {
        let tag = raw.trim();
        if !MARK_RE.is_match(tag) {
            if !tag.is_empty() {
                self.push(theme::muted());
                self.text(tag);
                self.pop();
            }
            return;
        }
        match tag {
            "<mark>" => self.push(Style::default().add_modifier(Modifier::REVERSED)),
            "<u>" => self.push(Style::default().add_modifier(Modifier::UNDERLINED)),
            "<tags>" => self.push(tag_style()),
            _ => self.pop(),
        }
    }

    fn heading_style(level: HeadingLevel) -> Style {
        match level {
            HeadingLevel::H1 => Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
            HeadingLevel::H2 => Style::default().add_modifier(Modifier::BOLD),
            _ => Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC),
        }
    }

    fn end_table(&mut self, table: TableState) {
        let cols = table
            .aligns
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                let w: usize = cell
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                widths[i] = widths[i].max(w);
            }
        }
        let prefix = self.quote_prefix();
        for (r, row) in table.rows.iter().enumerate() {
            let mut spans = prefix.clone();
            for (i, width) in widths.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                let w: usize = cell
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                let pad = width.saturating_sub(w);
                let align = table.aligns.get(i).copied().unwrap_or(Alignment::None);
                let (left, right) = match align {
                    Alignment::Right => (pad, 0),
                    Alignment::Center => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                if i > 0 {
                    spans.push(Span::styled(" │ ", theme::muted()));
                }
                spans.push(Span::raw(" ".repeat(left)));
                for mut s in cell {
                    if r < table.header_rows {
                        s.style = s.style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(s);
                }
                spans.push(Span::raw(" ".repeat(right)));
            }
            self.lines.push(RLine {
                spans,
                block: self.block,
                cont: Vec::new(),
            });
            if r + 1 == table.header_rows && table.header_rows > 0 {
                let mut spans = prefix.clone();
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                spans.push(Span::styled(rule.join("─┼─"), theme::muted()));
                self.lines.push(RLine {
                    spans,
                    block: self.block,
                    cont: Vec::new(),
                });
            }
        }
    }
}

/// Render Bear markdown into block-indexed lines.
pub fn render(content: &str) -> Vec<RLine> {
    let prepared = prepare(content);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer::new();
    for event in Parser::new_ext(&prepared, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if r.item_open {
                        r.item_open = false;
                    } else {
                        r.blank();
                        r.new_block();
                    }
                }
                Tag::Heading { level, .. } => {
                    r.blank();
                    r.new_block();
                    r.push(Renderer::heading_style(level));
                }
                Tag::BlockQuote(_) => {
                    r.blank();
                    r.quote_depth += 1;
                }
                Tag::CodeBlock(kind) => {
                    r.blank();
                    r.new_block();
                    r.in_code = true;
                    r.push(code_style());
                    if let CodeBlockKind::Fenced(lang) = kind
                        && !lang.is_empty()
                    {
                        r.spans
                            .push(Span::styled(format!("  {lang}"), theme::muted()));
                        r.flush();
                        r.begin_line();
                    }
                }
                Tag::List(start) => {
                    if r.lists.is_empty() {
                        r.blank();
                    } else {
                        r.flush();
                    }
                    let depth = r.lists.len();
                    r.lists.push(ListLevel { next: start, depth });
                }
                Tag::Item => {
                    r.flush();
                    r.block += 1;
                    let (marker, depth) = {
                        let level = r.lists.last_mut().expect("item inside a list");
                        let marker = match level.next {
                            Some(n) => {
                                level.next = Some(n + 1);
                                format!("{n}. ")
                            }
                            None => "• ".to_string(),
                        };
                        (marker, level.depth)
                    };
                    let indent = " ".repeat(depth * 2);
                    r.spans = r.quote_prefix();
                    r.spans.push(Span::raw(indent.clone()));
                    r.cont = r.spans.clone();
                    r.cont.push(Span::raw(
                        " ".repeat(UnicodeWidthStr::width(marker.as_str())),
                    ));
                    r.spans.push(Span::styled(marker, theme::muted()));
                    r.item_open = true;
                }
                Tag::Emphasis => r.push(Style::default().add_modifier(Modifier::ITALIC)),
                Tag::Strong => r.push(Style::default().add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => r.push(Style::default().add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { .. } => r.push(
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Tag::Image { .. } => {
                    r.in_image = true;
                    r.image_alt.clear();
                }
                Tag::Table(aligns) => {
                    r.blank();
                    r.new_block();
                    r.spans.clear();
                    r.table = Some(TableState {
                        aligns,
                        rows: Vec::new(),
                        header_rows: 0,
                        in_head: false,
                        cell: Vec::new(),
                        row: Vec::new(),
                    });
                }
                Tag::TableHead => {
                    if let Some(t) = r.table.as_mut() {
                        t.in_head = true;
                        t.row.clear();
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = r.table.as_mut() {
                        t.row.clear();
                    }
                }
                Tag::TableCell => {
                    if let Some(t) = r.table.as_mut() {
                        t.cell.clear();
                    }
                }
                Tag::HtmlBlock => {
                    r.blank();
                    r.new_block();
                    r.push(theme::muted());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    r.flush();
                    r.item_open = false;
                }
                TagEnd::Heading(_) => {
                    r.pop();
                    r.flush();
                    r.blank();
                }
                TagEnd::BlockQuote(_) => {
                    r.flush();
                    r.quote_depth = r.quote_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    r.pop();
                    r.in_code = false;
                    // Trailing newline in the code text left an empty continuation line.
                    if r.spans.iter().all(|s| s.content.trim().is_empty()) {
                        r.spans.clear();
                    }
                    r.flush();
                    r.blank();
                }
                TagEnd::List(_) => {
                    r.flush();
                    r.lists.pop();
                    r.item_open = false;
                    if r.lists.is_empty() {
                        r.blank();
                    }
                }
                TagEnd::Item => {
                    r.flush();
                    r.item_open = false;
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => r.pop(),
                TagEnd::Image => {
                    r.in_image = false;
                    let alt = std::mem::take(&mut r.image_alt);
                    r.push(theme::muted());
                    r.text(&format!("[image: {alt}]"));
                    r.pop();
                }
                TagEnd::Table => {
                    if let Some(table) = r.table.take() {
                        r.end_table(table);
                        r.spans.clear();
                        r.blank();
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = r.table.as_mut() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                        t.header_rows = t.rows.len();
                        t.in_head = false;
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = r.table.as_mut() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
                TagEnd::TableCell => {
                    if let Some(t) = r.table.as_mut() {
                        let cell = std::mem::take(&mut t.cell);
                        t.row.push(cell);
                    }
                }
                TagEnd::HtmlBlock => {
                    r.pop();
                    r.flush();
                    r.blank();
                }
                _ => {}
            },
            Event::Text(text) => {
                // pulldown-cmark leaves the image's own alt text and URL out of Text events;
                // an image link's file name comes through the alt or, failing that, stays blank.
                r.text(&text);
            }
            Event::Code(code) => {
                let style = r.style().patch(code_style());
                if let Some(t) = r.table.as_mut() {
                    t.cell.push(Span::styled(code.to_string(), style));
                } else {
                    if r.spans.is_empty() {
                        r.begin_line();
                    }
                    r.spans.push(Span::styled(code.to_string(), style));
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                for piece in html.split_inclusive('>') {
                    r.html(piece);
                }
            }
            Event::SoftBreak => r.text(" "),
            Event::HardBreak => {
                r.flush();
                r.begin_line();
            }
            Event::Rule => {
                r.blank();
                r.new_block();
                r.spans.push(Span::styled("─".repeat(40), theme::muted()));
                r.flush();
                r.blank();
            }
            Event::TaskListMarker(checked) => {
                let glyph = if checked { DONE_BOX } else { OPEN_BOX };
                r.spans.push(Span::raw(format!("{glyph} ")));
                if let Some(last) = r.cont.last_mut() {
                    last.content = format!("{}  ", last.content).into();
                }
            }
            _ => {}
        }
    }
    r.flush();
    while r.lines.last().is_some_and(RLine::is_blank) {
        r.lines.pop();
    }
    r.lines
}

/// The image file names a note links to, percent-decoded, in order.
pub fn image_name(dest: &str) -> String {
    crate::render::percent_decode(dest)
}

/// Break one line into rows no wider than `width`, at spaces where possible,
/// each continuation starting with the line's `cont` prefix.
pub fn wrap(line: &RLine, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let cont_width: usize = line
        .cont
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let mut chunks: Vec<(String, Style)> = Vec::new();
    for span in &line.spans {
        let mut word = String::new();
        let mut spaces = false;
        for ch in span.content.chars() {
            let is_space = ch == ' ';
            if !word.is_empty() && is_space != spaces {
                chunks.push((std::mem::take(&mut word), span.style));
            }
            spaces = is_space;
            word.push(ch);
        }
        if !word.is_empty() {
            chunks.push((word, span.style));
        }
    }
    let new_row =
        |rows: &mut Vec<Line<'static>>, current: &mut Vec<Span<'static>>, used: &mut usize| {
            // A row never ends in the spaces it broke at.
            while current.len() > line.cont.len()
                && current.last().is_some_and(|s| s.content.trim().is_empty())
            {
                current.pop();
            }
            if let Some(last) = current.last_mut() {
                last.content = last.content.trim_end().to_string().into();
            }
            rows.push(Line::from(std::mem::take(current)));
            *current = line.cont.clone();
            *used = cont_width.min(width.saturating_sub(1));
        };
    for (chunk, style) in chunks {
        let w = UnicodeWidthStr::width(chunk.as_str());
        if used + w <= width {
            current.push(Span::styled(chunk, style));
            used += w;
            continue;
        }
        if chunk.trim().is_empty() {
            // A run of spaces at the edge is where the row breaks.
            new_row(&mut rows, &mut current, &mut used);
            continue;
        }
        if used > cont_width || (used > 0 && rows.is_empty() && used > cont_width) {
            new_row(&mut rows, &mut current, &mut used);
        }
        if used + w <= width {
            current.push(Span::styled(chunk, style));
            used += w;
            continue;
        }
        // A single word wider than the row: hard split.
        let mut piece = String::new();
        let mut pw = 0usize;
        for ch in chunk.chars() {
            let cw = UnicodeWidthStr::width(ch.to_string().as_str());
            if used + pw + cw > width && pw > 0 {
                current.push(Span::styled(std::mem::take(&mut piece), style));
                new_row(&mut rows, &mut current, &mut used);
                pw = 0;
            }
            piece.push(ch);
            pw += cw;
        }
        if !piece.is_empty() {
            current.push(Span::styled(piece, style));
            used += pw;
        }
    }
    rows.push(Line::from(current));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[RLine]) -> Vec<String> {
        lines.iter().map(RLine::plain).collect()
    }

    #[test]
    fn tasks_become_glyphs_outside_fences() {
        let src = "- [ ] open\n- [x] done\n  - [ ] nested\n\n```\n- [ ] in code\n```\n\n1. [ ] numbered\n";
        let out = plain(&render(src));
        assert!(
            out.iter().any(|l| l == &format!("• {OPEN_BOX} open")),
            "{out:?}"
        );
        assert!(
            out.iter().any(|l| l == &format!("• {DONE_BOX} done")),
            "{out:?}"
        );
        assert!(
            out.iter().any(|l| l == &format!("  • {OPEN_BOX} nested")),
            "{out:?}"
        );
        assert!(out.iter().any(|l| l == "  - [ ] in code"), "{out:?}");
        assert!(
            out.iter().any(|l| l == &format!("1. {OPEN_BOX} numbered")),
            "{out:?}"
        );
    }

    #[test]
    fn bear_only_marks_are_styled_not_shown() {
        let lines = render("a ==big== deal and an ~under~ line, path ~/foo/bar, ~~strike~~ stays");
        assert_eq!(
            plain(&lines),
            vec!["a big deal and an under line, path ~/foo/bar, strike stays"]
        );
        let big = lines[0].spans.iter().find(|s| s.content == "big").unwrap();
        assert!(big.style.add_modifier.contains(Modifier::REVERSED));
        let under = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "under")
            .unwrap();
        assert!(under.style.add_modifier.contains(Modifier::UNDERLINED));
        let strike = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "strike")
            .unwrap();
        assert!(strike.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn tag_line_is_a_tag_row_and_headings_are_bold() {
        let lines = render("# T\n#robotics/Coding #tech\n\nbody");
        assert_eq!(
            plain(&lines),
            vec!["T", "", "#robotics/Coding #tech", "", "body"]
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(
            lines[2]
                .spans
                .iter()
                .find(|s| s.content.contains("#robotics"))
                .unwrap()
                .style
                .fg,
            Some(theme::ACCENT)
        );
        assert_ne!(lines[0].block, lines[2].block);
        assert_ne!(lines[2].block, lines[4].block);
    }

    #[test]
    fn lists_quotes_code_and_tables() {
        let src = "## Frame\n- Brace the *north* wall\n  - inner\n> quoted `code`\n\n```python\nx = 1\n```\n\n| a | b |\n|---|--:|\n| one | 2 |\n";
        let out = plain(&render(src));
        assert_eq!(
            out,
            vec![
                "Frame",
                "",
                "• Brace the north wall",
                "  • inner",
                "",
                "▎ quoted code",
                "",
                "  python",
                "  x = 1",
                "",
                "a   │ b",
                "────┼──",
                "one │ 2",
            ],
            "{out:?}"
        );
    }

    #[test]
    fn images_and_links() {
        let out = plain(&render(
            "see [Bear](https://bear.app) and ![Front bed](Front%20bed.png)",
        ));
        assert_eq!(out, vec!["see Bear and [image: Front bed]"]);
    }

    #[test]
    fn wrap_breaks_at_spaces_and_indents_continuations() {
        let line = RLine {
            spans: vec![Span::raw("• "), Span::raw("one two three four five six")],
            block: 1,
            cont: vec![Span::raw("  ")],
        };
        let rows: Vec<String> = wrap(&line, 12).iter().map(|l| l.to_string()).collect();
        assert_eq!(rows, vec!["• one two", "  three four", "  five six"]);
        let long = RLine {
            spans: vec![Span::raw("abcdefghijklmnop")],
            block: 1,
            cont: vec![],
        };
        let rows: Vec<String> = wrap(&long, 5).iter().map(|l| l.to_string()).collect();
        assert_eq!(rows, vec!["abcde", "fghij", "klmno", "p"]);
        let full = render(&format!(
            "# Reading Queue\n\n{}\n",
            (1..=120)
                .map(|i| format!("- Book {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        assert!(full.iter().any(|l| l.plain() == "• Book 120"));
    }
}
