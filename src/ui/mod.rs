//! Drawing: the three columns, their headers, the footer, overlays and toasts.

pub mod highlight;
pub mod markdown;
pub mod modals;
pub mod note_list;
pub mod note_view;
pub mod sidebar;
pub mod theme;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Pane, Rects};
use crate::export::FORMATS;
use crate::search_box::HINT;
use crate::ui::modals::{Field, Overlay, Severity};
use crate::ui::note_list::ROW_HEIGHT;
use crate::ui::note_view::column_glyph;

pub const SIDEBAR_WIDTH: u16 = 30;
pub const NOTES_WIDTH: u16 = 36;

/// Footer entries: key, label. Grows as phases land.
pub const FOOTER: &[(&str, &str)] = &[
    ("q", "Quit"),
    ("?", "Help"),
    ("r", "Refresh"),
    ("/", "Search"),
    ("n", "New"),
    ("e", "Edit"),
    ("d", "Trash"),
    ("p", "Pin"),
    ("x", "Export"),
    ("b", "Bear"),
    ("w", "Workspace"),
    ("f", "Fold"),
    ("c", "Columns"),
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [body, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let mut rects = Rects::default();

    let columns: Vec<Constraint> = match app.columns {
        3 => vec![
            Constraint::Length(SIDEBAR_WIDTH),
            Constraint::Length(NOTES_WIDTH),
            Constraint::Min(20),
        ],
        2 => vec![Constraint::Length(NOTES_WIDTH), Constraint::Min(20)],
        _ => vec![Constraint::Min(20)],
    };
    let areas = Layout::horizontal(columns).split(body);
    let mut next = 0;
    if app.columns == 3 {
        draw_sidebar(frame, app, areas[next], &mut rects);
        next += 1;
    }
    if app.columns >= 2 {
        draw_notes(frame, app, areas[next], &mut rects);
        next += 1;
    }
    draw_reader(frame, app, areas[next], &mut rects);
    draw_footer(frame, footer);
    app.rects = rects;

    draw_toasts(frame, app, body);
    if let Some(overlay) = app.overlay.clone() {
        draw_overlay(frame, app, area, &overlay);
    }
}

fn header_line(text: &str, width: u16, focused: bool) -> Paragraph<'static> {
    let style = if focused {
        theme::header_focused()
    } else {
        theme::header()
    };
    let padded = format!(
        " {text}{}",
        " ".repeat((width as usize).saturating_sub(UnicodeWidthStr::width(text) + 1))
    );
    Paragraph::new(Line::from(Span::styled(padded, style)))
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect, rects: &mut Rects) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(theme::border());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let focused = app.focus == Pane::Sidebar;
    frame.render_widget(
        header_line(&app.sidebar.header(), header.width, focused),
        header,
    );
    rects.sidebar_header = header;
    rects.sidebar_rows = rows;
    // Paragraph leaves the cell after a wide glyph as it was; start from a clean pane.
    frame.render_widget(Clear, rows);
    app.sidebar.ensure_visible(rows.height as usize);
    let width = rows.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for index in
        app.sidebar.scroll..(app.sidebar.scroll + rows.height as usize).min(app.sidebar.rows.len())
    {
        let cursor = if index == app.sidebar.cursor {
            Some(if focused {
                theme::cursor_focused()
            } else {
                theme::cursor_unfocused()
            })
        } else {
            None
        };
        lines.push(app.sidebar.render_row(index, width, cursor));
    }
    frame.render_widget(Paragraph::new(lines), rows);
}

fn draw_notes(frame: &mut Frame, app: &mut App, area: Rect, rects: &mut Rects) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(theme::border());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let search_rows = if app.notes.search.open { 4 } else { 0 };
    let [header, search, rows] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(search_rows),
        Constraint::Min(1),
    ])
    .areas(inner);
    let focused = app.focus == Pane::Notes;
    frame.render_widget(
        header_line(&app.notes.header, header.width, focused),
        header,
    );
    rects.notes_header = header;
    rects.notes_rows = rows;
    frame.render_widget(Clear, rows);
    if app.notes.search.open {
        draw_search_box(frame, app, search, rects);
    }
    if app.notes.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled("No notes", theme::muted())))
            .block(Block::default().padding(Padding::new(2, 2, 1, 0)));
        frame.render_widget(empty, rows);
        return;
    }
    app.notes.ensure_visible(rows.height as usize);
    let width = rows.width.saturating_sub(2) as usize;
    let visible = (rows.height as usize).div_ceil(ROW_HEIGHT);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, note) in app
        .notes
        .notes
        .iter()
        .enumerate()
        .skip(app.notes.scroll)
        .take(visible)
    {
        let is_cursor = app.notes.cursor == Some(i);
        let cursor = if is_cursor {
            Some(if focused {
                theme::cursor_focused()
            } else {
                theme::cursor_unfocused()
            })
        } else {
            None
        };
        for mut line in app.notes.render_item(note, width, cursor) {
            let used: usize = line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let fill = (rows.width as usize).saturating_sub(used + 1);
            let base = cursor.unwrap_or_default();
            line.spans.insert(0, Span::styled(" ", base));
            line.spans.push(Span::styled(" ".repeat(fill), base));
            lines.push(line);
        }
        lines.push(Line::from(Span::styled(
            "─".repeat(rows.width as usize),
            theme::border(),
        )));
    }
    lines.truncate(rows.height as usize);
    frame.render_widget(Paragraph::new(lines), rows);
}

