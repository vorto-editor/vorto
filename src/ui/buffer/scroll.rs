//! Sticky vertical and horizontal scroll computation for the active
//! buffer viewport.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::text_width::{char_cell_width, visual_col_of};

use super::SCROLL_OFF;
use super::diagnostics::DiagLine;

/// Update and return the viewport scroll position. Sticky: the scroll
/// only moves when the cursor would otherwise fall outside the
/// visible `height`-row window. Cursor-above-viewport scrolls up so
/// the cursor sits on the top line; cursor-below-viewport scrolls
/// down so the cursor sits on the bottom line. Otherwise the existing
/// scroll is preserved — which is what fixes "cursor stuck at the
/// bottom" on upward movement.
///
/// `row_diag` is the per-row diagnostic summary; each surfaced
/// diagnostic line consumes one extra visual row (the cursor's row can
/// surface several), so the "does the cursor fit" check uses visual
/// heights rather than raw source-row counts.
///
/// `hidden` are rows inside collapsed folds; they consume zero visual
/// rows so the scroll-off math counts only what's actually drawn.
pub(super) fn compute_scroll(
    app: &App,
    height: usize,
    row_diag: &HashMap<usize, Vec<DiagLine>>,
    hidden: &HashSet<usize>,
) -> usize {
    let cur = app.editor.cursor.row;
    let mut scroll = app.active_doc().scroll.get();
    // Deferred centering from a picker-driven jump that fired before
    // the viewport size was known. Take-and-clear so it's a one-shot
    // override, then fall through to publishing the new scroll/height.
    if app.active_doc().pending_center.replace(false) && height > 0 {
        let last = app.active_doc().lines.len().saturating_sub(1);
        let max_scroll = last.saturating_sub(height.saturating_sub(1));
        scroll = cur.saturating_sub(height / 2).min(max_scroll);
        app.active_doc().scroll.set(scroll);
        app.active_doc().viewport_height.set(height);
        return scroll;
    }
    // Shrink the scroll-off to 0 on viewports too small to give the
    // cursor room on both sides; otherwise the padding would fight
    // itself and lock the cursor in place.
    let off = if height > 2 * SCROLL_OFF + 1 {
        SCROLL_OFF
    } else {
        0
    };

    if cur < scroll + off {
        scroll = cur.saturating_sub(off);
    } else if height > 0 {
        // Walk rows [scroll..cur], accumulating each row's visual
        // height (1 + 1 if it has diagnostics). Advance scroll forward
        // until the cursor's source row fits with `off` rows of room
        // below it — i.e. `consumed_above_cursor < height - off`. Past
        // EOF this lets scroll exceed `last - height + 1`; the render
        // loop just stops emitting rows when source lines run out.
        let effective_height = height.saturating_sub(off);
        loop {
            if scroll >= cur {
                break;
            }
            let mut consumed: usize = 0;
            for row in scroll..cur {
                if hidden.contains(&row) {
                    continue;
                }
                consumed += 1 + row_diag.get(&row).map_or(0, Vec::len);
                if consumed >= effective_height {
                    break;
                }
            }
            if consumed < effective_height {
                break;
            }
            scroll += 1;
        }
    }
    // Keep at least the last source line visible — don't let past-EOF
    // padding push every real row off the top.
    let last_row = app.active_doc().lines.len().saturating_sub(1);
    scroll = scroll.min(last_row);
    app.active_doc().scroll.set(scroll);
    // Publish the height so `H`/`M`/`L` and the `<C-d>`/`<C-u>` family
    // (handled in the input thread) can read what's currently visible.
    app.active_doc().viewport_height.set(height);
    scroll
}

/// Update and return the horizontal scroll offset. Sticky like
/// [`compute_scroll`]: shifts the visible window only when the cursor's
/// visual column would otherwise fall outside `[col_scroll, col_scroll
/// + width)`. `width == 0` collapses to no scroll (degenerate frame).
pub(super) fn compute_col_scroll(app: &App, width: usize, tab_width: usize) -> usize {
    if width == 0 {
        app.active_doc().col_scroll.set(0);
        return 0;
    }
    let line = &app.active_doc().lines[app.editor.cursor.row];
    let visual_col = visual_col_of(line, app.editor.cursor.col, tab_width);
    // Cell width of the char the cursor sits on. A wide CJK glyph or
    // emoji occupies two cells; the scroll math must keep *both* visible
    // or the terminal can't draw the glyph in the single remaining cell
    // at the right edge and the character vanishes. Past EOL the cursor
    // sits on a one-cell blank.
    let cursor_width = cursor_cell_width(line, app.editor.cursor.col, visual_col, tab_width);
    let prev = app.active_doc().col_scroll.get();
    let col_scroll = horizontal_scroll(visual_col, cursor_width, width, prev);
    app.active_doc().col_scroll.set(col_scroll);
    col_scroll
}

