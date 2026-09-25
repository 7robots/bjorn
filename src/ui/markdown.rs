//! Bear markdown -> styled terminal lines.
//!
//! The Python Bjorn handed markdown to Textual's `Markdown` widget, which
//! mounts one widget per block and so had to render long notes in two halves.
//! Here `pulldown-cmark` events become plain `ratatui` lines that carry the
//! index of the block they belong to, so the whole note is rendered once,
//! scrolling is free, and search can count and jump by block as before.

use std::sync::LazyLock;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd, TextMergeStream,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

use crate::render::{
    DONE_BOX, HIGHLIGHT_RE, OPEN_BOX, UNDERLINE_RE, is_fence, is_tag_line, tags_in_line,
};
use crate::ui::theme;
use crate::wiki::{self, LinkTable, Piece, WikiLink};

/// One rendered line: its spans, the block it belongs to, and the prefix a
/// wrapped continuation of it starts with (list indent, quote bar).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RLine {
    pub spans: Vec<Span<'static>>,
    pub block: usize,
    pub cont: Vec<Span<'static>>,
    /// Wiki links drawn on this line: the index of the span that shows each.
    pub links: Vec<(usize, WikiLink)>,
}

impl RLine {
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.plain().trim().is_empty()
    }
}

/// A heading as the reader drew it: its level (1-6), its text without the
/// `#`s or inline markers, and the block it opens, which is how the reader
/// finds the row it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub block: usize,
}

static MARK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^</?(mark|u|tags)>$").unwrap());

/// Bear-only syntax rewritten into inline HTML the renderer understands, with
/// fenced code left alone. Tag lines become `<tags>…</tags>` rows, and every
/// wiki link becomes a token into the returned table, so nothing inside one
/// is ever read as markdown.
pub fn prepare(content: &str) -> (String, LinkTable) {
    let mut table = LinkTable::default();
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in content.lines() {
        if is_fence(line) || in_fence {
            if is_fence(line) {
                in_fence = !in_fence;
            }
            // Only link tokens may carry the token characters, in code too.
            out.push(wiki::neutralize(line).into_owned());
            continue;
        }
        if is_tag_line(line) {
            out.push(format!("<tags>{}</tags>", tags_in_line(line).join(" ")));
            continue;
        }
        // A setext underline (`=====`) is a heading marker, not a highlight.
        if line.trim().chars().all(|c| c == '=') && !line.trim().is_empty() {
            out.push(line.to_string());
            continue;
        }
        let line = table.tokenize(line);
        let line = HIGHLIGHT_RE.replace_all(&line, "<mark>$1</mark>");
        let line = UNDERLINE_RE.replace_all(&line, "<u>$1</u>");
        out.push(line.into_owned());
    }
    (out.join("\n"), table)
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
    /// Links on the line being built, by span index.
    line_links: Vec<(usize, WikiLink)>,
    /// The note's wiki links, which its text refers to by token.
    wiki: LinkTable,
    headings: Vec<Heading>,
    /// The heading being read, while inside one.
    heading: Option<Heading>,
}

/// One table cell: its spans, and the links among them by span index.
#[derive(Clone, Default)]
struct Cell {
    spans: Vec<Span<'static>>,
    links: Vec<(usize, WikiLink)>,
}

struct TableState {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    header_rows: usize,
    in_head: bool,
    cell: Cell,
    row: Vec<Cell>,
}

fn code_style() -> Style {
    theme::code()
}

fn tag_style() -> Style {
    theme::tag()
}

