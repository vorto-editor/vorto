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
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::App;
use crate::prompt::{GrammarState, Prompt};

const MAX_WIDTH: u16 = 60;
const MAX_HEIGHT: u16 = 30;

pub(super) fn draw_grammar_list(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::GrammarList {
        rows,
        selected,
        query,
        filtering,
    } = &app.prompt.state
    else {
        return;
    };
    if area.width < 12 || area.height < 6 {
        return;
    }

    // Rows the filter currently lets through; `selected` indexes this.
    let visible = crate::prompt::grammar_visible_indices(rows, query);
    // A filter row appears at the top while the input is live or a query
    // is still narrowing the list (so the user sees why rows are hidden).
    let show_filter = *filtering || !query.is_empty();

    // One title + one footer line eat into the body, plus the border, plus
    // the optional filter row.
    let installed = rows
        .iter()
        .filter(|r| r.state == GrammarState::Installed)
        .count();
    let title = if show_filter {
        format!(
            " grammars · {} match{} · {}/{} installed ",
            visible.len(),
            if visible.len() == 1 { "" } else { "es" },
            installed,
            rows.len()
        )
    } else {
        format!(" grammars · {}/{} installed ", installed, rows.len())
    };
    let footer = "Enter install · d remove · j/k move · / filter · Esc close";

    let inner_w = (footer.len() as u16)
        .max(title.len() as u16)
        .clamp(20, MAX_WIDTH);
    let popup_w = (inner_w + 2).min(area.width.saturating_sub(2));

    // Box body holds (optionally) the filter row plus the list; +2 for the
    // border. The hint lives on its own line *below* the box, so it's not
    // counted here. Sized off the full catalog so the modal doesn't jitter
    // as the filter narrows it.
    let filter_h = u16::from(show_filter);
    let body_h = (rows.len() as u16).min(MAX_HEIGHT);
    let box_h = (body_h + 2 + filter_h).min(area.height);
    // Reserve one row under the box for the hint and center the pair, so
    // the box + hint sit together rather than the box alone.
    let total_h = (box_h + 1).min(area.height);

    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(total_h)) / 2;
    let popup = Rect {
        x,
        y,
        width: popup_w,
        height: box_h,
    };

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(title)
        .style(Style::default().bg(super::panel_bg()));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);

    // Optional filter row at the top: an active `/` prompt (with a block
    // cursor) while typing, or a passive dimmed indicator once the input
    // is dismissed but the query still filters.
    if show_filter {
        lines.push(filter_line(query, *filtering));
    }

    // The whole inner area below the filter row is the scrollable list.
    let list_h = (inner.height as usize).saturating_sub(filter_h as usize);
    let scroll = scroll_offset(*selected, visible.len(), list_h);

    for (pos, &row_idx) in visible.iter().enumerate().skip(scroll).take(list_h) {
        let row = &rows[row_idx];
        let (glyph, glyph_color) = match row.state {
            GrammarState::Installed => ("✓", Color::Green),
            GrammarState::Missing => ("·", Color::DarkGray),
            GrammarState::Installing => ("⟳", Color::Yellow),
        };
        let is_sel = pos == *selected;
        let row_style = if is_sel {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(format!(" {} ", glyph), Style::default().fg(glyph_color)),
            Span::raw(row.name.clone()),
        ];
        // Display width so far: " X " glyph cell (3) + the name.
        let mut used = 3 + row.name.chars().count();
        if row.state == GrammarState::Installing {
            let label = "  installing…";
            used += label.chars().count();
            spans.push(Span::styled(label, Style::default().fg(Color::Yellow)));
        }
        // Pad the selected row out to the full inner width so the reversed
        // highlight bar spans the whole row instead of stopping after the
        // text.
        if is_sel {
            let pad = (inner.width as usize).saturating_sub(used);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
        }
        lines.push(Line::from(spans).style(row_style));
    }

    // Empty filter result: tell the user nothing matched rather than
    // leaving a blank body.
    if visible.is_empty() && list_h > 0 {
        lines.push(Line::from(Span::styled(
            "  no matching grammars",
            Style::default().fg(Color::DarkGray),
        )));
    }

    f.render_widget(Paragraph::new(lines), inner);

    // Key hint on its own line just below the box (outside the border), if
    // there's room. Cleared first so it reads against the terminal bg
    // rather than the editor text behind the modal.
    let hint_y = y + box_h;
    if hint_y < area.y + area.height {
        let hint_rect = Rect {
            x,
            y: hint_y,
            width: popup_w,
            height: 1,
        };
        f.render_widget(Clear, hint_rect);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                footer,
                Style::default().fg(Color::DarkGray),
            ))),
            hint_rect,
        );
    }
}

