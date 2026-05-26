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
use crate::finder::{ExplorerMode, ExplorerState};

/// Color the dir glyph and dir name share — matches the fuzzy picker's
/// directory tint so the two widgets read as siblings.
const DIR_FG: Color = Color::Blue;
/// Indent step (cells) per tree depth. Two-space indent is the same
/// step the indent guides use for content panes.
const INDENT_STEP: usize = 2;

/// True when the tree pane should reserve a row at the top for the
/// filter input line: either Filter mode is active, or the user has a
/// query that's still narrowing the visible set. The pending modes
/// (create/delete/rename/move) get their own modal overlay and don't
/// influence the tree header.
fn filter_header_visible(state: &ExplorerState) -> bool {
    matches!(state.mode, ExplorerMode::Filter) || !state.query.is_empty()
}

/// Filter-only header. Called when [`filter_header_visible`] is true.
/// Returns the line and the cursor column when Filter mode owns the
/// input; in Selection mode with a non-empty query the line is a
/// passive "filter: foo" indicator and the cursor is `None`.
fn filter_header(state: &ExplorerState) -> (Line<'_>, Option<usize>) {
    if matches!(state.mode, ExplorerMode::Filter) {
        let line = Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Yellow)),
            Span::raw(state.query.clone()),
        ]);
        (line, Some(2 + state.cursor))
    } else {
        // Selection mode with an active query — passive indicator.
        let line = Line::from(vec![
            Span::styled("filter: ", Style::default().fg(Color::DarkGray)),
            Span::styled(state.query.clone(), Style::default().fg(Color::Gray)),
        ]);
        (line, None)
    }
}

pub(super) fn draw_explorer(f: &mut Frame, app: &App, area: Rect) {
    let Prompt::Explorer(state) = &app.prompt.state else {
        return;
    };
    let popup = centered_rect(90, 80, area);
    f.render_widget(Clear, popup);

    let total = state.visible.len();
    let position = if total == 0 { 0 } else { state.selected + 1 };
    let footer = format!(" {}/{} ", position, total.max(1));
    let panel = Style::default().bg(super::panel_bg()).fg(super::panel_fg());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(" explorer ")
        .title_bottom(Line::from(footer).right_aligned())
        .style(panel)
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
    f.render_widget(
        Block::default().style(Style::default().bg(super::panel_bg())),
        panes[2],
    );
    draw_preview(f, app, state, panes[2]);

    // Pending file-op modals (add / delete / rename / move) float on
    // top of the explorer popup so the tree underneath stays visible
    // for context — the user can see what they're acting on while
    // typing the new name.
    draw_action_modal(f, state, area);
}

/// Render a small modal box for the pending action modes. The box
/// floats over the explorer popup; the title (`add`/`delete`/`rename`/
/// `move`) and body change per mode. No-op for [`Selection`] /
/// [`Filter`].
///
/// [`Selection`]: ExplorerMode::Selection
/// [`Filter`]: ExplorerMode::Filter
fn draw_action_modal(f: &mut Frame, state: &ExplorerState, area: Rect) {
    let Some((title, body_lines, cursor)) = action_modal_content(state) else {
        return;
    };
    let modal = centered_action_rect(area);
    f.render_widget(Clear, modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(super::panel_border_fg()))
        .title(format!(" {title} "))
        .style(Style::default().bg(super::panel_bg()).fg(super::panel_fg()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(modal);
    f.render_widget(block, modal);
    f.render_widget(Paragraph::new(body_lines), inner);
    if let Some((row, col)) = cursor {
        let x = inner.x + (col as u16).min(inner.width.saturating_sub(1));
        let y = inner.y + row as u16;
        f.set_cursor_position((x, y));
    }
}

/// `(title, body lines, optional (row, col) for the terminal cursor)`
/// returned by [`action_modal_content`].
type ActionModalContent<'a> = (&'static str, Vec<Line<'a>>, Option<(usize, usize)>);

/// Translate the explorer state into modal content. Returns the
/// content tuple, or `None` when no modal should be drawn.
fn action_modal_content(state: &ExplorerState) -> Option<ActionModalContent<'_>> {
    let target_label = state
        .selection()
        .map(|n| {
            if n.is_dir {
                format!("{}/", n.rel_path)
            } else {
                n.rel_path.clone()
            }
        })
        .unwrap_or_default();
    let err_line = state.error.as_deref().map(|e| {
        Line::from(Span::styled(
            format!("error: {e}"),
            Style::default().fg(Color::Red),
        ))
    });

    match state.mode {
        ExplorerMode::Selection | ExplorerMode::Filter => None,
        ExplorerMode::PendingCreate => {
            let input = state.action.as_ref()?;
            let mut lines = vec![
                Line::from(Span::styled(
                    "new path (trailing `/` for directory)",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::raw(input.text.clone())),
            ];
            if let Some(e) = err_line {
                lines.push(e);
            }
            // The input lives on row 1 inside the modal body.
            Some(("add", lines, Some((1, input.cursor))))
        }
        ExplorerMode::PendingRename => {
            let input = state.action.as_ref()?;
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("rename ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        target_label.clone(),
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::raw(input.text.clone())),
            ];
            if let Some(e) = err_line {
                lines.push(e);
            }
            Some(("rename", lines, Some((1, input.cursor))))
        }
        ExplorerMode::PendingMove => {
            let input = state.action.as_ref()?;
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("move ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        target_label.clone(),
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(Span::raw(input.text.clone())),
            ];
            if let Some(e) = err_line {
                lines.push(e);
            }
            Some(("move", lines, Some((1, input.cursor))))
        }
        ExplorerMode::PendingDelete => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("delete ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        target_label,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("?", Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(Span::styled(
                    "[y]es / [N]o",
                    Style::default().fg(Color::Gray),
                )),
            ];
            if let Some(e) = err_line {
                lines.push(e);
            }
            Some(("delete", lines, None))
        }
    }
}

/// Centered rectangle for the action modal. Sized to fit the title +
/// label + input + optional error (4 rows of body + 2 for borders), and
/// to stay narrow enough that the tree underneath remains visible at
/// the edges. Clamped against the surrounding area so small terminals
/// still get a usable modal.
fn centered_action_rect(area: Rect) -> Rect {
    let width = area.width.saturating_mul(60) / 100;
    let width = width.clamp(30, area.width.saturating_sub(2)).max(10);
    let height = 6u16.min(area.height.saturating_sub(2)).max(3);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn draw_tree(f: &mut Frame, state: &ExplorerState, area: Rect) {
    // The header strip (filter input + separator) only exists when a
    // filter is active or being entered. Selection mode with no query
    // gets the full pane for the tree, so the explorer doesn't pay a
    // two-row tax for a widget the user isn't using.
    let show_header = filter_header_visible(state);
    let constraints: Vec<Constraint> = if show_header {
        vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ]
    } else {
        vec![Constraint::Min(1)]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let list_rect = if show_header {
        let (line, cursor_col) = filter_header(state);
        f.render_widget(Paragraph::new(line), chunks[0]);
        if let Some(col) = cursor_col {
            let x = chunks[0].x + (col as u16).min(chunks[0].width.saturating_sub(1));
            f.set_cursor_position((x, chunks[0].y));
        }
        let sep = "─".repeat(chunks[1].width as usize);
        f.render_widget(
            Paragraph::new(Span::styled(sep, Style::default().fg(Color::DarkGray))),
            chunks[1],
        );
        chunks[2]
    } else {
        chunks[0]
    };

    let list_h = list_rect.height as usize;
    let list_w = list_rect.width as usize;
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
    f.render_widget(List::new(items), list_rect);
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
