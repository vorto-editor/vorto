//! Sticky vertical and horizontal scroll computation for the active
//! buffer viewport.

use std::collections::HashMap;

use crate::app::App;
use crate::text_width::visual_col_of;

use super::SCROLL_OFF;
use super::diagnostics::RowDiag;

/// Update and return the viewport scroll position. Sticky: the scroll
/// only moves when the cursor would otherwise fall outside the
/// visible `height`-row window. Cursor-above-viewport scrolls up so
/// the cursor sits on the top line; cursor-below-viewport scrolls
/// down so the cursor sits on the bottom line. Otherwise the existing
/// scroll is preserved — which is what fixes "cursor stuck at the
/// bottom" on upward movement.
///
/// `row_diag` is the per-row diagnostic summary; rows with diagnostics
/// each consume one extra visual row, so the "does the cursor fit"
/// check uses visual heights rather than raw source-row counts.
pub(super) fn compute_scroll(
    app: &App,
    height: usize,
    row_diag: &HashMap<usize, RowDiag>,
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
                consumed += 1 + row_diag.get(&row).map_or(0, |_| 1);
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
    let mut col_scroll = app.active_doc().col_scroll.get();
    if visual_col < col_scroll {
        col_scroll = visual_col;
    } else if visual_col >= col_scroll + width {
        col_scroll = visual_col + 1 - width;
    }
    app.active_doc().col_scroll.set(col_scroll);
    col_scroll
}
