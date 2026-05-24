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
    // Viewport-style vertical scroll: the cursor moves freely inside the
    // visible page; the page only shifts when the cursor would step off
    // either edge. We clamp the stored offset against the live `visible`
    // length first so a refilter that shrinks the list doesn't strand the
    // viewport past the end.
    let mut scroll = state.scroll.get();
    let max_scroll = state.visible.len().saturating_sub(list_h);
    if scroll > max_scroll {
        scroll = max_scroll;
    }
    if list_h > 0 && state.selected >= scroll + list_h {
        scroll = state.selected + 1 - list_h;
    }
    if state.selected < scroll {
        scroll = state.selected;
    }
    state.scroll.set(scroll);

    // Horizontal scroll: judged off the *parent directory* row, not the
    // selected file. If the parent's "indent + glyph + name + /" fits in
    // the pane, we don't scroll at all — even a long file basename can
    // stay clipped, since the preview header below carries the full
    // path. When the parent overflows, scroll just enough to fit it,
    // capped at `(depth - 1) * INDENT_STEP` so the parent row is pinned
    // at column 0 in the worst case.
    let h_scroll = state
        .visible
        .get(state.selected)
        .map(|&i| {
            let n = &state.nodes[i];
            if n.depth == 0 {
                return 0;
            }
            let parent_name = parent_basename(&n.rel_path);
            let parent_total = INDENT_STEP * (n.depth - 1) + 2 + parent_name.chars().count() + 1;
            let needed = parent_total.saturating_sub(list_w);
            let ceiling = INDENT_STEP * (n.depth - 1);
            needed.min(ceiling)
        })
        .unwrap_or(0);
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
                h_scroll,
            ))
        })
        .collect();
    f.render_widget(List::new(items), chunks[2]);
}

/// One row in the tree pane: indent, expand/collapse glyph (dirs
/// only), basename. Selected rows get a dark background so the cursor
/// stays visible; dir rows get the directory tint regardless of
/// selection so the user can tell apart files and folders at a glance.
///
/// `h_scroll` shifts every row's content left by that many cells before
/// clipping to `width`. The caller picks it from the selected row so the
/// cursor's file name is guaranteed visible; deeper neighbors will have
/// their indent (and possibly the start of their name) clipped off, but
/// the trade-off is what makes deep trees navigable.
fn render_row<'a>(
    depth: usize,
    name: &str,
    is_dir: bool,
    expanded: bool,
    selected: bool,
    width: usize,
    h_scroll: usize,
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

    let indent = INDENT_STEP * depth;
    let glyph = if is_dir {
        if expanded { "▾ " } else { "▸ " }
    } else {
        "  "
    };

    // Materialize the row as (char, style) cells, then window it to
    // [h_scroll, h_scroll+width). Building the full row first keeps the
    // scroll math trivial — one assumption per cell rather than juggling
    // span boundaries.
    let name_chars: Vec<char> = name.chars().collect();
    let mut cells: Vec<(char, Style)> =
        Vec::with_capacity(indent + 2 + name_chars.len() + if is_dir { 1 } else { 0 });
    for _ in 0..indent {
        cells.push((' ', base));
    }
    let gstyle = if is_dir { dir } else { glyph_style };
    for c in glyph.chars() {
        cells.push((c, gstyle));
    }
    let nstyle = if is_dir { dir } else { base };
    for c in &name_chars {
        cells.push((*c, nstyle));
    }
    if is_dir {
        cells.push(('/', dir));
    }

    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    let mut written = 0usize;
    for (c, st) in cells.into_iter().skip(h_scroll).take(width) {
        if cur != Some(st) {
            if let Some(prev) = cur {
                spans.push(Span::styled(std::mem::take(&mut buf), prev));
            }
            cur = Some(st);
        }
        buf.push(c);
        written += 1;
    }
    if let Some(st) = cur
        && !buf.is_empty()
    {
        spans.push(Span::styled(buf, st));
    }
    if written < width {
        spans.push(Span::styled(" ".repeat(width - written), base));
    }
    Line::from(spans)
}

fn draw_preview(f: &mut Frame, app: &App, state: &ExplorerState, area: Rect) {
    let Some(node) = state.selection() else {
        return;
    };

    // Header carries the full relative path of the current selection.
    // The tree pane scrolls the indent off-screen so a deep selection's
    // basename may be clipped there; the header is the unambiguous label
    // that tells the user what they're about to open.
    let header_style = if node.is_dir {
        Style::default().fg(DIR_FG).add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let mut label = node.rel_path.clone();
    if node.is_dir {
        label.push('/');
    }
    let body = super::fuzzy::split_with_header(f, area, &label, header_style);

    if node.is_dir {
        // No useful preview for a directory row; the header line above
        // already says which dir it is.
        return;
    }
    // Defer to the fuzzy preview's file path — shares the same LRU /
    // worker that already warms previews for `<space>f`. We construct
    // the absolute path the same way `FuzzyKind::Files` does.
    let path = app.startup_cwd.join(&node.rel_path);
    super::fuzzy::draw_explorer_preview(f, app, body, &path);
}

/// Extract the parent directory's basename from a relative path.
/// `"src/ui/baz.rs"` → `"ui"`. Top-level entries (no slash) return `""`,
/// matching the depth-0 short-circuit in the scroll math.
fn parent_basename(rel_path: &str) -> &str {
    let trimmed = rel_path.trim_end_matches('/');
    let parent = match trimmed.rfind('/') {
        Some(i) => &trimmed[..i],
        None => return "",
    };
    parent.rsplit('/').next().unwrap_or("")
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