/// The top filter row. While `filtering`, draws a yellow `/` prompt with
/// the live query and a reversed block cursor; once dismissed (query
/// still set), draws a dimmed `filter:` indicator instead.
fn filter_line(query: &str, filtering: bool) -> Line<'static> {
    if filtering {
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Yellow)),
            Span::raw(query.to_string()),
            Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
        ])
    } else {
        Line::from(Span::styled(
            format!(" filter: {query}"),
            Style::default().fg(Color::DarkGray),
        ))
    }
}

/// Small centered confirmation box for the open-time "install this
/// grammar?" prompt ([`Prompt::GrammarInstallConfirm`]). Renders the
/// message, a navigable Yes/No button pair (highlighting `accept`), and a
/// dim key hint, with generous padding inside the border. Key handling
/// lives in the prompt layer.
pub(super) fn draw_grammar_install_prompt(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::GrammarInstallConfirm {
        grammar,
        language,
        accept,
    } = &app.prompt.state
    else {
        return;
    };
    if area.width < 20 || area.height < 6 {
        return;
    }

    let msg = format!("No parser/queries for {language} ({grammar}).");
    let question = "Install it now?";
    let hint = "y/n · ←/→ select · Enter confirm · Esc cancel";

    // Highlight the selected button; dim the other. The brackets keep the
    // choice legible even where reverse video is subtle.
    let sel = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let unsel = Style::default().fg(Color::DarkGray);
    let (yes_style, no_style) = if *accept { (sel, unsel) } else { (unsel, sel) };
    let buttons = Line::from(vec![
        Span::styled("  Yes  ", yes_style),
        Span::raw("    "),
        Span::styled("  No  ", no_style),
    ])
    .centered();

    let lines = vec![
        Line::from(Span::raw(msg.clone())),
        Line::default(),
        Line::from(Span::raw(question)).centered(),
        Line::default(),
        buttons,
        Line::default(),
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))).centered(),
    ];

    // Padding: 2 cols each side, 1 row top/bottom. Width sizes off the
    // longest line; height off the body plus border + vertical padding.
    let (pad_x, pad_y) = (2u16, 1u16);
    let inner_w = (msg.len() as u16)
        .max(hint.len() as u16)
        .clamp(24, MAX_WIDTH);
    let popup_w = (inner_w + 2 * pad_x + 2).min(area.width.saturating_sub(2));
    let popup_h = (lines.len() as u16 + 2 * pad_y + 2).min(area.height);

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
        .border_style(Style::default().fg(Color::Yellow))
        .title(" install grammar ")
        .style(Style::default().bg(super::panel_bg()))
        .padding(Padding::new(pad_x, pad_x, pad_y, pad_y));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    f.render_widget(Paragraph::new(lines), inner);
}

/// First visible row index that keeps `selected` inside a `window`-tall
/// viewport. Clamps so the list never scrolls past its end. Shared with
/// the theme picker, which has the same scroll-a-list-in-a-box need.
pub(super) fn scroll_offset(selected: usize, len: usize, window: usize) -> usize {
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
