//! Buffer viewport: gutter (diagnostic signs + line numbers),
//! per-character syntax highlighting layered with the visual selection,
//! and the terminal cursor placement that goes with it.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::config::EditorConfig;
use crate::editor::Editor;
use crate::editor::fold::FoldState;
use crate::text_width::visual_col_of;

mod diagnostics;
mod indent_guides;
mod render_line;
mod scroll;

use diagnostics::{build_row_diag_summary, build_row_severity, diagnostic_line};
use indent_guides::{GuideMap, IndentGuide, compute_indent_guides};
use render_line::{
    bookmark_sign_span, build_jump_overlay, conflict_captures, find_matches_in_line, render_line,
    sign_span, vcs_bar_span,
};
use scroll::{compute_col_scroll, compute_scroll};

/// Style patched onto visually-selected text — the active theme's
/// `ui.selection`, or ANSI bright-black bg (color 8, follows the user's
/// terminal theme) when the theme doesn't set one.
pub(super) fn sel_style() -> Style {
    crate::theme::active().ui_selection()
}

/// Style patched onto every visible match of the active search pattern
/// (vim's `hlsearch`) — the active theme's `ui.search`, defaulting to
/// ANSI bright-black bg so it sits under text without competing with a
/// visual selection.
pub(super) fn search_style() -> Style {
    crate::theme::active().ui_search()
}

/// Background used to render each extra-cursor cell. ANSI regular
/// yellow (palette slot 3) so the cell picks up the user's terminal
/// theme — typically a muted mustard/ochre on dark themes, clearly
/// darker than bright yellow (LightYellow, slot 11). Distinct in hue
/// from `SEL_BG` and `SEARCH_HIT_BG` (DarkGray), so a stacked cursor
/// remains visible inside a selection or a search match.
pub(super) const EXTRA_CURSOR_BG: Color = Color::Yellow;

/// Foreground paired with `EXTRA_CURSOR_BG`. ANSI black so the glyph
/// stays legible against the yellow cell across themes, regardless of
/// the base style's fg.
pub(super) const EXTRA_CURSOR_FG: Color = Color::Black;

/// Foreground used to mark the bracket pair when the cursor sits on
/// one half of `()`, `[]`, or `{}`. Combined with `BOLD` rather than a
/// background fill so the highlight remains legible on top of any
/// other layer (search hit, selection, syntax bg) without competing
/// for the same channel.
pub(super) const MATCH_BRACKET_FG: Color = Color::Yellow;

/// Foreground used for `gw` jump labels. Bright magenta on a near-black
/// background so the label always pops over surrounding syntax.
pub(super) const JUMP_LABEL_FG: Color = Color::Rgb(255, 100, 200);
pub(super) const JUMP_LABEL_BG: Color = Color::Rgb(40, 0, 40);

/// Foreground used for the whitespace marker glyphs (middle-dot and
/// tab arrow) when `show_whitespace` is enabled. Dim enough to fade
/// into the background but still legible.
pub(super) const WHITESPACE_FG: Color = Color::DarkGray;

/// Foreground used for inactive indent-guide bars. ANSI bright-black so
/// the guides pick up the user's terminal theme and stay readable as
/// structural hints without competing with code.
pub(super) const INDENT_GUIDE_FG: Color = Color::DarkGray;

/// Default glyph for indent-guide cells. Light vertical box-drawing
/// line. Used as the per-cell glyph unless a specific guide
/// (`p10k` corners/arrow) carries its own.
pub(super) const INDENT_GUIDE_CHAR: char = '│';

/// Width of the gutter prefix (severity sign + space). Kept in sync with
/// [`place_cursor`] so the cursor lands on the right column.
pub(super) const GUTTER_SIGN_WIDTH: u16 = 1;

/// Width of the VCS-bar column rendered between the line number and the
/// buffer text. One cell wide regardless of status — the bar character
/// itself is single-width.
pub(super) const GUTTER_VCS_WIDTH: u16 = 1;

/// Minimum rows kept above and below the cursor inside the viewport
/// (vim's `scrolloff`). Near the end of the file this lets scroll
/// advance past the last source row, leaving blank rows below — so the
/// cursor isn't pinned to the bottom edge when sitting on the last few
/// lines. Disabled automatically when the viewport is too small to
/// give the cursor room (height ≤ 2 * SCROLL_OFF + 1).
pub(super) const SCROLL_OFF: usize = 5;