/// Sticky horizontal scroll: shift the visible window only when the
/// cursor glyph (`cursor_width` cells starting at `visual_col`) would
/// fall outside `[prev, prev + width)`. A wide CJK char or emoji
/// (`cursor_width == 2`) at the right edge is scrolled so *both* its
/// cells stay visible — otherwise the terminal can't draw the glyph in
/// the single remaining cell and the character disappears.
pub(super) fn horizontal_scroll(
    visual_col: usize,
    cursor_width: usize,
    width: usize,
    prev: usize,
) -> usize {
    if width == 0 {
        return 0;
    }
    // Clamp the glyph width to the viewport: a 2-cell glyph in a 1-cell
    // window can't fit either way, but without the clamp the scroll start
    // would advance *past* `visual_col`, pushing even the cursor's left
    // cell off-screen and breaking the `col_scroll <= visual_col`
    // invariant that `place_cursor` relies on. Clamping keeps at least the
    // left edge anchored.
    let cursor_width = cursor_width.min(width);
    if visual_col < prev {
        visual_col
    } else if visual_col + cursor_width > prev + width {
        visual_col + cursor_width - width
    } else {
        prev
    }
}

/// Cell width of the character at `char_col` in `line`, where
/// `visual_col` is that character's starting visual column (tabs already
/// accounted for). Tabs expand to the next `tab_width`-aligned stop;
/// a cursor past the last char counts as one blank cell.
pub(super) fn cursor_cell_width(
    line: &str,
    char_col: usize,
    visual_col: usize,
    tab_width: usize,
) -> usize {
    match line.chars().nth(char_col) {
        Some('\t') => tab_width - (visual_col % tab_width),
        Some(ch) => char_cell_width(ch),
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_cursor_at_right_edge_scrolls_one_column() {
        // Column 80 just past an 80-wide window → shift by one so the
        // cursor sits on the last visible column.
        assert_eq!(horizontal_scroll(80, 1, 80, 0), 1);
    }

    #[test]
    fn wide_cursor_at_right_edge_keeps_both_cells_visible() {
        // A 2-cell glyph starting on the last column (79) of an 80-wide
        // window: without width-awareness this stayed at scroll 0 and the
        // glyph's second cell (80) fell off-screen, so the terminal drew
        // nothing. Now we scroll by one so the glyph occupies cols 78–79.
        assert_eq!(horizontal_scroll(79, 2, 80, 0), 1);
        // The fix is a no-op for a glyph that already fits at the edge.
        assert_eq!(horizontal_scroll(78, 2, 80, 0), 0);
    }

    #[test]
    fn wide_glyph_in_one_cell_viewport_keeps_left_edge() {
        // Degenerate: a 2-cell glyph can't fit a 1-cell window. The scroll
        // start must not advance past `visual_col` (which would hide even
        // the left cell and misplace the cursor) — it clamps to the glyph
        // start so `col_scroll <= visual_col` holds.
        assert_eq!(horizontal_scroll(40, 2, 1, 0), 40);
        assert_eq!(horizontal_scroll(40, 2, 1, 50), 40);
    }

    #[test]
    fn scroll_left_when_cursor_precedes_window() {
        assert_eq!(horizontal_scroll(3, 1, 80, 10), 3);
    }

    #[test]
    fn sticky_when_cursor_already_visible() {
        assert_eq!(horizontal_scroll(40, 1, 80, 10), 10);
        assert_eq!(horizontal_scroll(40, 2, 80, 10), 10);
    }

    #[test]
    fn cursor_cell_width_handles_tabs_and_wide_chars() {
        assert_eq!(cursor_cell_width("\t", 0, 0, 4), 4);
        assert_eq!(cursor_cell_width("ab\t", 2, 2, 4), 2);
        assert_eq!(cursor_cell_width("あ", 0, 0, 4), 2);
        assert_eq!(cursor_cell_width("a", 0, 0, 4), 1);
        // Past EOL: one blank cell.
        assert_eq!(cursor_cell_width("a", 1, 1, 4), 1);
    }
}
