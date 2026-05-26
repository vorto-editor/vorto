//! Indent-guide subsystem: tree-sitter–driven vertical guide bars,
//! active-scope detection, and the top-to-bottom animation envelope.

use std::collections::HashMap;

use crate::app::App;
use crate::config::IndentGuideStyle;
use crate::editor::IndentAnimState;

use super::INDENT_GUIDE_CHAR;

/// Visual column occupied by the first non-whitespace character of
/// `line`, with tabs expanded to `tab_width`-aligned stops. For a line
/// that is entirely whitespace (or empty), returns the visual width of
/// the whole line — callers treat that as "no content, defer to
/// neighbouring rows for guide layout".
fn leading_indent_visual(line: &str, tab_width: usize) -> Option<usize> {
    let mut v = 0usize;
    for ch in line.chars() {
        if ch == ' ' {
            v += 1;
        } else if ch == '\t' {
            v += tab_width - (v % tab_width);
        } else {
            return Some(v);
        }
    }
    None
}

/// One indent-guide cell to paint on a row: the visual column it
/// lives at, the glyph drawn there, and whether it belongs to the
/// active scope (→ distinct color/bold).
#[derive(Debug, Clone, Copy)]
pub(super) struct IndentGuide {
    pub(super) col: usize,
    pub(super) glyph: char,
    pub(super) active: bool,
}

/// Per-source-row guide list for the visible window. Keyed by row
/// index; rows with no guides are absent from the map.
pub(super) type GuideMap = HashMap<usize, Vec<IndentGuide>>;