/// Resolve foldable `regions` against a view's collapse state into the
/// per-frame fold view: the set of rows hidden by collapsed folds and a
/// `header → end_row` map for drawing fold markers. Both are empty when
/// nothing is collapsed.
fn build_fold_view(
    regions: &[(usize, usize)],
    folds: &FoldState,
) -> (HashSet<usize>, HashMap<usize, usize>) {
    let mut hidden = HashSet::new();
    let mut header_end = HashMap::new();
    for &(h, e) in regions {
        if folds.is_collapsed(h) {
            hidden.extend((h + 1)..=e);
            header_end.insert(h, e);
        }
    }
    (hidden, header_end)
}

pub(super) fn draw_buffer(f: &mut Frame, app: &App, area: Rect) {
    let height = area.height as usize;
    let row_diag = build_row_diag_summary(app, app.editor.cursor.row);
    // Fold view first: `compute_scroll` weights hidden rows as zero, and
    // the render loop skips them. Computing regions only when something
    // is collapsed keeps the indent-fallback scan off the all-open hot
    // path.
    let (hidden, header_end) = if app.editor.folds().is_empty() {
        (HashSet::new(), HashMap::new())
    } else {
        let regions = app.fold_regions();
        build_fold_view(&regions, app.editor.folds())
    };
    let scroll = compute_scroll(app, height, &row_diag, &hidden);

    let sel = app.selection();
    // Last document row the viewport can reach. Hidden rows consume no
    // screen row, so with collapsed folds the viewport spans *more*
    // document rows than `height` — the per-row data windows below
    // (syntax captures, indent guides, diagnostic severity) must cover
    // all of them or the bottom of the screen renders bare. Walk from
    // `scroll`, skipping hidden rows, until `height` visible rows are
    // accounted for. Falls back to the cheap `scroll + height` when
    // nothing is folded.
    let last_visible = if hidden.is_empty() {
        scroll + height
    } else {
        let total = app.active_doc().lines.len();
        let mut shown = 0usize;
        let mut row = scroll;
        while row < total && shown < height {
            if !hidden.contains(&row) {
                shown += 1;
            }
            row += 1;
        }
        row.max(scroll + 1)
    };
    let mut captures = app
        .active_doc()
        .highlighter
        .as_ref()
        .map(|h| h.captures_in_rows(scroll, last_visible))
        .unwrap_or_default();
    // Conflict-marker captures layer on top of (after) the syntax
    // captures so their styles win on those rows. Hunks come from the
    // buffer's version-cached parse, so this is free on a hot cache.
    let conflict_hunks = app.active_doc().conflict_hunks();
    captures.extend(conflict_captures(
        &app.active_doc().lines,
        &conflict_hunks,
        scroll,
        last_visible,
    ));
    let row_severity = build_row_severity(app, scroll, last_visible);
    // Rows of the active buffer that carry a harpoon bookmark — drawn
    // with a sign-column dot (taking priority over the diagnostic sign).
    let bookmark_rows: std::collections::HashSet<usize> = app
        .bookmarks
        .marks
        .iter()
        .filter(|m| m.target == app.editor.doc)
        .map(|m| m.line)
        .collect();
    let vcs_statuses = app.active_doc().vcs_statuses();
    let cursor_row = app.editor.cursor.row;
    let cursor_col = app.editor.cursor.col;
    let extras = &app.editor.extra_cursors;
    let search_query = &app.search.query;
    let jump_overlay = build_jump_overlay(app.jump_state.as_ref());
    // Tree-sitter–driven matching-pair highlight. Yields the two cells
    // to paint (cursor's pair half + its mate) when the cursor sits on
    // a syntactic bracket or quote; brackets inside strings/comments
    // resolve to the containing literal node and naturally don't match
    // here.
    let bracket_pair: Vec<(usize, usize)> = app
        .active_doc()
        .highlighter
        .as_ref()
        .and_then(|h| h.matching_bracket(cursor_row, cursor_col))
        .map(|mate| vec![(cursor_row, cursor_col), mate])
        .unwrap_or_default();
    let eff = app.effective_editor();
    let tab_width = eff.tab_width.max(1);
    let show_whitespace = eff.show_whitespace;
    let indent_guides = if eff.indent_guides {
        compute_indent_guides(
            app,
            scroll,
            last_visible,
            tab_width,
            eff.indent_width,
            eff.indent_guides_skip_levels,
            eff.indent_guide_style,
            eff.indent_animation,
            eff.indent_animation_ms,
        )
    } else {
        GuideMap::new()
    };

    // Interleave one virtual diagnostic line below each source row that
    // has any diagnostics. Stop accumulating once we've consumed
    // `height` visual rows.
    let mut visible: Vec<Line> = Vec::with_capacity(height);
    let mut visual_y: u16 = 0;
    let mut cursor_visual_y: u16 = 0;
    let inner_text_width =
        area.width
            .saturating_sub(GUTTER_SIGN_WIDTH + 5 + GUTTER_VCS_WIDTH) as usize;
    let col_scroll = compute_col_scroll(app, inner_text_width, tab_width);
    // One theme handle for the whole gutter pass (cheap Arc clone).
    let theme = crate::theme::active();
    for (i, line) in app.active_doc().lines.iter().enumerate().skip(scroll) {
        if visual_y as usize >= height {
            break;
        }
        // Rows inside a collapsed fold contribute no screen row. The
        // cursor is kept off hidden rows (see `snap_cursor_out_of_fold`),
        // so the cursor-row capture below is safe after this skip.
        if hidden.contains(&i) {
            continue;
        }
        if i == cursor_row {
            cursor_visual_y = visual_y;
        }
        // Sign cell: the diagnostic severity sign wins the cell (errors
        // matter more than a bookmark). On a row that's *both* diagnosed
        // and bookmarked, the bookmark is signalled by underlining that
        // sign instead of being hidden; a bookmark with no diagnostic
        // shows the dot; otherwise the cell is blank.
        let sev = row_severity.get(&i).copied();
        let bookmarked = bookmark_rows.contains(&i);
        let sign = match (sev, bookmarked) {
            (Some(_), true) => {
                let mut s = sign_span(sev);
                s.style = s.style.add_modifier(ratatui::style::Modifier::UNDERLINED);
                s
            }
            (Some(_), false) => sign_span(sev),
            (None, true) => bookmark_sign_span(),
            (None, false) => sign_span(None),
        };
        let mut spans = vec![sign];
        // Gutter layout: <sign><4-digit num><space><vcs-bar><buffer>.
        // The breathing-room space sits between the number and the
        // bar; cursor column math in `place_cursor` matches.
        let num = format!("{:>4} ", i + 1);
        // Cursor row vs. the rest, from the active theme
        // (`ui.linenr.selected` / `ui.linenr`). Defaults preserve the old
        // look: the cursor row tracks the terminal fg (`Reset`), others
        // dim gray.
        let num_style = if i == cursor_row {
            theme.ui_linenr_selected()
        } else {
            theme.ui_linenr()
        };
        spans.push(Span::styled(num, num_style));
        let vcs_status = vcs_statuses.get(i).copied().flatten();
        spans.push(vcs_bar_span(vcs_status));
        let extra_cols: Vec<usize> = extras
            .iter()
            .filter_map(|c| if c.row == i { Some(c.col) } else { None })
            .collect();
        let hits = find_matches_in_line(line, search_query);
        let row_jumps: Vec<(usize, char)> = jump_overlay
            .iter()
            .filter_map(|(pos, ch)| if pos.0 == i { Some((pos.1, *ch)) } else { None })
            .collect();
        let row_bracket_cols: Vec<usize> = bracket_pair
            .iter()
            .filter_map(|(r, c)| if *r == i { Some(*c) } else { None })
            .collect();
        let row_guides: &[IndentGuide] = indent_guides.get(&i).map(Vec::as_slice).unwrap_or(&[]);
        spans.extend(render_line(
            i,
            line,
            sel.as_ref(),
            &captures,
            &extra_cols,
            &hits,
            &row_jumps,
            &row_bracket_cols,
            row_guides,
            tab_width,
            col_scroll,
            inner_text_width,
            show_whitespace,
        ));
        // Collapsed-fold marker: appended at EOL of the (visible) header
        // row, summarizing how many rows the fold hides.
        if let Some(&end) = header_end.get(&i) {
            let n = end - i;
            let label = if n == 1 {
                " ⋯ 1 line".to_string()
            } else {
                format!(" ⋯ {n} lines")
            };
            spans.push(Span::styled(label, theme.fold_marker()));
        }
        // Inline suggestion (ghost text). Anchored at EOL of the
        // cursor row, so the first line is appended after the rendered
        // text, flush against the cursor cell. Multi-line suggestions
        // produce continuation rows pushed in below — same visual
        // shift mechanism the diagnostic interleave uses.
        let ghost_continuation: Vec<String> = if i == cursor_row
            && let Some(s) = app.inline_suggestion.showing()
            && s.is_anchored_at(app.editor.cursor)
        {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(ratatui::style::Modifier::ITALIC);
            let mut parts = s.text.split('\n');
            if let Some(first) = parts.next()
                && !first.is_empty()
            {
                spans.push(Span::styled(first.to_string(), style));
            }
            parts.map(str::to_string).collect()
        } else {
            Vec::new()
        };
        visible.push(Line::from(spans));
        visual_y += 1;
        if visual_y as usize >= height {
            break;
        }
        if !ghost_continuation.is_empty() {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(ratatui::style::Modifier::ITALIC);
            for cont in ghost_continuation {
                visible.push(Line::from(Span::styled(cont, style)));
                visual_y += 1;
                if visual_y as usize >= height {
                    break;
                }
            }
            if visual_y as usize >= height {
                break;
            }
        }
        if let Some(entries) = row_diag.get(&i) {
            for entry in entries {
                if visual_y as usize >= height {
                    break;
                }
                visible.push(diagnostic_line(entry, inner_text_width));
                visual_y += 1;
            }
        }
    }

    app.active_doc().cursor_visual_y.set(cursor_visual_y);
    f.render_widget(with_background(Paragraph::new(visible), &theme), area);
}

