//! Screen-centered modal for the Copilot device-flow signin.
//!
//! Stays visible (unlike the toast queue) so the verification URL and
//! user code don't scroll away while the user is in the browser. The
//! user code is already on the OS clipboard by the time this draws —
//! the modal is just a stable surface to read it from.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::{App, Prompt};
use crate::text_width::str_cell_width;

const MIN_INNER_W: u16 = 40;
const MAX_INNER_W: u16 = 78;

pub(super) fn draw_copilot_signin(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::CopilotSignin { code, url } = &app.prompt.state else {
        return;
    };
    if area.width < 8 || area.height < 6 {
        return;
    }

    let bg = Style::default().bg(super::panel_bg());
    let label = Style::default().bg(super::panel_bg()).fg(Color::Gray);
    let value = Style::default()
        .bg(super::panel_bg())
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().bg(super::panel_bg()).fg(Color::DarkGray);

    let body: Vec<Line> = vec![
        Line::from(Span::styled(
            "GitHub Copilot — device flow signin",
            Style::default()
                .bg(super::panel_bg())
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("", bg)),
        Line::from(vec![
            Span::styled("code:  ", label),
            Span::styled(code.clone(), value),
            Span::styled("   (copied to clipboard)", dim),
        ]),
        Line::from(vec![
            Span::styled("url:   ", label),
            Span::styled(url.clone(), value),
        ]),
        Line::from(Span::styled("", bg)),
        Line::from(Span::styled(
            "Paste the code at the URL in your browser, then return here.",
            label,
        )),
        Line::from(Span::styled(
            "Signin completes in the background — any key dismisses this modal.",
            dim,
        )),
        Line::from(Span::styled(
            "Re-show with :copilot code if the clipboard gets overwritten.",
            dim,
        )),
    ];

    let longest = body
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| str_cell_width(s.content.as_ref()))
                .sum::<usize>() as u16
        })
        .max()
        .unwrap_or(MIN_INNER_W);
    let inner_w = longest.clamp(MIN_INNER_W, MAX_INNER_W);
    let popup_w = (inner_w + 2 + 2).min(area.width.saturating_sub(2));
    let popup_h = (body.len() as u16 + 2 + 2).min(area.height);

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
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(" copilot signin ")
        .style(bg)
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    f.render_widget(Paragraph::new(body).style(bg), inner);
}
