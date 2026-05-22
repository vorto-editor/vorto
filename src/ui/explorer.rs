//! Tree file-explorer popup. The layout mirrors the fuzzy picker
//! (centered popup, query line on top, scrollable list on the left,
//! source preview pane on the right) so users get a familiar widget
//! shape — but the list itself is the tree projection over
//! [`ExplorerState::visible`] rather than a fuzzy match set.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph};

use crate::app::{App, Prompt};
use crate::finder::ExplorerState;

/// Color the dir glyph and dir name share — matches the fuzzy picker's
/// directory tint so the two widgets read as siblings.
const DIR_FG: Color = Color::Blue;
/// Indent step (cells) per tree depth. Two-space indent is the same
/// step the indent guides use for content panes.
const INDENT_STEP: usize = 2;

pub(super) fn draw_explorer(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::Explorer(state) = &app.prompt.state else {
        return;
    };
    let popup = centered_rect(90, 80, area);
    f.render_widget(Clear, popup);

    let total = state.visible.len();
    let position = if total == 0 { 0 } else { state.selected + 1 };
    let footer = format!(" {}/{} ", position, total.max(1));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::PANEL_BORDER_FG))
        .title(" explorer ")
        .title_bottom(Line::from(footer).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    draw_tree(f, state, panes[0]);

    let sep_v: Vec<Line> = (0..panes[1].height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::DarkGray))))
        .collect();
    f.render_widget(Paragraph::new(sep_v), panes[1]);

    // Belt-and-suspenders: wipe the preview rect so leftover cells from
    // a previous frame's preview (different file with longer lines)
    // don't shine through past the new content. Same reasoning as the
    // fuzzy popup.
    f.render_widget(Clear, panes[2]);
    draw_preview(f, app, state, panes[2]);
}

fn draw_tree(f: &mut Frame, state: &ExplorerState, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(area);

    // Query line — single source of truth for the filter; even though
    // selected nodes can be expanded with Enter, typing also flows
    // straight into the query buffer.
    let query_line = Line::from(vec![
        Span::styled("› ", Style::default().fg(Color::Yellow)),
        Span::raw(state.query.clone()),
    ]);
    f.render_widget(Paragraph::new(query_line), chunks[0]);

    // Park the terminal cursor at the insertion point so backspace
    // visibly lands. `› ` is two single-cell glyphs.
    let col = (2 + state.cursor) as u16;
    let x = chunks[0].x + col.min(chunks[0].width.saturating_sub(1));
    f.set_cursor_position((x, chunks[0].y));

    let sep = "─".repeat(chunks[1].width as usize);
    f.render_widget(
        Paragraph::new(Span::styled(sep, Style::default().fg(Color::DarkGray))),
        chunks[1],
    );

    let list_h = chunks[2].height as usize;
    let list_w = chunks[2].width as usize;
    // Selection-anchored scroll: keep `selected` at the bottom of the
    // window once we'd otherwise scroll past, and snap to the very top
    // until the cursor leaves the first page. Same heuristic the fuzzy
    // picker uses — there's no separate scroll input the user can move
    // independently, so deriving from selection each frame is correct.
    let scroll = state.selected.saturating_sub(list_h.saturating_sub(1));
    let items: Vec<ListItem> = state
        .visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(list_h)
        .map(|(i, &node_idx)| {
            let node = &state.nodes[node_idx];
            let selected = i == state.selected;
            let expanded = node.is_dir && state.expanded.contains(&node.rel_path);
            ListItem::new(render_row(
                node.depth,
                &node.name,
                node.is_dir,
                expanded,
                selected,
                list_w,
            ))
        })
        .collect();
    f.render_widget(List::new(items), chunks[2]);
}

/// One row in the tree pane: indent, expand/collapse glyph (dirs
/// only), basename. Selected rows get a dark background so the cursor
/// stays visible; dir rows get the directory tint regardless of
/// selection so the user can tell apart files and folders at a glance.
fn render_row<'a>(
    depth: usize,
    name: &str,
    is_dir: bool,
    expanded: bool,
    selected: bool,
    width: usize,
) -> Line<'a> {
    let base = if selected {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let dir = base.fg(DIR_FG).add_modifier(Modifier::BOLD);
    let glyph_style = base.fg(Color::DarkGray);
    let ellipsis_style = base.fg(Color::DarkGray);

    let indent = INDENT_STEP * depth;
    // Each row: indent spaces, then "▾ " / "▸ " for dirs (2 cells) or
    // "  " for files. Limit by `width` so a deep tree doesn't blow up
    // the right margin.
    let glyph = if is_dir {
        if expanded { "▾ " } else { "▸ " }
    } else {
        "  "
    };
    let glyph_cells = 2;
    let prefix_w = indent + glyph_cells;

    let mut spans: Vec<Span<'a>> = Vec::new();
    if indent > 0 {
        spans.push(Span::styled(" ".repeat(indent), base));
    }
    spans.push(Span::styled(
        glyph.to_string(),
        if is_dir { dir } else { glyph_style },
    ));

    let name_chars: Vec<char> = name.chars().collect();
    let budget = width.saturating_sub(prefix_w);
    // For dirs we want the trailing `/` to read like a path; reserve
    // a cell for it when we're laying out the name.
    let suffix = if is_dir { 1 } else { 0 };
    let name_budget = budget.saturating_sub(suffix);

    let (start, lead) = if name_budget >= 2 && name_chars.len() > name_budget {
        (name_chars.len() - (name_budget - 1), Some("…"))
    } else {
        (0, None)
    };
    if let Some(e) = lead {
        spans.push(Span::styled(e, ellipsis_style));
    }
    let visible: String = name_chars[start..].iter().collect();
    spans.push(Span::styled(visible, if is_dir { dir } else { base }));
    if is_dir {
        spans.push(Span::styled("/", dir));
    }

    // Pad to the right edge with the row's base background so the
    // selection bar reaches the separator instead of stopping at the
    // end of the name.
    let written = prefix_w + (lead.is_some() as usize) + (name_chars.len() - start) + suffix;
    if written < width {
        spans.push(Span::styled(" ".repeat(width - written), base));
    }
    Line::from(spans)
}

fn draw_preview(f: &mut Frame, app: &App, state: &ExplorerState, area: Rect) {
    let Some(node) = state.selection() else {
        return;
    };
    if node.is_dir {
        // No useful preview for a directory row; just leave it blank.
        return;
    }
    // Defer to the fuzzy preview's file path — shares the same LRU /
    // worker that already warms previews for `<space>f`. We construct
    // the absolute path the same way `FuzzyKind::Files` does.
    let path = app.startup_cwd.join(&node.rel_path);
    super::fuzzy::draw_explorer_preview(f, app, area, &path);
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