impl Renderer {
    fn new(wiki: LinkTable) -> Renderer {
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
            line_links: Vec::new(),
            wiki,
            headings: Vec::new(),
            heading: None,
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
        self.line_links.clear();
    }

    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let cont = std::mem::take(&mut self.cont);
        let links = std::mem::take(&mut self.line_links);
        self.lines.push(RLine {
            spans,
            block: self.block,
            cont,
            links,
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
                ..RLine::default()
            });
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_image {
            let alt = self.wiki.restore(text).into_owned();
            self.image_alt.push_str(&alt);
            return;
        }
        if let Some(heading) = self.heading.as_mut() {
            // As drawn: a link by its label, so `[[Note/Heading]]` finds it
            // by the text the reader shows.
            for piece in self.wiki.split(text) {
                match piece {
                    Piece::Text(t) => heading.text.push_str(&t),
                    Piece::Link(link) => heading.text.push_str(&link.label()),
                }
            }
        }
        if self.table.is_some() {
            let style = self.style();
            let pieces = self.wiki.split(text);
            if let Some(table) = self.table.as_mut() {
                for piece in pieces {
                    let span = match piece {
                        Piece::Text(t) => Span::styled(t, style),
                        Piece::Link(link) => {
                            let span = Span::styled(link.label(), style.patch(theme::link()));
                            table.cell.links.push((table.cell.spans.len(), link));
                            span
                        }
                    };
                    table.cell.spans.push(span);
                }
            }
            return;
        }
        if self.in_code {
            let style = self.style();
            let text = self.wiki.restore(text).into_owned();
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
        let style = self.style();
        for piece in self.wiki.split(text) {
            match piece {
                Piece::Text(t) => self.spans.push(Span::styled(t, style)),
                Piece::Link(link) => {
                    self.line_links.push((self.spans.len(), link.clone()));
                    self.spans
                        .push(Span::styled(link.label(), style.patch(theme::link())));
                }
            }
        }
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
        theme::heading(level as u8)
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
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                widths[i] = widths[i].max(w);
            }
        }
        let prefix = self.quote_prefix();
        for (r, row) in table.rows.iter().enumerate() {
            let mut spans = prefix.clone();
            let mut links: Vec<(usize, WikiLink)> = Vec::new();
            for (i, width) in widths.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                let w: usize = cell
                    .spans
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
                let base = spans.len();
                links.extend(cell.links.into_iter().map(|(k, l)| (base + k, l)));
                for mut s in cell.spans {
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
                links,
                ..RLine::default()
            });
            if r + 1 == table.header_rows && table.header_rows > 0 {
                let mut spans = prefix.clone();
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                spans.push(Span::styled(rule.join("─┼─"), theme::muted()));
                self.lines.push(RLine {
                    spans,
                    block: self.block,
                    ..RLine::default()
                });
            }
        }
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// The wiki links a note makes, in order, exactly as the reader draws them:
/// from text, table cells and HTML, and none from code (spans or blocks,
/// fenced or indented) or image captions. `L` and the backlink check both
/// use this, so the three always agree.
pub fn wiki_links(content: &str) -> Vec<WikiLink> {
    let (prepared, table) = prepare(content);
    let (mut in_code, mut in_image) = (false, false);
    let mut out = Vec::new();
    for event in TextMergeStream::new(Parser::new_ext(&prepared, options())) {
        let text = match event {
            Event::Start(Tag::CodeBlock(_)) => {
                in_code = true;
                continue;
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                continue;
            }
            Event::Start(Tag::Image { .. }) => {
                in_image = true;
                continue;
            }
            Event::End(TagEnd::Image) => {
                in_image = false;
                continue;
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => text,
            _ => continue,
        };
        if in_code || in_image {
            continue;
        }
        out.extend(table.split(&text).into_iter().filter_map(|p| match p {
            Piece::Link(link) => Some(link),
            Piece::Text(_) => None,
        }));
    }
    out
}

/// Render Bear markdown into block-indexed lines.
pub fn render(content: &str) -> Vec<RLine> {
    render_with_headings(content).0
}

/// Render Bear markdown, and list its headings in document order. The
/// headings come from the same parse that draws the note, so a `#` line in
/// fenced code is not one, a setext heading is, and each maps to the block
/// the reader shows it in.
pub fn render_with_headings(content: &str) -> (Vec<RLine>, Vec<Heading>) {
    let (prepared, table) = prepare(content);
    let mut r = Renderer::new(table);
    // Merged, so a link token never arrives in two pieces.
    for event in TextMergeStream::new(Parser::new_ext(&prepared, options())) {
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
                    r.heading = Some(Heading {
                        level: level as u8,
                        text: String::new(),
                        block: r.block,
                    });
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
                    r.spans.push(Span::styled(marker, theme::bullet()));
                    r.item_open = true;
                }
                Tag::Emphasis => r.push(Style::default().add_modifier(Modifier::ITALIC)),
                Tag::Strong => r.push(Style::default().add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => r.push(Style::default().add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { .. } => r.push(theme::link()),
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
                        cell: Cell::default(),
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
                        t.cell = Cell::default();
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
                    // An empty heading draws no row, so it could not be jumped to.
                    if let Some(mut heading) = r.heading.take() {
                        heading.text = heading.text.trim().to_string();
                        if !heading.text.is_empty() {
                            r.headings.push(heading);
                        }
                    }
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
                let code = r.wiki.restore(&code).into_owned();
                if let Some(heading) = r.heading.as_mut() {
                    heading.text.push_str(&code);
                }
                if let Some(t) = r.table.as_mut() {
                    t.cell.spans.push(Span::styled(code, style));
                } else {
                    if r.spans.is_empty() {
                        r.begin_line();
                    }
                    r.spans.push(Span::styled(code, style));
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
    (r.lines, r.headings)
}

/// The image file names a note links to, percent-decoded, in order.
pub fn image_name(dest: &str) -> String {
    crate::render::percent_decode(dest)
}

/// Break one line into rows no wider than `width`, at spaces where possible,
/// each continuation starting with the line's `cont` prefix.
pub fn wrap(line: &RLine, width: usize) -> Vec<Line<'static>> {
    wrap_mapped(line, width)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// Where one of a line's spans landed on a wrapped row: the columns
/// `start..end` show (part of) span `span`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub start: usize,
    pub end: usize,
    pub span: usize,
}

/// `wrap`, also saying where each of the line's spans was placed on each row,
/// so a click can be traced back to the span (and the link) under it.
pub fn wrap_mapped(line: &RLine, width: usize) -> Vec<(Line<'static>, Vec<Placed>)> {
    let width = width.max(1);
    let mut rows: Vec<(Line<'static>, Vec<Placed>)> = Vec::new();
    let mut placed: Vec<Placed> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let cont_width: usize = line
        .cont
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let mut chunks: Vec<(String, Style, usize)> = Vec::new();
    for (index, span) in line.spans.iter().enumerate() {
        let mut word = String::new();
        let mut spaces = false;
        for ch in span.content.chars() {
            let is_space = ch == ' ';
            if !word.is_empty() && is_space != spaces {
                chunks.push((std::mem::take(&mut word), span.style, index));
            }
            spaces = is_space;
            word.push(ch);
        }
        if !word.is_empty() {
            chunks.push((word, span.style, index));
        }
    }
    let new_row = |rows: &mut Vec<(Line<'static>, Vec<Placed>)>,
                   placed: &mut Vec<Placed>,
                   current: &mut Vec<Span<'static>>,
                   used: &mut usize| {
        // A row never ends in the spaces it broke at.
        while current.len() > line.cont.len()
            && current.last().is_some_and(|s| s.content.trim().is_empty())
        {
            current.pop();
        }
        if let Some(last) = current.last_mut() {
            last.content = last.content.trim_end().to_string().into();
        }
        rows.push((Line::from(std::mem::take(current)), std::mem::take(placed)));
        *current = line.cont.clone();
        *used = cont_width.min(width.saturating_sub(1));
    };
    for (chunk, style, span) in chunks {
        let w = UnicodeWidthStr::width(chunk.as_str());
        if used + w <= width {
            placed.push(Placed {
                start: used,
                end: used + w,
                span,
            });
            current.push(Span::styled(chunk, style));
            used += w;
            continue;
        }
        if chunk.trim().is_empty() {
            // A run of spaces at the edge is where the row breaks.
            new_row(&mut rows, &mut placed, &mut current, &mut used);
            continue;
        }
        // Break only if this row holds something of its own: on a continuation
        // row `used` starts at the prefix's width, and the first row's spans
        // carry that same prefix, so `cont_width` is the empty mark on both.
        if used > cont_width {
            new_row(&mut rows, &mut placed, &mut current, &mut used);
        }
        if used + w <= width {
            placed.push(Placed {
                start: used,
                end: used + w,
                span,
            });
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
                placed.push(Placed {
                    start: used,
                    end: used + pw,
                    span,
                });
                current.push(Span::styled(std::mem::take(&mut piece), style));
                new_row(&mut rows, &mut placed, &mut current, &mut used);
                pw = 0;
            }
            piece.push(ch);
            pw += cw;
        }
        if !piece.is_empty() {
            placed.push(Placed {
                start: used,
                end: used + pw,
                span,
            });
            current.push(Span::styled(piece, style));
            used += pw;
        }
    }
    rows.push((Line::from(current), placed));
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
            Some(theme::current().tag_fg)
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
    fn wiki_links_are_drawn_as_links_and_code_keeps_its_brackets() {
        let src = "# Title\n\nSee [[Garden Plan/Next spring]] and [[Reading Queue|the list]], \
                   not `[[In Code]]`.\n\n```\n[[In Fence]]\n```\n\n    [[Indented Code]]\n";
        let lines = render(src);
        let out = plain(&lines);
        assert!(
            out.contains(
                &"See Garden Plan › Next spring and the list, not [[In Code]].".to_string()
            ),
            "{out:?}"
        );
        assert!(out.iter().any(|l| l.contains("[[In Fence]]")), "{out:?}");
        assert!(
            out.iter().any(|l| l.contains("[[Indented Code]]")),
            "{out:?}"
        );
        let para = lines.iter().find(|l| l.plain().starts_with("See")).unwrap();
        let styled: Vec<&str> = para
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(theme::current().link))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(styled, vec!["Garden Plan › Next spring", "the list"]);
        let linked: Vec<&str> = para.links.iter().map(|(_, l)| l.title.as_str()).collect();
        assert_eq!(linked, vec!["Garden Plan", "Reading Queue"]);
        for (index, link) in &para.links {
            assert_eq!(para.spans[*index].content, link.label());
        }
        let (_, headings) = render_with_headings(src);
        assert_eq!(headings.first().map(|h| h.block), Some(lines[0].block));
        assert!(headings.iter().all(|h| h.block != para.block));
        assert_eq!(
            wiki_links(src)
                .iter()
                .map(|l| l.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Garden Plan", "Reading Queue"]
        );
    }

    #[test]
    fn escaped_slashes_and_alias_bars_survive_the_parser() {
        let src = "[[New\\/Modern DNS/ESXi]] and `a\\/b` in code\n\n| a | b |\n|---|---|\n| [[One|uno]] | two |\n";
        let lines = render(src);
        let out = plain(&lines);
        assert_eq!(out[0], "New/Modern DNS › ESXi and a\\/b in code", "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("uno │ two")), "{out:?}");
        let links = wiki_links(src);
        assert_eq!(links[0].title, "New/Modern DNS");
        assert_eq!(links[0].section, "ESXi");
        assert_eq!(
            (links[1].title.as_str(), links[1].alias.as_str()),
            ("One", "uno")
        );
    }

    #[test]
    fn token_characters_in_a_fence_are_neutralized() {
        let (prepared, _) = prepare("```\na \u{FDD0}0\u{FDD1} b\n```\n");
        assert!(!prepared.contains(['\u{FDD0}', '\u{FDD1}']), "{prepared:?}");
        assert!(prepared.contains("a \u{FFFD}0\u{FFFD} b"));
    }

    #[test]
    fn markup_inside_a_link_is_part_of_its_title() {
        let cases = [
            "[[September 6, 2021 - September `0, 2021 (Weekly)]]",
            "[[a *b* c]]",
            "[[a ~b~ c]]",
            "[[a ==b== c]]",
            "[[Q&A]]",
            "[[a <b> c]]",
            "[[Ref]]",
        ];
        for case in cases {
            let src = format!("{case}\nnext line\n\n[Ref]: https://example.com\n");
            let links = wiki_links(&src);
            let want = &case[2..case.len() - 2];
            assert_eq!(links.len(), 1, "{case}: {links:?}");
            assert_eq!(links[0].title, want, "{case}");
            let lines = render(&src);
            let first = &lines[0];
            assert_eq!(first.plain(), format!("{want} next line"), "{case}");
            assert_eq!(first.links.len(), 1, "{case}");
            assert_eq!(first.spans[first.links[0].0].content, want);
        }
    }

    #[test]
    fn links_in_html_blocks_and_tables_count_everywhere() {
        let src = "<div>\n[[Inside Html]]\n</div>\n\n| a |\n|---|\n| [[In Table|t]] |\n\n![see [[Not In Alt]]](x.png)\n";
        let titles: Vec<String> = wiki_links(src).into_iter().map(|l| l.title).collect();
        assert_eq!(titles, vec!["Inside Html", "In Table"]);
        let lines = render(src);
        let html = lines
            .iter()
            .find(|l| l.plain().contains("Inside Html"))
            .unwrap();
        assert_eq!(html.links.len(), 1);
        let row = lines.iter().find(|l| l.plain().starts_with('t')).unwrap();
        let (index, link) = &row.links[0];
        assert_eq!(row.spans[*index].content, "t");
        assert_eq!(link.title, "In Table");
        assert!(
            lines
                .iter()
                .any(|l| l.plain().contains("[image: see [[Not In Alt]]]")),
            "{:?}",
            plain(&lines)
        );
    }

    #[test]
    fn wrapping_maps_columns_back_to_spans() {
        let line = render("aaa [[Target|link text]] bbb").remove(0);
        let rows = wrap_mapped(&line, 12);
        let (index, _) = &line.links[0];
        let hits: Vec<(usize, Placed)> = rows
            .iter()
            .enumerate()
            .flat_map(|(r, (_, placed))| placed.iter().map(move |p| (r, *p)))
            .filter(|(_, p)| p.span == *index)
            .collect();
        // One entry per word or run of spaces; the link wraps onto row two.
        assert_eq!((hits[0].0, hits[0].1.start), (0, 4), "{hits:?}");
        let last = hits.last().unwrap();
        assert_eq!((last.0, last.1.start, last.1.end), (1, 0, 4), "{hits:?}");
    }

    #[test]
    fn wrap_breaks_at_spaces_and_indents_continuations() {
        let line = RLine {
            spans: vec![Span::raw("• "), Span::raw("one two three four five six")],
            block: 1,
            cont: vec![Span::raw("  ")],
            ..RLine::default()
        };
        let rows: Vec<String> = wrap(&line, 12).iter().map(|l| l.to_string()).collect();
        assert_eq!(rows, vec!["• one two", "  three four", "  five six"]);
        let long = RLine {
            spans: vec![Span::raw("abcdefghijklmnop")],
            block: 1,
            ..RLine::default()
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

    fn outline(src: &str) -> Vec<(u8, String)> {
        render_with_headings(src)
            .1
            .into_iter()
            .map(|h| (h.level, h.text))
            .collect()
    }

    #[test]
    fn headings_skip_fenced_code_and_keep_duplicates() {
        let src = "# Title\n#tag\n\n## Setup\ntext\n\n```sh\n# a shell comment\n## not a heading\n```\n\n~~~\n# tilde fence\n~~~\n\n### Notes\n\n## Setup\n\n#hashtag line\n\n####### seven is text\n";
        assert_eq!(
            outline(src),
            vec![
                (1, "Title".to_string()),
                (2, "Setup".to_string()),
                (3, "Notes".to_string()),
                (2, "Setup".to_string()),
            ]
        );
    }

    #[test]
    fn heading_text_drops_markup_and_keeps_code() {
        let src = "## *Bold* ==mark== and `code`\n\nSetext\n------\n";
        assert_eq!(
            outline(src),
            vec![
                (2, "Bold mark and code".to_string()),
                (2, "Setext".to_string()),
            ]
        );
    }

    #[test]
    fn each_heading_names_the_block_it_is_drawn_in() {
        let (lines, headings) = render_with_headings("# T\n\npara\n\n## A\n\n- item\n\n## B\n");
        for heading in &headings {
            let line = lines
                .iter()
                .find(|l| l.block == heading.block)
                .expect("the heading's block is drawn");
            assert_eq!(line.plain(), heading.text);
        }
    }
}