/// Apply the theme's `ui.background` as the paragraph's base bg, so the
/// whole viewport (gutter, text, and blank cells past the content) picks
/// up the editor background. Themes that don't set one (e.g. `ansi`)
/// leave the terminal's own background showing through.
fn with_background<'a>(para: Paragraph<'a>, theme: &crate::theme::Theme) -> Paragraph<'a> {
    match theme.ui_background() {
        Some(style) => para.style(style),
        None => para,
    }
}

pub(super) fn place_cursor(f: &mut Frame, app: &App, buf_area: Rect) {
    if app.prompt.is_open() {
        return;
    }
    let line_no_width: u16 = 5;
    let tab_width = app.effective_editor().tab_width.max(1);
    let line = &app.active_doc().lines[app.editor.cursor.row];
    let visual_col = visual_col_of(line, app.editor.cursor.col, tab_width);
    let col_scroll = app.active_doc().col_scroll.get();
    let on_screen_col = visual_col.saturating_sub(col_scroll);
    let x =
        buf_area.x + GUTTER_SIGN_WIDTH + line_no_width + GUTTER_VCS_WIDTH + on_screen_col as u16;
    // `draw_buffer` ran first this frame and published the cursor's
    // visual y, accounting for any virtual diagnostic lines pushing it
    // down. Use it directly so the terminal cursor stays glued to the
    // rendered cursor row.
    let y = buf_area.y + app.active_doc().cursor_visual_y.get();
    f.set_cursor_position((x, y));
}

