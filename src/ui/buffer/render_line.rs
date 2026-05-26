//! Per-line rendering: layering syntax-highlight captures, selection
//! backgrounds, search hits, extra cursors, indent guides, jump labels,
//! and whitespace markers into a minimal run of styled spans.

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use std::collections::HashMap;

use crate::app::{JumpState, Selection};
use crate::lsp::Severity;
use crate::syntax::{self, Capture};
use crate::text_width::char_cell_width;
use crate::vcs::LineStatus;

use super::indent_guides::IndentGuide;
use super::{
    EXTRA_CURSOR_BG, EXTRA_CURSOR_FG, INDENT_GUIDE_FG, JUMP_LABEL_BG, JUMP_LABEL_FG,
    MATCH_BRACKET_FG, WHITESPACE_FG, search_style, sel_style,
};

/// Gutter cell rendered between the line number and the buffer text.
/// A thin vertical bar colored per VCS status, or a plain space when
/// the row has no status (and the trailing-space slot is preserved).
pub(super) fn vcs_bar_span(status: Option<LineStatus>) -> Span<'static> {
    match status {
        Some(LineStatus::Added) => Span::styled("▎", Style::default().fg(Color::Green)),
        Some(LineStatus::Modified) => Span::styled("▎", Style::default().fg(Color::Yellow)),
        Some(LineStatus::DeletedAbove) => Span::styled("▁", Style::default().fg(Color::Red)),
        None => Span::raw(" "),
    }
}

/// Gutter sign for a bookmarked row — a filled dot in the sign column,
/// shown in place of the diagnostic severity sign on rows carrying a
/// harpoon bookmark (`<space>m`). Single-width so the gutter layout is
/// unchanged.
pub(super) fn bookmark_sign_span() -> Span<'static> {
    Span::styled("●", Style::default().fg(Color::LightMagenta))
}

pub(super) fn sign_span(sev: Option<Severity>) -> Span<'static> {
    match sev {
        Some(Severity::Error) => Span::styled("E", Style::default().fg(Color::Red)),
        Some(Severity::Warning) => Span::styled("W", Style::default().fg(Color::Yellow)),
        Some(Severity::Info) => Span::styled("I", Style::default().fg(Color::LightBlue)),
        Some(Severity::Hint) => Span::styled("H", Style::default().fg(Color::DarkGray)),
        None => Span::raw(" "),
    }
}

