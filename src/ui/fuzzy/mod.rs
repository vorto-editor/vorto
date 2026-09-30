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

/// Popup width at or below which the preview pane is hidden — Helix's
/// `MIN_AREA_WIDTH_FOR_PREVIEW`.
const MIN_WIDTH_FOR_PREVIEW: u16 = 72;

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
        FuzzyKind::Jumps => " jumps ",
        FuzzyKind::WorkspaceSearch => " fuzzy: workspace ",
        FuzzyKind::Buffers => " fuzzy: buffers ",
        FuzzyKind::Diagnostics { workspace: false } => " diagnostics ",
        FuzzyKind::Diagnostics { workspace: true } => " diagnostics: workspace ",
        FuzzyKind::Bookmarks => " bookmarks ",
        FuzzyKind::GitChangedFiles => " git: changed files ",
    };
    // Panel bg + text fg from the active theme, so the picker matches the
    // editor background and its text stays legible (esp. on light themes).
    let panel = Style::default().bg(super::panel_bg()).fg(super::panel_fg());
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(title)
        .style(panel)
        .padding(Padding::horizontal(1));
    // The bookmark picker is an explorer-style modal: its key operations
    // live on the bottom border (left-aligned), and the query line only
    // appears while filtering. Both depend on the current Selection/Filter
    // mode.
    let bookmark_filtering = matches!(finder.kind, FuzzyKind::Bookmarks)
        && app.prompt.bookmark_mode() == crate::prompt::BookmarkPickMode::Filter;
    if matches!(finder.kind, FuzzyKind::Bookmarks) {
        // No query line in Selection mode, so the count moves to the
        // bottom border.
        if !bookmark_filtering {
            block = block
                .title_bottom(Line::from(format!(" {} ", finder.count_label())).right_aligned());
        }
        let hint = if bookmark_filtering {
            " type to filter · ↵ jump · esc back "
        } else {
            " j/k move · d delete · / filter · ↵ jump · esc close "
        };
        block = block.title_bottom(Line::from(hint));
    }
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    // Hide the query line in the bookmark picker's Selection mode — it
    // only makes sense while actually filtering. Every other picker types
    // to filter, so the query line is always shown.
    let show_query = !matches!(finder.kind, FuzzyKind::Bookmarks) || bookmark_filtering;

    // Narrow terminal: drop the preview and give the whole popup to the
    // list, same cut-off as Helix's picker.
    if popup.width <= MIN_WIDTH_FOR_PREVIEW {
        list::draw_fuzzy_list(f, finder, inner, show_query);
        return;
    }

    // Left: query + matches list. Right: source preview for the current
    // selection, split evenly like Helix. A vertical separator visually
    // divides the two panes.
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    list::draw_fuzzy_list(f, finder, panes[0], show_query);

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