fn draw_search_box(frame: &mut Frame, app: &App, area: Rect, rects: &mut Rects) {
    let focused = app.focus == Pane::Search;
    let [box_area, hint] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(1)]).areas(area);
    rects.search_box = box_area;
    let border = if focused {
        Style::default().fg(theme::ACCENT)
    } else {
        theme::border()
    };
    let block = Block::default().borders(Borders::ALL).border_style(border);
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let search = &app.notes.search;
    let text_style = if focused {
        Style::default()
    } else {
        theme::dim()
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    if search.value.is_empty() && !focused {
        spans.push(Span::styled(
            "Search (Bear syntax) — enter to run, esc to clear",
            theme::muted(),
        ));
    } else {
        spans.push(Span::styled(search.value.clone(), text_style));
        if focused {
            spans.push(Span::styled(search.ghost().to_string(), theme::muted()));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
    if focused {
        let cursor_x: u16 = search
            .value
            .chars()
            .take(search.cursor)
            .map(|c| UnicodeWidthStr::width(c.to_string().as_str()) as u16)
            .sum();
        frame.set_cursor_position((
            (inner.x + cursor_x).min(inner.x + inner.width.saturating_sub(1)),
            inner.y,
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!(" {HINT}"), theme::muted()))),
        hint,
    );
}

fn field_line(field: &Field, focused: bool, width: usize) -> Line<'static> {
    let style = if focused {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    } else {
        theme::muted()
    };
    let pad = width.saturating_sub(UnicodeWidthStr::width(field.value.as_str()));
    Line::from(vec![
        Span::raw("  "),
        Span::styled(field.value.clone(), style),
        Span::styled(" ".repeat(pad), style),
    ])
}

fn field_cursor(frame: &mut Frame, field: &Field, x: u16, y: u16, width: u16) {
    let offset: u16 = field
        .value
        .chars()
        .take(field.cursor)
        .map(|c| UnicodeWidthStr::width(c.to_string().as_str()) as u16)
        .sum();
    frame.set_cursor_position(((x + offset).min(x + width.saturating_sub(1)), y));
}

