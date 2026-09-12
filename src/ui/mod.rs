//! Drawing: the three columns, their headers, the footer, overlays and toasts.

pub mod highlight;
pub mod markdown;
pub mod modals;
pub mod note_list;
pub mod note_view;
pub mod sidebar;
pub mod theme;
pub mod triage;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Pane, Rects};
use crate::export::FORMATS;
use crate::search_box::HINT;
use crate::ui::modals::{Field, Overlay, Severity};
use crate::ui::note_list::ROW_HEIGHT;
use crate::ui::note_view::column_glyph;
use tui_term::widget::{Cursor, PseudoTerminal};

pub const SIDEBAR_WIDTH: u16 = 28;
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
    ("a", "Actions"),
    ("x", "Export"),
    ("b", "Bear"),
    ("w", "Workspace"),
    ("f", "Fold"),
    ("c", "Columns"),
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Every pane paints its own surface over this; it covers the gaps and
    // gives the reader and the modals their background.
    frame.buffer_mut().set_style(area, theme::screen());
    let [body, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(area);
    let mut rects = Rects::default();

    if app.triage.is_some() {
        draw_triage(frame, app, body);
        draw_footer_entries(frame, footer, TRIAGE_FOOTER);
        app.rects = rects;
        draw_toasts(frame, app, body);
        if let Some(overlay) = app.overlay.clone() {
            draw_overlay(frame, app, area, &overlay);
        }
        return;
    }

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
    if app.editing.is_some() {
        draw_footer_entries(frame, footer, EDITING_FOOTER);
    } else {
        draw_footer(frame, footer);
    }
    app.rects = rects;

    draw_toasts(frame, app, body);
    if let Some(overlay) = app.overlay.clone() {
        draw_overlay(frame, app, area, &overlay);
    }
}

fn header_line(text: &str, width: u16, style: Style) -> Paragraph<'static> {
    let padded = format!(
        " {text}{}",
        " ".repeat((width as usize).saturating_sub(UnicodeWidthStr::width(text) + 1))
    );
    Paragraph::new(Line::from(Span::styled(padded, style)))
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect, rects: &mut Rects) {
    let focused = app.focus == Pane::Sidebar;
    frame
        .buffer_mut()
        .set_style(area, theme::sidebar_surface(focused));
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(theme::sidebar_border());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header, rows] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let style = if focused {
        theme::header_focused()
    } else {
        theme::sidebar_header()
    };
    frame.render_widget(
        header_line(&app.sidebar.header(), header.width, style),
        header,
    );
    rects.sidebar_header = header;
    rects.sidebar_rows = rows;
    app.sidebar.ensure_visible(rows.height as usize);
    let width = rows.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for index in
        app.sidebar.scroll..(app.sidebar.scroll + rows.height as usize).min(app.sidebar.rows.len())
    {
        let cursor = if index == app.sidebar.cursor {
            Some(theme::sidebar_cursor(focused))
        } else {
            None
        };
        lines.push(app.sidebar.render_row(index, width, cursor));
    }
    frame.render_widget(Paragraph::new(lines), rows);
}