/// Render one buffer line, layering syntax-highlight captures
/// (foreground) underneath the visual-selection background. Spans
/// group consecutive characters that share the same resolved style so
/// the terminal sees as few escape changes as possible.
///
/// `captures` is the row-range slice produced by the highlighter for
/// the visible window; we filter per row internally rather than
/// re-extracting per call.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_line(
    row: usize,
    line: &str,
    sel: Option<&Selection>,
    captures: &[Capture],
    extra_cols: &[usize],
    search_hits: &[(usize, usize)],
    jump_labels: &[(usize, char)],
    bracket_cols: &[usize],
    indent_guides: &[IndentGuide],
    tab_width: usize,
    col_scroll: usize,
    viewport_width: usize,
    show_whitespace: bool,
) -> Vec<Span<'static>> {
    // Look up a guide at visual column `vc`; cached by closure so the
    // tight per-cell loop stays branch-light.
    let guide_at =
        |vc: usize| -> Option<IndentGuide> { indent_guides.iter().find(|g| g.col == vc).copied() };
    let guide_style = |g: IndentGuide| -> Style {
        if g.active {
            // Active uses the terminal's default foreground + bold so
            // the bar is the same hue as code (no extra palette
            // assumption) but visibly stands out from inactive guides.
            // `Color::Reset` is explicit so `style.patch(guide_style)`
            // overrides any underlying cell fg (e.g. `WHITESPACE_FG`
            // when `show_whitespace` is on) instead of inheriting it.
            Style::default()
                .fg(Color::Reset)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default().fg(INDENT_GUIDE_FG)
        }
    };
    let is_extra_cursor = |col: usize| -> bool { extra_cols.contains(&col) };
    let is_search_hit =
        |col: usize| -> bool { search_hits.iter().any(|(lo, hi)| col >= *lo && col < *hi) };
    let is_match_bracket = |col: usize| -> bool { bracket_cols.contains(&col) };
    let jump_label_at = |col: usize| -> Option<char> {
        jump_labels
            .iter()
            .find_map(|(c, ch)| if *c == col { Some(*ch) } else { None })
    };
    let is_selected = |col: usize| -> bool {
        let Some(sel) = sel else { return false };
        match *sel {
            Selection::Char { from, to } => {
                if row < from.row || row > to.row {
                    return false;
                }
                let lo = if row == from.row { from.col } else { 0 };
                if row < to.row {
                    col >= lo
                } else {
                    col >= lo && col <= to.col
                }
            }
            Selection::Line { from_row, to_row } => row >= from_row && row <= to_row,
            Selection::Block { r0, c0, r1, c1 } => row >= r0 && row <= r1 && col >= c0 && col <= c1,
        }
    };

    let chars: Vec<char> = line.chars().collect();
    let viewport_right = col_scroll.saturating_add(viewport_width);
    // Max guide visual column — used to pad past EOL when guides need
    // to extend beyond the line content (blank lines inside a scope,
    // or lines shorter than the deepest scope's column).
    let max_guide_col = indent_guides.iter().map(|g| g.col).max();
    if chars.is_empty() {
        let cursor_cell_style = {
            let mut style = Style::default();
            if is_selected(0) {
                style = style.patch(sel_style());
            }
            if is_extra_cursor(0) {
                style = extra_cursor_style(style);
            }
            style
        };
        let emit_until = max_guide_col.map(|m| m + 1).unwrap_or(0).max(
            if cursor_cell_style != Style::default() {
                1
            } else {
                0
            },
        );
        if emit_until == 0 || col_scroll >= emit_until {
            return Vec::new();
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut buf_style = Style::default();
        let mut started = false;
        for vc in col_scroll..emit_until {
            if viewport_width > 0 && vc >= viewport_right {
                break;
            }
            let base_style = if vc == 0 {
                cursor_cell_style
            } else {
                Style::default()
            };
            let (ch, style) = if let Some(g) = guide_at(vc) {
                (g.glyph, base_style.patch(guide_style(g)))
            } else {
                (' ', base_style)
            };
            if !started {
                buf_style = style;
                started = true;
            } else if style != buf_style {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
                }
                buf_style = style;
            }
            buf.push(ch);
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, buf_style));
        }
        return spans;
    }

    // Build the per-character base (highlight) style. Captures are
    // sorted in document order; later-arriving captures overwrite
    // earlier ones for the same character, matching the convention
    // that more-specific rules appear later in `highlights.scm`.
    let mut base: Vec<Style> = vec![Style::default(); chars.len()];
    for cap in captures {
        if cap.end_row < row || cap.start_row > row {
            continue;
        }
        let lo = if cap.start_row == row {
            cap.start_col
        } else {
            0
        };
        let hi = if cap.end_row == row {
            cap.end_col.min(chars.len())
        } else {
            chars.len()
        };
        if lo >= hi {
            continue;
        }
        let style = syntax::style_for(&cap.name);
        // Patch (not overwrite) so later, more-specific captures layer
        // on top of earlier ones — e.g. a heading's per-token fg sits on
        // top of the parent `(atx_heading)` bg instead of erasing it.
        for slot in base.iter_mut().take(hi).skip(lo) {
            *slot = slot.patch(style);
        }
    }

    // Backgrounds layered from least to most specific: search hit →
    // visual selection → extra cursor (which uses an outline modifier
    // rather than a fill, so it sits on top of any underlying bg).
    // Matching-bracket is a fg/bold overlay applied last so the pair
    // remains identifiable even when sitting inside a selection or
    // search match.
    // Resolve the theme-driven selection/search styles once for the whole
    // row instead of per cell (this closure runs for every column).
    let sel = sel_style();
    let search = search_style();
    let style_at = |col: usize| -> Style {
        let mut s = base[col];
        if is_search_hit(col) {
            s = s.patch(search);
        }
        if is_selected(col) {
            s = s.patch(sel);
        }
        if is_extra_cursor(col) {
            s = extra_cursor_style(s);
        }
        if is_match_bracket(col) {
            s = s
                .fg(MATCH_BRACKET_FG)
                .add_modifier(ratatui::style::Modifier::BOLD);
        }
        s
    };

    // Per-col character + style. A `gw` jump label overlays its char on
    // top of the underlying buffer char with `JUMP_LABEL_*` styling.
    // When `show_whitespace` is on, plain spaces become `·` and the
    // leading cell of a tab becomes `→`, both painted in `WHITESPACE_FG`
    // so they sit visibly above (but quietly with) the surrounding text.
    let cell_at = |col: usize| -> (char, Style) {
        if let Some(label) = jump_label_at(col) {
            return (
                label,
                Style::default()
                    .fg(JUMP_LABEL_FG)
                    .bg(JUMP_LABEL_BG)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            );
        }
        let original = chars[col];
        let style = style_at(col);
        if show_whitespace {
            match original {
                ' ' => return ('·', style.fg(WHITESPACE_FG)),
                '\t' => return ('→', style.fg(WHITESPACE_FG)),
                _ => {}
            }
        }
        (original, style)
    };

    // Each char takes one visible cell except `\t`, which jumps to the
    // next `tab_width`-aligned stop. The expanded tab is filled with
    // spaces so its background style (selection / search hit / extra
    // cursor) covers the entire run, and `visual_col` tracks the running
    // cell position so each tab measures from where it actually sits.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut buf_style = Style::default();
    let mut visual_col = 0usize;
    let mut started = false;
    let push_cell = |spans: &mut Vec<Span<'static>>,
                     buf: &mut String,
                     buf_style: &mut Style,
                     started: &mut bool,
                     ch: char,
                     style: Style| {
        if !*started {
            *buf_style = style;
            *started = true;
        } else if style != *buf_style {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(buf), *buf_style));
            }
            *buf_style = style;
        }
        buf.push(ch);
    };
    for (col, &original) in chars.iter().enumerate() {
        let (ch, style) = cell_at(col);
        let width = if original == '\t' {
            tab_width - (visual_col % tab_width)
        } else {
            char_cell_width(original)
        };
        let cell_start = visual_col;
        let cell_end = visual_col + width;
        visual_col = cell_end;

        // Stop once we've passed the right edge: ratatui's Paragraph
        // would truncate anyway, but bailing early keeps very long
        // lines from materializing megabytes of spans per draw.
        if viewport_width > 0 && cell_start >= viewport_right {
            break;
        }
        // Skip cells entirely to the left of the horizontal scroll.
        if cell_end <= col_scroll {
            continue;
        }
        let is_ws = original == ' ' || original == '\t';
        // When `show_whitespace=true` puts a `→` on the tab's leading
        // cell and an indent guide overrides that same cell, remember
        // the arrow so the next non-guide cell within this tab can
        // emit it — otherwise the marker would silently vanish on
        // every tab-indented line.
        let mut displaced_arrow: Option<Style> = None;
        // Per-cell emission so a tab spanning multiple cells can have
        // any subset overridden by an indent-guide glyph at that
        // exact visual column.
        for k in 0..width {
            let vc = cell_start + k;
            if vc < col_scroll {
                continue;
            }
            if viewport_width > 0 && vc >= viewport_right {
                break;
            }
            // Guide override: any whitespace cell may be replaced
            // by an indent-guide glyph. Jump labels still take
            // precedence over guides (they're emitted via `ch`
            // below when no guide is present). The tab
            // whitespace marker (`→`) yields to a guide on the
            // leading cell but reappears in the next free cell
            // via `displaced_arrow`.
            let jump_lead =
                original == '\t' && k == 0 && ch != '\t' && jump_label_at(col).is_some();
            let guide = if is_ws && !jump_lead {
                guide_at(vc)
            } else {
                None
            };
            let (out_ch, out_style) = if let Some(g) = guide {
                if original == '\t' && k == 0 && ch != '\t' {
                    displaced_arrow = Some(style);
                }
                // Patch (don't replace) so the cell's selection /
                // extra-cursor / search-hit background carries through
                // under the guide's fg/modifier — otherwise every
                // tab-aligned guide column erases the highlight on
                // tab-indented lines.
                (g.glyph, style.patch(guide_style(g)))
            } else if original == '\t' {
                if k == 0 && ch != '\t' {
                    (ch, style)
                } else if let Some(arrow_style) = displaced_arrow.take() {
                    ('→', arrow_style)
                } else {
                    (' ', style)
                }
            } else if k == 0 {
                (ch, style)
            } else {
                // Trailing cell of a wide glyph: ratatui renders the
                // wide char across both cells from a single span entry,
                // so the leading-cell push above already covers this
                // column. When the leading cell was clipped on the
                // left by `col_scroll`, emit a filler space instead so
                // downstream columns don't shift.
                if cell_start >= col_scroll {
                    continue;
                }
                (' ', style)
            };
            push_cell(
                &mut spans,
                &mut buf,
                &mut buf_style,
                &mut started,
                out_ch,
                out_style,
            );
        }
    }
    // Pad past EOL with guide cells when a scope's guide column
    // lives to the right of the last char on this line (typical for
    // a body row that's shorter than its enclosing scope's column,
    // e.g. a comment with less indent inside a deeper block).
    if let Some(m) = max_guide_col
        && visual_col <= m
    {
        for vc in visual_col.max(col_scroll)..=m {
            if viewport_width > 0 && vc >= viewport_right {
                break;
            }
            let (ch, style) = match guide_at(vc) {
                Some(g) => (g.glyph, guide_style(g)),
                None => (' ', Style::default()),
            };
            push_cell(
                &mut spans,
                &mut buf,
                &mut buf_style,
                &mut started,
                ch,
                style,
            );
        }
        visual_col = visual_col.max(m + 1);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, buf_style));
    }
    // Past-end extra cursor — paint one extra cell so a cursor sitting
    // one column past the last char (the natural Insert-mode position
    // after typing) stays visible. Only when it falls inside the
    // horizontal viewport.
    if is_extra_cursor(chars.len())
        && visual_col >= col_scroll
        && (viewport_width == 0 || visual_col < viewport_right)
    {
        spans.push(Span::styled(
            " ".to_string(),
            extra_cursor_style(Style::default()),
        ));
    }
    spans
}