fn draw_reader(frame: &mut Frame, app: &mut App, area: Rect, rects: &mut Rects) {
    let [bar, body, meta] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let focused = app.focus == Pane::Reader;
    let bar_style = if focused {
        theme::header_focused()
    } else {
        Style::default()
    };
    let glyph = column_glyph(app.columns);
    let glyph_style = if focused {
        theme::header_focused()
    } else {
        theme::muted()
    };
    let title_style = if focused {
        theme::header_focused()
    } else {
        theme::header()
    };
    let glyph_width = UnicodeWidthStr::width(glyph) + 2;
    let title = app.reader.header.clone();
    let pad =
        (bar.width as usize).saturating_sub(glyph_width + UnicodeWidthStr::width(title.as_str()));
    let line = Line::from(vec![
        Span::styled(format!(" {glyph} "), glyph_style),
        Span::styled(title, title_style),
        Span::styled(" ".repeat(pad), bar_style),
    ]);
    frame.render_widget(Paragraph::new(line), bar);
    rects.note_bar = bar;
    rects.glyph = Rect {
        x: bar.x,
        y: bar.y,
        width: glyph_width as u16,
        height: 1,
    };

    let inner = Rect {
        x: body.x + 2,
        y: body.y,
        width: body.width.saturating_sub(4),
        height: body.height,
    };
    rects.reader_body = body;
    frame.render_widget(Clear, body);
    app.set_reader_viewport(inner.width as usize, inner.height as usize);
    let lines = app
        .reader
        .visible(inner.width as usize, inner.height as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);

    let meta_text = format!(" {}", app.reader.meta);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(meta_text, theme::muted()))),
        meta,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (key, label) in FOOTER {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!("{label} "), theme::muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_toasts(frame: &mut Frame, app: &App, area: Rect) {
    let width = 50u16.min(area.width.saturating_sub(4));
    let mut bottom = area.y + area.height;
    for toast in app.toasts.iter().rev() {
        let color = match toast.severity {
            Severity::Information => theme::ACCENT,
            Severity::Warning => theme::WARNING,
            Severity::Error => theme::ERROR,
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        if !toast.title.is_empty() {
            lines.push(Line::from(Span::styled(
                toast.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(toast.message.clone()));
        let text_width = width.saturating_sub(4).max(1) as usize;
        let text_rows: usize = lines
            .iter()
            .map(|l| {
                UnicodeWidthStr::width(l.to_string().as_str())
                    .div_ceil(text_width)
                    .max(1)
            })
            .sum();
        let height = (text_rows + 2) as u16;
        if bottom < area.y + height {
            break;
        }
        let rect = Rect {
            x: area.x + area.width - width - 1,
            y: bottom - height,
            width,
            height,
        };
        bottom = rect.y;
        frame.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color));
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(block),
            rect,
        );
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn dialog(frame: &mut Frame, area: Rect, width: u16, height: u16, title: Option<&str>) -> Rect {
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT));
    if let Some(title) = title {
        block = block.title(format!(" {title} "));
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

fn draw_overlay(frame: &mut Frame, app: &mut App, area: Rect, overlay: &Overlay) {
    match overlay {
        Overlay::Confirm {
            message,
            confirm_label,
            ..
        } => {
            let inner = dialog(frame, area, 60, 7, None);
            let lines = vec![
                Line::from(""),
                Line::from(format!("  {message}")),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Cancel (n / esc)  ", theme::muted()),
                    Span::styled(
                        format!("  {confirm_label} (y)  "),
                        Style::default()
                            .bg(theme::ERROR)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
                .alignment(Alignment::Right),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Format { index, .. } => {
            let inner = dialog(frame, area, 70, 7, None);
            let mut choices: Vec<Span<'static>> = vec![Span::raw("  ")];
            for (i, fmt) in FORMATS.iter().enumerate() {
                if i > 0 {
                    choices.push(Span::raw("   "));
                }
                let selected = i == *index;
                let style = if selected {
                    theme::match_style()
                } else {
                    Style::default()
                };
                choices.push(Span::styled(
                    format!(" {} ", fmt.key),
                    if selected { style } else { theme::bold() },
                ));
                choices.push(Span::styled(format!("{} ", fmt.label), style));
            }
            let lines = vec![
                Line::from("  Export as"),
                Line::from(""),
                Line::from(choices),
                Line::from(""),
                Line::from(Span::styled(
                    "  letter or ←/→ then enter · esc to cancel",
                    theme::muted(),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Text {
            title, field, hint, ..
        } => {
            let inner = dialog(frame, area, 70, 7, None);
            let width = inner.width.saturating_sub(4) as usize;
            let lines = vec![
                Line::from(format!("  {title}")),
                Line::from(""),
                field_line(field, true, width),
                Line::from(""),
                Line::from(Span::styled(format!("  {hint}"), theme::muted())),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
            field_cursor(frame, field, inner.x + 2, inner.y + 2, width as u16);
        }
        Overlay::NewNote { title, tags, field } => {
            let inner = dialog(frame, area, 70, 10, None);
            let width = inner.width.saturating_sub(4) as usize;
            let lines = vec![
                Line::from("  New note"),
                Line::from(""),
                Line::from("  Title"),
                field_line(title, *field == 0, width),
                Line::from("  Tags (comma-separated, no #)"),
                field_line(tags, *field == 1, width),
                Line::from(""),
                Line::from(Span::styled(
                    "  enter to create and open in your editor · esc to cancel",
                    theme::muted(),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), inner);
            let (active, y) = if *field == 0 {
                (title, inner.y + 3)
            } else {
                (tags, inner.y + 5)
            };
            field_cursor(frame, active, inner.x + 2, y, width as u16);
        }
        Overlay::Help { scroll } => {
            let inner = dialog(
                frame,
                area,
                80,
                (area.height as u32 * 9 / 10) as u16,
                Some("Help (esc closes)"),
            );
            let width = inner.width.saturating_sub(4) as usize;
            let rows: Vec<Line<'static>> = markdown::render(modals::HELP_TEXT)
                .iter()
                .flat_map(|l| markdown::wrap(l, width))
                .collect();
            let max = rows.len().saturating_sub(inner.height as usize);
            let wanted = *scroll;
            let scroll = wanted.min(max);
            if scroll != wanted {
                app.overlay = Some(Overlay::Help { scroll });
            }
            let shown: Vec<Line<'static>> = rows
                .into_iter()
                .skip(scroll)
                .take(inner.height as usize)
                .collect();
            let text_area = Rect {
                x: inner.x + 2,
                y: inner.y,
                width: inner.width.saturating_sub(4),
                height: inner.height,
            };
            frame.render_widget(Paragraph::new(shown), text_area);
        }
    }
}
