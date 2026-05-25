//! Screen-centered modal for the `:agent` picker.
//!
//! Shown when a bare `:agent` runs with no configured default: a short
//! selection list of agent names. A trimmed-down sibling of
//! [`super::grammar_list`] — no filter, no per-row state glyphs, since
//! the catalog is small and every entry is launchable. Key handling
//! lives in the prompt layer; this module only paints.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::prompt::Prompt;

const MAX_WIDTH: u16 = 50;
const MAX_HEIGHT: u16 = 20;

pub(super) fn draw_agent_picker(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::AgentPicker { agents, selected } = &app.prompt.state else {
        return;
    };
    if area.width < 12 || area.height < 6 {
        return;
    }

    let title = " launch agent ";
    let footer = "Enter launch · j/k move · Esc cancel";

    let widest_row = agents
        .iter()
        .map(|a| a.chars().count() + 3)
        .max()
        .unwrap_or(0);
    let inner_w = (footer.len())
        .max(title.len())
        .max(widest_row)
        .clamp(20, MAX_WIDTH as usize) as u16;
    let popup_w = (inner_w + 2).min(area.width.saturating_sub(2));

    let body_h = (agents.len() as u16).min(MAX_HEIGHT);
    let box_h = (body_h + 2).min(area.height);
    // Box + a hint line below it, centered as a pair.
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
        .border_style(Style::default().fg(super::PANEL_BORDER_FG))
        .title(title)
        .style(Style::default().bg(super::PANEL_BG));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::with_capacity(agents.len());
    for (i, name) in agents.iter().enumerate() {
        let is_sel = i == *selected;
        let marker = if is_sel { "▸ " } else { "  " };
        let mut spans = vec![Span::raw(marker), Span::raw(name.clone())];
        if is_sel {
            // Pad out so the highlight bar spans the full inner width.
            let used = 2 + name.chars().count();
            let pad = (inner.width as usize).saturating_sub(used);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
        }
        let style = if is_sel {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(spans).style(style));
    }
    f.render_widget(Paragraph::new(lines), inner);

    // Hint line just below the box (outside the border), space permitting.
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
