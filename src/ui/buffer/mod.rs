//! Buffer viewport: gutter (diagnostic signs + line numbers),
//! per-character syntax highlighting layered with the visual selection,
//! and the terminal cursor placement that goes with it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::config::EditorConfig;
use crate::editor::Editor;
use crate::text_width::visual_col_of;

mod diagnostics;
mod indent_guides;
mod render_line;
mod scroll;

use diagnostics::{build_row_diag_summary, build_row_severity, diagnostic_line};
use indent_guides::{GuideMap, IndentGuide, compute_indent_guides};
use render_line::{build_jump_overlay, find_matches_in_line, render_line, sign_span, vcs_bar_span};
use scroll::{compute_col_scroll, compute_scroll};

/// Color used to paint visually-selected text. ANSI bright-black so the
/// shade follows the user's terminal theme (color 8 in the palette).
pub(super) const SEL_BG: Color = Color::DarkGray;

/// Background used to highlight every visible match of the active
/// search pattern (vim's `hlsearch`). ANSI bright-black (the terminal's
/// dim gray) so it sits underneath text without competing with a
/// visual selection.
pub(super) const SEARCH_HIT_BG: Color = Color::DarkGray;

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

pub(super) fn draw_buffer(f: &mut Frame, app: &App, area: Rect) {
    let height = area.height as usize;
    let row_diag = build_row_diag_summary(app, app.editor.cursor.row);
    let scroll = compute_scroll(app, height, &row_diag);

    let sel = app.selection();
    let last_visible = scroll + height;
    let captures = app
        .active_doc()
        .highlighter
        .as_ref()
        .map(|h| h.captures_in_rows(scroll, last_visible))
        .unwrap_or_default();
    let row_severity = build_row_severity(app, scroll, last_visible);
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
    for (i, line) in app.active_doc().lines.iter().enumerate().skip(scroll) {
        if visual_y as usize >= height {
            break;
        }
        if i == cursor_row {
            cursor_visual_y = visual_y;
        }
        let mut spans = vec![sign_span(row_severity.get(&i).copied())];
        // Gutter layout: <sign><4-digit num><space><vcs-bar><buffer>.
        // The breathing-room space sits between the number and the
        // bar; cursor column math in `place_cursor` matches.
        let num = format!("{:>4} ", i + 1);
        // The cursor's row gets the terminal's default foreground
        // (`Color::Reset`) so the number stays in sync with whatever
        // color the terminal paints the cursor itself.
        let num_style = if i == cursor_row {
            Style::default().fg(Color::Reset)
        } else {
            Style::default().fg(Color::DarkGray)
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
        if let Some(summary) = row_diag.get(&i) {
            visible.push(diagnostic_line(summary, inner_text_width));
            visual_y += 1;
        }
    }

    app.active_doc().cursor_visual_y.set(cursor_visual_y);
    f.render_widget(Paragraph::new(visible), area);
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
/// active pane. Scroll is anchored on the inactive pane's own
/// `Buffer.cursor.row` / `Buffer.scroll`, so each pane remembers where
/// the user was last looking.
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
    let last_visible = scroll + height;
    let captures = buf
        .highlighter
        .as_ref()
        .map(|h| h.captures_in_rows(scroll, last_visible))
        .unwrap_or_default();
    let vcs_statuses = buf.vcs_statuses();
    let tab_width = eff.tab_width.max(1);
    let show_whitespace = eff.show_whitespace;
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
    for (visual_y, (i, line)) in (0_usize..).zip(buf.lines.iter().enumerate().skip(scroll)) {
        if visual_y >= height {
            break;
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
        visible.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(visible), area);
}