/// Compute indent guides for `[scroll, last_visible)`.
///
/// Drawing is tree-sitter–driven: every `@indent.begin` scope from
/// the language's `indents.scm` becomes one vertical bar, positioned
/// at the **header row's own indent column** (not the body indent).
/// That choice keeps the bar in the leading-whitespace area of every
/// body row instead of colliding with content at the body's indent
/// — which would otherwise force the guide to be skipped on the
/// rows that need it most.
///
/// Active marking is scoped to the innermost `@indent.begin` node
/// containing the cursor — its column lights up only on rows that
/// belong to that scope's body, so sibling scopes at the same column
/// stay quiet.
///
/// When no tree-sitter highlighter is loaded (plain-text buffer or a
/// language without `indents.scm`), drawing falls back to a uniform
/// leading-whitespace stair-step at multiples of `indent_width`.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_indent_guides(
    app: &App,
    scroll: usize,
    last_visible: usize,
    tab_width: usize,
    indent_width: usize,
    skip_levels: usize,
    style: IndentGuideStyle,
    animation: bool,
    animation_ms: u64,
) -> GuideMap {
    let mut map: GuideMap = HashMap::new();
    if last_visible <= scroll || indent_width == 0 {
        return map;
    }
    let cursor_row = app.editor.cursor.row;
    let lines = &app.active_doc().lines;
    let line_count = lines.len();
    if line_count == 0 {
        return map;
    }

    // Stair-step at every multiple of `indent_width`. The step
    // comes from the effective per-language config so a 4-space
    // file doesn't get a guide every 2 cols just because the
    // global default is 2.
    let resolve_indent = |row: usize| -> usize {
        // Walk the leading whitespace ourselves so we can
        // distinguish "blank line, inherit from neighbours" from
        // "user typed indent but no content yet" — the latter
        // should use the typed-in width directly so guides show
        // up immediately while typing.
        let line = &lines[row];
        let mut ws = 0usize;
        let mut had_chars = false;
        for ch in line.chars() {
            had_chars = true;
            if ch == ' ' {
                ws += 1;
            } else if ch == '\t' {
                ws += tab_width - (ws % tab_width);
            } else {
                return ws;
            }
        }
        if had_chars {
            return ws;
        }
        // Truly empty: inherit from the shallower of the nearest
        // non-blank neighbours so bars stay continuous across
        // whitespace gaps.
        let above = (0..row)
            .rev()
            .find_map(|r| leading_indent_visual(&lines[r], tab_width))
            .unwrap_or(0);
        let below = (row + 1..line_count)
            .find_map(|r| leading_indent_visual(&lines[r], tab_width))
            .unwrap_or(0);
        above.min(below)
    };
    // With `skip_levels = 0` the stair-step also draws at col 0
    // (the buffer's left edge) so deeply indented rows show a `│`
    // running all the way to the left margin. The skip-levels
    // filter below handles other suppression — for `skip = 0`
    // nothing is dropped, so col 0 survives.
    let start_col = if skip_levels == 0 { 0 } else { indent_width };
    for row in scroll..last_visible.min(line_count) {
        let indent = resolve_indent(row);
        let mut col = start_col;
        while col < indent {
            push_unique_guide(
                &mut map,
                row,
                IndentGuide {
                    col,
                    glyph: INDENT_GUIDE_CHAR,
                    active: false,
                },
            );
            col += indent_width;
        }
    }

    // Suppress the first `skip_levels` indent positions
    // (`indent_width`, `2*indent_width`, …). Fixed by config —
    // not derived from what's visible — so a shallow file doesn't
    // lose its only guide just because the deeper levels weren't
    // present in the window.
    if skip_levels > 0 {
        let cutoff = skip_levels.saturating_mul(indent_width);
        for guides in map.values_mut() {
            guides.retain(|g| g.col > cutoff);
        }
    }

    // Active marking & (in p10k mode) bracket decoration.
    let active = active_scope_range(app, cursor_row, lines, tab_width, indent_width);
    if let Some((lo_active, hi_active, ac)) = active {
        // Envelope bounds use `s` (the scope's actual header row),
        // not `lo_active` (= s+1, the first body row), so the p10k
        // `╭─` corner can land on `s`. Line mode naturally clamps
        // away rows ≤ s when iterating, because no `│` exists at
        // the active col on the header row.
        let s = lo_active.saturating_sub(1);
        let (anim_top, anim_bot) = animation_envelope(
            &app.active_doc().indent_anim,
            (s, hi_active, ac),
            cursor_row,
            animation,
            animation_ms,
        );
        // Both modes anchor at `ac` — same col as the inactive
        // stair-step guides on body rows. The p10k corner glyphs
        // (`╭`, `╰`, `>`) that land on header/last-row content
        // (e.g. the `i` of `if`, the closing `}`) are silently
        // dropped by `render_line`; only the `│` middles survive
        // — that's the trade-off for keeping the bracket aligned
        // with the rest of the indent guides instead of in its
        // own offset lane.
        let _ = indent_width;
        match style {
            IndentGuideStyle::Line => {
                // Mark the cursor scope's own col (`ac`) active
                // on its body rows. `ac == 0` for top-level
                // scopes, which lights up the leftmost stair-step
                // guide (col 0) — same logic as deeper scopes,
                // just at level 0.
                let s = lo_active.saturating_sub(1);
                let row_lo = s.max(anim_top).max(scroll);
                let row_hi = hi_active
                    .min(anim_bot)
                    .min(last_visible.saturating_sub(1))
                    .min(line_count.saturating_sub(1));
                if row_lo > row_hi {
                    return map;
                }
                for row in row_lo..=row_hi {
                    if let Some(guides) = map.get_mut(&row) {
                        for g in guides.iter_mut() {
                            if g.col == ac {
                                g.active = true;
                            }
                        }
                    }
                }
            }
            IndentGuideStyle::P10k => {
                // p10k bracket sits two cells left of `ac`. When
                // `ac < 2` (level-1 scope with `indent_width = 2`,
                // or any top-level scope) it lands at col 0 —
                // corner glyphs on the header / closing rows there
                // collide with content and silently drop, but the
                // `│` middles still animate visibly through body
                // rows where col 0 is in leading whitespace.
                let p10k_col = ac.saturating_sub(2);
                let anim_s = s.max(anim_top);
                let anim_e = hi_active.min(anim_bot);
                let row_lo = anim_s.max(scroll);
                let row_hi = anim_e
                    .min(last_visible.saturating_sub(1))
                    .min(line_count.saturating_sub(1));
                if row_lo > row_hi {
                    return map;
                }
                let top_reached = anim_s == s;
                let bot_reached = anim_e == hi_active;
                for row in row_lo..=row_hi {
                    let glyph = if top_reached && row == s {
                        '╭'
                    } else if bot_reached && row == hi_active {
                        '╰'
                    } else {
                        INDENT_GUIDE_CHAR
                    };
                    push_unique_guide(
                        &mut map,
                        row,
                        IndentGuide {
                            col: p10k_col,
                            glyph,
                            active: true,
                        },
                    );
                }
                // Horizontal extensions on the corner rows.
                // Gated by `top_reached`/`bot_reached` so the
                // bracket grows in two clean steps during the
                // animation rather than baring its cap mid-flight.
                let in_view = |row: usize| -> bool {
                    row >= scroll && row < last_visible && row < line_count
                };
                if top_reached && in_view(s) {
                    push_unique_guide(
                        &mut map,
                        s,
                        IndentGuide {
                            col: p10k_col + 1,
                            glyph: '─',
                            active: true,
                        },
                    );
                }
                // Skip the `>` only for top-level scopes (ac=0)
                // where the bracket has no scope header to point
                // at. For nested scopes (ac >= indent_width) the
                // `>` lands in leading whitespace of the close
                // row even when p10k_col is 0 (e.g. `ac=2` with
                // `indent_width=2`).
                if bot_reached && in_view(hi_active) && ac > 0 {
                    push_unique_guide(
                        &mut map,
                        hi_active,
                        IndentGuide {
                            col: p10k_col + 1,
                            glyph: '>',
                            active: true,
                        },
                    );
                }
            }
        }
    }
    map
}