fn draw_notes(frame: &mut Frame, app: &mut App, area: Rect, rects: &mut Rects) {
    let focused = app.focus == Pane::Notes;
    frame.buffer_mut().set_style(area, theme::surface(focused));
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
    let style = if focused {
        theme::header_focused()
    } else {
        theme::header()
    };
    frame.render_widget(header_line(&app.notes.header, header.width, style), header);
    rects.notes_header = header;
    rects.notes_rows = rows;
    if app.notes.search.open {
        draw_search_box(frame, app, search, rects);
    }
    if app.notes.is_empty() {
        // Before the first snapshot lands an empty column reads as broken, so
        // it says which it is.
        let text = if app.loaded { "No notes" } else { "Loading…" };
        let empty = Paragraph::new(Line::from(Span::styled(text, theme::muted())))
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
        Style::default().fg(theme::accent_color())
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
        theme::cursor_focused()
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

/// `text` cut to `width` display cells, with … when cut, and padded out to it.
fn fit_cells(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let full = UnicodeWidthStr::width(text);
    if full <= width {
        return format!("{text}{}", " ".repeat(width - full));
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    format!("{out}…{}", " ".repeat(width.saturating_sub(used + 1)))
}

/// A path with the home directory written as `~`.
fn tilde_path(path: &std::path::Path) -> String {
    match path.strip_prefix(crate::util::home_dir()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
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
        theme::header()
    };
    let glyph = column_glyph(app.columns);
    let glyph_style = if focused {
        theme::header_focused()
    } else {
        theme::header().fg(theme::current().muted)
    };
    let title_style = if focused {
        theme::header_focused()
    } else {
        theme::header()
    };
    let glyph_width = UnicodeWidthStr::width(glyph) + 2;
    let title = match &app.editing {
        Some(editing) => format!("Editing · {}", editing.job.note.title),
        None => app.reader.header.clone(),
    };
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
    app.set_reader_viewport(inner.width as usize, inner.height as usize);
    if let Some(editing) = app.editing.as_mut() {
        // The editor's screen fills the reader body; the pane's own cursor
        // stands in for the editor's.
        rects.editor = inner;
        editing.pty.resize(inner.height, inner.width);
        let parser = editing.pty.parser();
        let screen = parser.screen();
        let widget = PseudoTerminal::new(screen).cursor(Cursor::default().visibility(false));
        frame.render_widget(widget, inner);
        if !screen.hide_cursor() {
            let (row, col) = screen.cursor_position();
            if row < inner.height && col < inner.width {
                frame.set_cursor_position((inner.x + col, inner.y + row));
            }
        }
    } else {
        let lines = app
            .reader
            .visible(inner.width as usize, inner.height as usize);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    let meta_text = match &app.editing {
        Some(editing) => format!(
            " {} · quit the editor to save back to Bear",
            editing.job.command[0]
        ),
        None => format!(" {}", app.reader.meta),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(meta_text, theme::muted()))),
        meta,
    );
}

/// While an editor is open every key goes to it; the footer says so.
pub const EDITING_FOOTER: &[(&str, &str)] = &[
    ("editor", "Keys go to the editor"),
    ("quit it", "Save back to Bear"),
];

pub const TRIAGE_FOOTER: &[(&str, &str)] = &[
    ("esc", "Close"),
    ("space", "Mark"),
    ("x", "Tick in Bear"),
    ("b", "Bear"),
    ("a", "Add to Reminders"),
    ("/", "Filter"),
    ("r", "Reload"),
    ("?", "Help"),
];

fn draw_triage(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(triage) = app.triage.as_mut() else {
        return;
    };
    let filter_rows = if triage.filter.is_some() { 3 } else { 0 };
    let [header, filter, list, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(filter_rows),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(Clear, area);
    frame.buffer_mut().set_style(area, theme::surface(true));
    let head = format!(" {}", triage.header());
    let pad = (header.width as usize).saturating_sub(UnicodeWidthStr::width(head.as_str()));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{head}{}", " ".repeat(pad)),
            Style::default()
                .fg(theme::accent_color())
                .add_modifier(Modifier::BOLD),
        ))),
        header,
    );
    if let Some(field) = &triage.filter {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::accent_color()));
        let inner = block.inner(filter);
        frame.render_widget(block, filter);
        let shown = if field.value.is_empty() {
            Line::from(Span::styled(
                "Filter todos — enter to apply, esc to clear",
                theme::muted(),
            ))
        } else {
            Line::from(field.value.clone())
        };
        frame.render_widget(Paragraph::new(shown), inner);
        field_cursor(frame, field, inner.x, inner.y, inner.width);
    }
    let lines = triage.lines();
    if lines.is_empty() && triage.loaded {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No open todos in scope",
                theme::muted(),
            )))
            .block(Block::default().padding(Padding::new(3, 3, 2, 0))),
            list,
        );
    } else {
        let cursor_line = lines.iter().position(|l| matches!(l, crate::ui::triage::TriageLine::Todo(i) if triage.items.get(triage.cursor) == Some(i)));
        let height = list.height as usize;
        if let Some(c) = cursor_line {
            if c < triage.scroll {
                triage.scroll = c.saturating_sub(2);
            } else if c >= triage.scroll + height {
                triage.scroll = c + 1 - height;
            }
        }
        triage.scroll = triage.scroll.min(lines.len().saturating_sub(height));
        let mut rendered: Vec<Line<'static>> = Vec::new();
        for line in lines.iter().skip(triage.scroll).take(height) {
            let mut row = match line {
                crate::ui::triage::TriageLine::Blank => Line::default(),
                crate::ui::triage::TriageLine::Header { title, count, tags } => {
                    crate::ui::triage::Triage::render_header(title, *count, tags)
                }
                crate::ui::triage::TriageLine::Todo(i) => {
                    let is_cursor = triage.items.get(triage.cursor) == Some(i);
                    let style = if is_cursor {
                        Some(theme::cursor_focused())
                    } else {
                        None
                    };
                    let mut r = triage.render_row(&triage.rows[*i], style);
                    if let Some(base) = style {
                        let used: usize = r
                            .spans
                            .iter()
                            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                            .sum();
                        r.spans.push(Span::styled(
                            " ".repeat((list.width as usize).saturating_sub(used + 2)),
                            base,
                        ));
                    }
                    r
                }
            };
            row.spans.insert(0, Span::raw("  "));
            rendered.push(row);
        }
        frame.render_widget(Paragraph::new(rendered), list);
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", triage.status),
            theme::muted(),
        ))),
        status,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    draw_footer_entries(frame, area, FOOTER);
}

