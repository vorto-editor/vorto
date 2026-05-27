//! Small centered confirmation box for the autoreload watcher's
//! "file changed on disk — reload?" prompt ([`Prompt::FileReloadConfirm`]).
//!
//! Mirrors [`super::grammar_list::draw_grammar_install_prompt`]: a
//! message, a navigable Yes/No button pair (highlighting `accept`), and a
//! dim key hint. When the buffer had unsaved edits at detection time, an
//! extra warning line spells out that reloading replaces them (recoverable
//! with `u`). Key handling lives in the prompt layer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::App;
use crate::prompt::Prompt;

const MAX_WIDTH: u16 = 60;

pub(super) fn draw_file_reload_prompt(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::FileReloadConfirm {
        path,
        dirty,
        accept,
    } = &app.prompt.state
    else {
        return;
    };
    if area.width < 20 || area.height < 6 {
        return;
    }

    // Show just the file name — the full path can be long and the box
    // sizes off the longest line.
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| path.to_str().unwrap_or("file"));
    let msg = format!("{name} changed on disk.");
    let question = "Reload it?";
    let warn = "Unsaved edits will be replaced (u to undo).";
    let hint = "y/n · ←/→ select · Enter confirm · Esc cancel";

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

    let mut lines = vec![Line::from(Span::raw(msg.clone()))];
    if *dirty {
        lines.push(Line::from(Span::styled(
            warn,
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::raw(question)).centered());
    lines.push(Line::default());
    lines.push(buttons);
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))).centered());

    let (pad_x, pad_y) = (2u16, 1u16);
    // The warning line is only drawn when dirty, so only let it widen the
    // box then — otherwise a clean-buffer prompt sizes off the longest
    // line that's actually present.
    let warn_w = if *dirty { warn.len() as u16 } else { 0 };
    let inner_w = (msg.len() as u16)
        .max(warn_w)
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
        .title(" file changed ")
        .style(Style::default().bg(super::panel_bg()))
        .padding(Padding::new(pad_x, pad_x, pad_y, pad_y));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    f.render_widget(Paragraph::new(lines), inner);
}