/// Animation envelope for the active scope's bracket/bar: the
/// (inclusive) row range that should currently be drawn as active.
///
/// When `enabled == false`, returns the scope's full span — the
/// bracket renders instantly.
///
/// When enabled, the envelope grows **top-to-bottom**: the `╭─`
/// corner appears immediately on the scope's start row and the
/// bar cascades downward to the `╰>` over `duration_ms`. Progress
/// `p = elapsed / duration_ms` is clamped to `[0, 1]`. At p = 0
/// only `scope.0` (the start row) is active; at p = 1 the full
/// `(scope.0, scope.1)` span is active and the cached state is
/// cleared so the loop can stop waking on the timer.
///
/// State is cached in the buffer's `indent_anim` `Cell` keyed by
/// the scope tuple. Any change to the key (cursor enters a
/// different scope) restarts the animation from the top.
pub(super) fn animation_envelope(
    state: &std::cell::Cell<Option<IndentAnimState>>,
    scope: (usize, usize, usize),
    cursor_row: usize,
    enabled: bool,
    duration_ms: u64,
) -> (usize, usize) {
    if !enabled || duration_ms == 0 {
        state.set(None);
        return (scope.0, scope.1);
    }
    let now = std::time::Instant::now();
    let cached = state.get();
    // Three cases:
    // 1. Cached key matches current scope, in-flight (Some t): keep ticking.
    // 2. Cached key matches current scope, settled (None t): hold full extent.
    // 3. Key differs (or no cache): start a fresh animation.
    let started_at = match cached {
        Some((Some(t), k, _)) if k == scope => Some(t),
        Some((None, k, _)) if k == scope => None,
        _ => {
            state.set(Some((Some(now), scope, cursor_row)));
            Some(now)
        }
    };
    let p = match started_at {
        Some(t) => {
            let elapsed_ms = now.duration_since(t).as_millis() as u64;
            (elapsed_ms as f32 / duration_ms as f32).clamp(0.0, 1.0)
        }
        None => 1.0,
    };
    let length = scope.1.saturating_sub(scope.0) as f32;
    let bot = scope.0.saturating_add((length * p).round() as usize);
    if p >= 1.0 && started_at.is_some() {
        // Transition to settled — keep the key cached (so we detect
        // future scope changes) but drop the timer so the main loop
        // stops waking at 60fps.
        state.set(Some((None, scope, cursor_row)));
    }
    (scope.0, bot)
}