/// Render an inactive pane's buffer. Deliberately a thin renderer:
/// gutter (line numbers + VCS bars) and lines with syntax highlighting,
/// but no diagnostics, no selection, no extra cursors, no jump-label
/// overlay, no search-hit painting — those overlays all belong to the
/// active pane. Scroll is anchored on the inactive pane's own session
/// cursor (`Editor.cursor.row`) over the shared document's `scroll`, so
/// each pane remembers where the user was last looking.
pub(super) fn draw_buffer_inactive(
    f: &mut Frame,
    ed: &Editor,
    buf: &crate::editor::Buffer,
    eff: &EditorConfig,
    area: Rect,
) {
    let height = area.height as usize;
    let cur = ed.cursor.row;
    let mut scroll = buf.scroll.get();
    let off = if height > 2 * SCROLL_OFF + 1 {
        SCROLL_OFF
    } else {
        0
    };
    if cur < scroll + off {
        scroll = cur.saturating_sub(off);
    } else if height > 0 && cur + off >= scroll + height {
        scroll = (cur + off + 1).saturating_sub(height);
    }
    let last_row = buf.lines.len().saturating_sub(1);
    scroll = scroll.min(last_row);
    buf.scroll.set(scroll);
    buf.viewport_height.set(height);
    let tab_width = eff.tab_width.max(1);
    let show_whitespace = eff.show_whitespace;
    // Inactive panes fold by their own session state. Use the shared
    // region resolver so the document folds identically whether or not
    // this pane has focus (same syntax/indent + import merge as the
    // active path).
    let (hidden, header_end) = if ed.folds().is_empty() {
        (HashSet::new(), HashMap::new())
    } else {
        let regions = crate::editor::fold::buffer_fold_regions(buf, tab_width);
        build_fold_view(&regions, ed.folds())
    };
    // Fold-aware visible window (see `draw_buffer`): collapsed folds let
    // the viewport span more document rows than `height`, so the syntax
    // window must reach the last actually-rendered row.
    let last_visible = if hidden.is_empty() {
        scroll + height
    } else {
        let total = buf.lines.len();
        let mut shown = 0usize;
        let mut row = scroll;
        while row < total && shown < height {
            if !hidden.contains(&row) {
                shown += 1;
            }
            row += 1;
        }
        row.max(scroll + 1)
    };
    let mut captures = buf
        .highlighter
        .as_ref()
        .map(|h| h.captures_in_rows(scroll, last_visible))
        .unwrap_or_default();
    let conflict_hunks = buf.conflict_hunks();
    captures.extend(conflict_captures(
        &buf.lines,
        &conflict_hunks,
        scroll,
        last_visible,
    ));
    let vcs_statuses = buf.vcs_statuses();
    let theme = crate::theme::active();
    let inner_text_width =
        area.width
            .saturating_sub(GUTTER_SIGN_WIDTH + 5 + GUTTER_VCS_WIDTH) as usize;
    // Track col_scroll on the inactive pane's own cell so horizontal
    // jumps still work when the user re-focuses it.
    let line = buf.lines.get(cur).map(String::as_str).unwrap_or("");
    let visual_col = visual_col_of(line, ed.cursor.col, tab_width);
    let mut col_scroll = buf.col_scroll.get();
    if inner_text_width > 0 {
        if visual_col < col_scroll {
            col_scroll = visual_col;
        } else if visual_col >= col_scroll + inner_text_width {
            col_scroll = visual_col + 1 - inner_text_width;
        }
    } else {
        col_scroll = 0;
    }
    buf.col_scroll.set(col_scroll);

    let mut visible: Vec<Line> = Vec::with_capacity(height);
    let mut visual_y = 0usize;
    for (i, line) in buf.lines.iter().enumerate().skip(scroll) {
        if visual_y >= height {
            break;
        }
        if hidden.contains(&i) {
            continue;
        }
        let mut spans = vec![sign_span(None)];
        let num = format!("{:>4} ", i + 1);
        spans.push(Span::styled(num, Style::default().fg(Color::DarkGray)));
        let vcs_status = vcs_statuses.get(i).copied().flatten();
        spans.push(vcs_bar_span(vcs_status));
        spans.extend(render_line(
            i,
            line,
            None,
            &captures,
            &[],
            &[],
            &[],
            &[],
            &[],
            tab_width,
            col_scroll,
            inner_text_width,
            show_whitespace,
        ));
        if let Some(&end) = header_end.get(&i) {
            let n = end - i;
            let label = if n == 1 {
                " ⋯ 1 line".to_string()
            } else {
                format!(" ⋯ {n} lines")
            };
            spans.push(Span::styled(label, theme.fold_marker()));
        }
        visible.push(Line::from(spans));
        visual_y += 1;
    }
    f.render_widget(with_background(Paragraph::new(visible), &theme), area);
}
