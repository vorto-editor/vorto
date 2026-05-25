//! Screen-centered, interactive modal for `:grammar`.
//!
//! Lists every grammar in the recipe catalog with its install state and
//! a selection cursor. Unlike [`super::lsp_status`] (read-only), the
//! selected row can be acted on — Enter installs, `d` removes — so this
//! renderer draws a highlight bar and a per-state glyph, and auto-scrolls
//! to keep the selection in view. Action handling lives in the prompt
//! key path; this module only paints.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::prompt::{GrammarState, Prompt};

const MAX_WIDTH: u16 = 60;
const MAX_HEIGHT: u16 = 30;

pub(super) fn draw_grammar_list(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::GrammarList { rows, selected } = &app.prompt.state else {
        return;
    };
    if area.width < 12 || area.height < 6 {
        return;
    }

    // One title + one footer line eat into the body, plus the border.
    let installed = rows
        .iter()
        .filter(|r| r.state == GrammarState::Installed)
        .count();
    let title = format!(" grammars · {}/{} installed ", installed, rows.len());
    let footer = "Enter install · d remove · j/k move · Esc close";

    let inner_w = (footer.len() as u16)
        .max(title.len() as u16)
        .clamp(20, MAX_WIDTH);
    let popup_w = (inner_w + 2).min(area.width.saturating_sub(2));

    // Body rows are capped; +2 for the border, +1 for the footer line.
    let body_h = (rows.len() as u16).min(MAX_HEIGHT);
    let popup_h = (body_h + 3).min(area.height);

    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: popup_h,
    };

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::PANEL_BORDER_FG))
        .title(title)
        .style(Style::default().bg(super::PANEL_BG));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Reserve the last inner row for the footer hint; the rest is the
    // scrollable list.
    let list_h = inner.height.saturating_sub(1) as usize;
    let scroll = scroll_offset(*selected, rows.len(), list_h);

    let mut lines: Vec<Line> = Vec::with_capacity(list_h + 1);
    for (i, row) in rows.iter().enumerate().skip(scroll).take(list_h) {
        let (glyph, glyph_color) = match row.state {
            GrammarState::Installed => ("✓", Color::Green),
            GrammarState::Missing => ("·", Color::DarkGray),
            GrammarState::Installing => ("⟳", Color::Yellow),
        };
        let is_sel = i == *selected;
        let row_style = if is_sel {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(format!(" {} ", glyph), Style::default().fg(glyph_color)),
            Span::raw(row.name.clone()),
        ];
        if row.state == GrammarState::Installing {
            spans.push(Span::styled(
                "  installing…",
                Style::default().fg(Color::Yellow),
            ));
        }
        lines.push(Line::from(spans).style(row_style));
    }

    // Pad so the footer sits on the bottom inner row even with a short
    // list.
    while lines.len() < list_h {
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(Paragraph::new(lines), inner);
}

/// First visible row index that keeps `selected` inside a `window`-tall
/// viewport. Clamps so the list never scrolls past its end.
fn scroll_offset(selected: usize, len: usize, window: usize) -> usize {
    if window == 0 || len <= window {
        return 0;
    }
    let max_scroll = len - window;
    // Keep the selection in view: scroll just enough when it falls below
    // the window, and clamp to the tail.
    selected.saturating_sub(window - 1).min(max_scroll)
}

#[cfg(test)]
mod tests {
    use super::scroll_offset;

    #[test]
    fn no_scroll_when_list_fits() {
        // 5 rows, 10-tall window — everything visible, never scrolls.
        assert_eq!(scroll_offset(0, 5, 10), 0);
        assert_eq!(scroll_offset(4, 5, 10), 0);
    }

    #[test]
    fn selection_inside_first_window_stays_at_top() {
        // window = 3; selecting rows 0..=2 keeps the top at 0.
        assert_eq!(scroll_offset(0, 10, 3), 0);
        assert_eq!(scroll_offset(2, 10, 3), 0);
    }

    #[test]
    fn selection_past_window_scrolls_just_enough() {
        // window = 3; row 3 is the first that needs a one-row scroll.
        assert_eq!(scroll_offset(3, 10, 3), 1);
        assert_eq!(scroll_offset(5, 10, 3), 3);
    }

    #[test]
    fn scroll_clamps_to_tail() {
        // Last row selected: top clamps to len - window, never beyond.
        assert_eq!(scroll_offset(9, 10, 3), 7);
    }

    #[test]
    fn zero_window_is_safe() {
        assert_eq!(scroll_offset(3, 10, 0), 0);
    }
}
