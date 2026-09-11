//! Search-term highlighting over rendered lines.

use ratatui::text::{Line, Span};
use regex::Regex;

use crate::ui::theme;

/// Restyle every match of `pattern` in `line` with the match style, splitting
/// spans where a match starts or ends.
pub fn highlight_line(line: Line<'static>, pattern: &Regex) -> Line<'static> {
    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges: Vec<(usize, usize)> = pattern
        .find_iter(&plain)
        .map(|m| (m.start(), m.end()))
        .collect();
    if ranges.is_empty() {
        return line;
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut offset = 0usize;
    for span in line.spans {
        let text = span.content.as_ref();
        let mut buf = String::new();
        let mut buf_hit = false;
        for (i, ch) in text.char_indices() {
            let abs = offset + i;
            let hit = ranges.iter().any(|(s, e)| abs >= *s && abs < *e);
            if hit != buf_hit && !buf.is_empty() {
                let style = if buf_hit {
                    span.style.patch(theme::match_style())
                } else {
                    span.style
                };
                out.push(Span::styled(std::mem::take(&mut buf), style));
            }
            buf_hit = hit;
            buf.push(ch);
        }
        if !buf.is_empty() {
            let style = if buf_hit {
                span.style.patch(theme::match_style())
            } else {
                span.style
            };
            out.push(Span::styled(buf, style));
        }
        offset += text.len();
    }
    Line::from(out)
}

/// Does `pattern` match anywhere in the line's text?
pub fn line_matches(line: &Line<'_>, pattern: &Regex) -> bool {
    let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    pattern.is_match(&plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn matches_are_split_out_and_styled() {
        let line = Line::from(vec![Span::raw("the "), Span::raw("bulbs are bulbs")]);
        let out = highlight_line(line, &Regex::new("(?i)bulbs").unwrap());
        let hits: Vec<&Span> = out
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .collect();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|s| s.content == "bulbs"));
        assert_eq!(out.to_string(), "the bulbs are bulbs");
    }
}