/// Style overlay applied to every extra-cursor cell. Solid background
/// so the cell stays visible against any underlying syntax / search /
/// selection layer.
pub(super) fn extra_cursor_style(base: Style) -> Style {
    base.bg(EXTRA_CURSOR_BG).fg(EXTRA_CURSOR_FG)
}

/// Lower the active `gw` jump state into a `(row, col) → char` overlay
/// map suitable for the per-line renderer.
///
/// - Before any keystroke: each label contributes its first char at
///   the target col, and (when present) its second char at col+1.
/// - After the first keystroke: only labels whose `first` matches the
///   typed char survive; they show as just their second char at the
///   target col. Single-char labels never reach this state because
///   `handle_jump_key` short-circuits to the jump.
pub(super) fn build_jump_overlay(state: Option<&JumpState>) -> HashMap<(usize, usize), char> {
    let mut out = HashMap::new();
    let Some(s) = state else { return out };
    match s.typed_first {
        None => {
            for label in &s.labels {
                out.insert((label.pos.row, label.pos.col), label.first);
                if let Some(c2) = label.second {
                    out.insert((label.pos.row, label.pos.col + 1), c2);
                }
            }
        }
        Some(first) => {
            for label in &s.labels {
                if label.first != first {
                    continue;
                }
                if let Some(c2) = label.second {
                    out.insert((label.pos.row, label.pos.col), c2);
                }
            }
        }
    }
    out
}

/// All matches of `query` in `line`, returned as half-open char
/// ranges. Empty `query` returns no hits, so callers don't accidentally
/// paint the entire buffer when no search is active.
pub(super) fn find_matches_in_line(line: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let q_chars = query.chars().count();
    let mut hits = Vec::new();
    let mut search_from = 0;
    while let Some(byte_idx) = line[search_from..].find(query) {
        let abs_byte = search_from + byte_idx;
        let start_col = line[..abs_byte].chars().count();
        hits.push((start_col, start_col + q_chars));
        // Advance past this match so we don't re-find overlapping
        // occurrences. `query.len()` is byte length, which is safe to
        // add at a UTF-8 boundary.
        search_from = abs_byte + query.len();
        if search_from >= line.len() {
            break;
        }
    }
    hits
}