/// Active scope range as `(first_body_row, last_body_row,
/// active_col)`. Tree-sitter innermost scope containing the cursor
/// when available; synthetic indent-run otherwise.
pub(super) fn active_scope_range(
    app: &App,
    cursor_row: usize,
    lines: &[String],
    tab_width: usize,
    indent_width: usize,
) -> Option<(usize, usize, usize)> {
    if let Some(h) = app.active_doc().highlighter.as_ref() {
        let scopes = h.indent_scopes_in_rows(cursor_row, cursor_row);
        // Innermost = smallest span containing the cursor.
        // Header (start) row counts as inside so moving from
        // `if y:` into its body doesn't reassign which level is
        // active. Top-level scopes (header indent 0) are kept
        // — their `ac` of 0 lights up col 0 in the stair-step,
        // which makes the leftmost guide participate in the
        // active highlight (and its animation) just like deeper
        // levels.
        let mut best: Option<(usize, usize)> = None;
        for (s, e) in scopes {
            if cursor_row >= s && cursor_row <= e {
                match best {
                    None => best = Some((s, e)),
                    Some((bs, be)) if (e - s) < (be - bs) => best = Some((s, e)),
                    _ => {}
                }
            }
        }
        if let Some((s, e)) = best {
            let col = leading_indent_visual(&lines[s], tab_width).unwrap_or(0);
            return Some((s + 1, e, col));
        }
    }

    // Synthetic fallback: contiguous run of rows at or below the
    // cursor's indent level. Used for plain-text buffers and for
    // tree-sitter buffers whose innermost containing scope is at
    // column 0 (top-level).
    let cursor_indent = match leading_indent_visual(&lines[cursor_row], tab_width) {
        Some(v) => v,
        None => {
            let above = (0..cursor_row)
                .rev()
                .find_map(|r| leading_indent_visual(&lines[r], tab_width))
                .unwrap_or(0);
            let below = (cursor_row + 1..lines.len())
                .find_map(|r| leading_indent_visual(&lines[r], tab_width))
                .unwrap_or(0);
            above.min(below)
        }
    };
    if cursor_indent < indent_width {
        return None;
    }
    let active_col = ((cursor_indent - 1) / indent_width) * indent_width;
    if active_col == 0 {
        return None;
    }
    let threshold = active_col + indent_width;
    let n = lines.len();
    let mut s = cursor_row;
    while s > 0 {
        match leading_indent_visual(&lines[s - 1], tab_width) {
            Some(i) if i >= threshold => s -= 1,
            None => s -= 1,
            _ => break,
        }
    }
    let mut e = cursor_row;
    while e + 1 < n {
        match leading_indent_visual(&lines[e + 1], tab_width) {
            Some(i) if i >= threshold => e += 1,
            None => e += 1,
            _ => break,
        }
    }
    Some((s, e, active_col))
}

pub(super) fn push_unique_guide(map: &mut GuideMap, row: usize, guide: IndentGuide) {
    let entry = map.entry(row).or_default();
    // When two scopes report the same column the later one wins on
    // active flag and on glyph — the p10k decorator passes through
    // here to upgrade plain `│` cells into corner/arrow glyphs.
    if let Some(existing) = entry.iter_mut().find(|g| g.col == guide.col) {
        if guide.active {
            existing.active = true;
        }
        if guide.glyph != INDENT_GUIDE_CHAR {
            existing.glyph = guide.glyph;
        }
        return;
    }
    entry.push(guide);
}