/// The footer is two rows: a rule separating it from the columns, then the key hints.
fn draw_footer_entries(frame: &mut Frame, area: Rect, entries: &[(&str, &str)]) {
    let [rule, hints] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    frame.buffer_mut().set_style(area, theme::footer());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rule.width as usize),
            theme::footer().fg(theme::current().border),
        ))),
        rule,
    );
    let area = hints;
    let key_style = theme::footer_key();
    let label_style = theme::footer();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (key, label) in entries {
        spans.push(Span::styled(format!(" {key} "), key_style));
        spans.push(Span::styled(format!("{label} "), label_style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_toasts(frame: &mut Frame, app: &App, area: Rect) {
    let width = 50u16.min(area.width.saturating_sub(4));
    let mut bottom = area.y + area.height;
    for toast in app.toasts.iter().rev() {
        let color = match toast.severity {
            Severity::Information => theme::accent_color(),
            Severity::Warning => theme::warning_color(),
            Severity::Error => theme::error_color(),
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
            .style(theme::screen())
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
        .style(theme::screen())
        .border_style(Style::default().fg(theme::accent_color()));
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
                            .bg(theme::error_color())
                            .fg(theme::current().header_focus_fg)
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
        Overlay::Actions { field, index, note } => {
            let all = &app.config.actions;
            let matched = crate::actions::filter(all, &field.value);
            let default = crate::actions::default_action(all);
            let is_default =
                |action: &crate::actions::Action| default.is_some_and(|d| std::ptr::eq(d, action));
            let search = field.value.trim();

            let wide = area.width.saturating_sub(8).clamp(60, 104);
            // Text width inside the borders and the two-cell margin.
            let width = wide.min(area.width).saturating_sub(6) as usize;
            let chunk = width.saturating_sub(2).max(1);
            // Every matching action, then the row that adds one.
            let total = matched.len() + 1;
            let rows = total.min(12);
            let no_match = matched.is_empty() && !search.is_empty();
            // Rule, command lines, facts and a blank for the highlighted row.
            let detail = match matched.get(*index) {
                Some(a) => a.command.chars().count().div_ceil(chunk).clamp(1, 3) + 3,
                None => 3,
            };
            let height = (2 + usize::from(no_match) + rows + 1 + detail + 2 + 2) as u16;
            let title = format!("Actions · on “{}”", fit_cells(&note.title, 40).trim_end());
            let inner = dialog(frame, area, wide, height, Some(&title));

            let mut lines = Vec::new();
            if field.value.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        fit_cells("type to search/filter actions by name or command…", width),
                        theme::cursor_focused().add_modifier(Modifier::DIM),
                    ),
                ]));
            } else {
                lines.push(field_line(field, true, width));
            }
            lines.push(Line::from(""));
            if no_match {
                lines.push(Line::from(Span::styled(
                    "  no action matches",
                    theme::muted(),
                )));
            }

            // Columns: marker, name, default badge, format, confirm. The name
            // column is sized over every action so it holds still while filtering.
            let name_width = all
                .iter()
                .map(|a| UnicodeWidthStr::width(a.name.as_str()))
                .max()
                .unwrap_or(0)
                .clamp(8, width.saturating_sub(40).max(8));
            // The list scrolls under the highlight once it is past the window.
            let top = index.saturating_sub(rows - 1);
            for i in top..(top + rows).min(total) {
                let selected = i == *index;
                let style = if selected {
                    theme::match_style()
                } else {
                    Style::default()
                };
                let marker = Span::styled(if selected { "  ▸ " } else { "    " }, style);
                let Some(&action) = matched.get(i) else {
                    let label = if search.is_empty() {
                        "+ New action…".to_string()
                    } else {
                        format!("+ New action “{search}”…")
                    };
                    lines.push(Line::from(vec![
                        marker,
                        Span::styled(
                            fit_cells(&label, width.saturating_sub(4))
                                .trim_end()
                                .to_string(),
                            (if selected { style } else { theme::accent() })
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    continue;
                };
                let label = crate::export::format_by_id(&action.format).label;
                lines.push(Line::from(vec![
                    marker,
                    Span::styled(
                        fit_cells(&action.name, name_width),
                        style.add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        if is_default(action) {
                            "★ default "
                        } else {
                            "          "
                        },
                        theme::accent().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {label:<10}"), theme::muted()),
                    Span::styled(
                        if action.confirm { "  asks first" } else { "" },
                        Style::default().fg(theme::warning_color()),
                    ),
                ]));
            }
            lines.push(Line::from(""));

            // The highlighted row in full, so a long command is never a guess.
            lines.push(Line::from(Span::styled(
                format!("  {}", "─".repeat(width)),
                theme::border(),
            )));
            match matched.get(*index) {
                Some(&action) => {
                    let command: Vec<char> = action.command.chars().collect();
                    let pieces: Vec<String> =
                        command.chunks(chunk).map(|c| c.iter().collect()).collect();
                    for (n, piece) in pieces.iter().take(3).enumerate() {
                        let text = if n == 2 && pieces.len() > 3 {
                            let kept: String =
                                piece.chars().take(chunk.saturating_sub(1)).collect();
                            format!("{kept}…")
                        } else {
                            piece.clone()
                        };
                        lines.push(Line::from(vec![
                            Span::styled(if n == 0 { "  $ " } else { "    " }, theme::muted()),
                            Span::styled(text, theme::code()),
                        ]));
                    }
                    let mut facts = vec![
                        format!(
                            "renders as {}",
                            crate::export::format_by_id(&action.format).label
                        ),
                        format!("stops after {} s", action.timeout.as_secs()),
                    ];
                    if action.confirm {
                        facts.push("asks before running".into());
                    }
                    if is_default(action) {
                        facts.push("runs on !".into());
                    }
                    lines.push(Line::from(Span::styled(
                        format!("  {}", facts.join(" · ")),
                        theme::muted(),
                    )));
                }
                None => lines.push(Line::from(Span::styled(
                    format!(
                        "  adds an [[actions]] entry to {}; your comments stay as they are",
                        tilde_path(&app.config_path())
                    ),
                    theme::muted(),
                ))),
            }
            lines.push(Line::from(""));

            lines.push(Line::from(Span::styled(
                if default.is_some() {
                    "  ★ default · ! runs it without opening this menu"
                } else {
                    "  no default yet · tick Default when adding or editing an action, and ! runs it"
                },
                theme::muted(),
            )));
            lines.push(Line::from(Span::styled(
                "  type to search/filter · ↑/↓ pick · enter runs · ctrl+e edits · ctrl+d deletes · esc closes",
                theme::muted(),
            )));
            frame.render_widget(Paragraph::new(lines), inner);
            field_cursor(frame, field, inner.x + 2, inner.y, width as u16);
        }
        Overlay::NewAction {
            name,
            command,
            format,
            confirm,
            default,
            focus,
            editing,
            ..
        } => {
            let verb = if editing.is_some() { "Edit" } else { "New" };
            let title = format!(
                "{verb} action · saved to {}",
                tilde_path(&app.config_path())
            );
            let inner = dialog(frame, area, 84, 13, Some(&title));
            let width = inner.width.saturating_sub(4) as usize;
            let heading = |row: usize| {
                if *focus == row {
                    theme::accent().add_modifier(Modifier::BOLD)
                } else {
                    theme::bold()
                }
            };
            let control = |row: usize| {
                if *focus == row {
                    theme::match_style()
                } else {
                    Style::default()
                }
            };
            // A long command scrolls sideways so the cursor stays in view.
            let scrolled = |field: &Field| {
                let start = field.cursor.saturating_sub(width.saturating_sub(1));
                Field {
                    value: field.value.chars().skip(start).take(width).collect(),
                    cursor: field.cursor - start,
                }
            };
            let (name_shown, command_shown) = (scrolled(name), scrolled(command));
            let check = |on: bool| if on { "[x]" } else { "[ ]" };
            let mut lines = vec![
                Line::from(Span::styled("  Name", heading(0))),
                field_line(&name_shown, *focus == 0, width),
                Line::from(vec![
                    Span::styled("  Command", heading(1)),
                    Span::styled(
                        "   runs through sh -c · the note is in \"$BJORN_NOTE_FILE\" and on stdin",
                        theme::muted(),
                    ),
                ]),
                field_line(&command_shown, *focus == 1, width),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Format     ", heading(2)),
                    Span::styled(format!("‹ {} ›", FORMATS[*format].label), control(2)),
                    Span::styled("   ←/→ changes it", theme::muted()),
                ]),
                Line::from(vec![
                    Span::styled("  Ask first  ", heading(3)),
                    Span::styled(check(*confirm), control(3)),
                    Span::styled("   space ticks · asks before it runs", theme::muted()),
                ]),
                Line::from(vec![
                    Span::styled("  Default    ", heading(4)),
                    Span::styled(check(*default), control(4)),
                    Span::styled(
                        "   space ticks · ! runs it without the menu",
                        theme::muted(),
                    ),
                ]),
                Line::from(""),
            ];
            // Only one action is the default; say which one this replaces.
            let replaced = app
                .config
                .actions
                .iter()
                .find(|a| a.default && Some(*a) != editing.as_ref())
                .filter(|_| *default);
            lines.push(match replaced {
                Some(old) => Line::from(Span::styled(
                    format!("  ★ “{}” stops being the default", old.name),
                    Style::default().fg(theme::warning_color()),
                )),
                None => Line::from(""),
            });
            lines.push(Line::from(Span::styled(
                "  tab or ↑/↓ moves · enter saves · esc goes back to the menu",
                theme::muted(),
            )));
            frame.render_widget(Paragraph::new(lines), inner);
            match *focus {
                0 => field_cursor(frame, &name_shown, inner.x + 2, inner.y + 1, width as u16),
                1 => field_cursor(
                    frame,
                    &command_shown,
                    inner.x + 2,
                    inner.y + 3,
                    width as u16,
                ),
                _ => {}
            }
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
