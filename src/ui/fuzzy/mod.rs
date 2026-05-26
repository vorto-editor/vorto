//! Fuzzy picker popup: the match list on the left, source preview on
//! the right. The preview reads through a per-`App` highlighter cache
//! so scrolling between matches in the same file is cheap.

mod list;
mod preview;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::{App, Prompt};
use crate::finder::FuzzyKind;

pub(super) fn draw_fuzzy(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::Fuzzy(finder) = &app.prompt.state else {
        return;
    };
    let popup = centered_rect(90, 80, area);
    f.render_widget(Clear, popup);

    let title = match finder.kind {
        FuzzyKind::Files { ignore } if !ignore.hidden => " fuzzy: files (+hidden) ",
        FuzzyKind::Files { .. } => " fuzzy: files ",
        FuzzyKind::Lines => " fuzzy: lines ",
        FuzzyKind::Locations => " references ",
        FuzzyKind::WorkspaceSearch => " fuzzy: workspace ",
        FuzzyKind::Buffers => " fuzzy: buffers ",
        FuzzyKind::Diagnostics { workspace: false } => " diagnostics ",
        FuzzyKind::Diagnostics { workspace: true } => " diagnostics: workspace ",
    };
    let total = finder.matches.len();
    let footer = format!(" {}/{} ", finder.selected + 1, total.max(1));
    // Panel bg + text fg from the active theme, so the picker matches the
    // editor background and its text stays legible (esp. on light themes).
    let panel = Style::default().bg(super::panel_bg()).fg(super::panel_fg());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(title)
        .title_bottom(Line::from(footer).right_aligned())
        .style(panel)
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Left: query + matches list. Right: source preview for the current
    // selection. A vertical separator visually divides the two panes.
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    list::draw_fuzzy_list(f, finder, panes[0]);

    let sep_v: Vec<Line> = (0..panes[1].height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::DarkGray))))
        .collect();
    f.render_widget(Paragraph::new(sep_v), panes[1]);

    // Belt-and-suspenders: the popup-level `Clear` above should already
    // have wiped panes[2], but in practice we still see syntax-
    // highlighted fragments from prior preview renders leak through
    // when navigating between files of different lengths. Clearing the
    // preview rect explicitly here defends against whichever cell ends
    // up not being touched by the new render.
    f.render_widget(Clear, panes[2]);
    // Re-tint the just-cleared preview rect with the panel bg so it keeps
    // the theme background under the (fg-only) highlighted source.
    f.render_widget(
        Block::default().style(Style::default().bg(super::panel_bg())),
        panes[2],
    );
    preview::draw_fuzzy_preview(f, app, finder, panes[2]);
}

/// Reuse the fuzzy picker's file-preview pipeline (per-`App` LRU +
/// preview worker) for the explorer's preview pane. Lets the tree view
/// share warmed highlights with `<space>f` instead of reimplementing
/// the same path.
pub(super) fn draw_explorer_preview(f: &mut Frame, app: &App, area: Rect, path: &std::path::Path) {
    preview::preview_from_file(f, app, area, path, 0);
}

/// Reserve a 1-row header (the file label, tail-truncated with `…` when
/// it can't fit) plus a 1-row separator at the top of `area`, returning
/// the remaining body Rect for the actual preview content. The fuzzy
/// preview and the explorer preview both use this so the "filename on
/// top, content below" layout stays consistent.
pub(super) fn split_with_header(
    f: &mut Frame,
    area: Rect,
    label: &str,
    label_style: Style,
) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(area);
    let label_w = label.chars().count();
    let avail = chunks[0].width as usize;
    let display: String = if label_w > avail && avail >= 2 {
        let tail: String = label.chars().skip(label_w - (avail - 1)).collect();
        format!("…{}", tail)
    } else {
        label.to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(display, label_style))),
        chunks[0],
    );
    let sep = "─".repeat(chunks[1].width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(sep, Style::default().fg(Color::DarkGray))),
        chunks[1],
    );
    chunks[2]
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1])[1]
}
