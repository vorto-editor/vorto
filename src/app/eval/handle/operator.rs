//! `Expr::Op` evaluation — apply an operator (`d`/`y`/`c`/`>`/`<`) over a
//! target derived from a motion, a search match, a text object, or the
//! line-wise repeat (`dd`/`yy`/…). The shared [`apply_op_range`]
//! collapses motion + text-object + search-match dispatch onto one
//! range-based primitive.

use super::{cursor_to_first_non_blank, resolve_motion_pure};
use crate::action::{Operator, Target};
use crate::app::App;
use crate::app::eval::is_inclusive_motion;
use crate::editor::Cursor;
use crate::effect::Cmd;
use crate::mode::Mode;

pub(super) fn handle_op(app: &mut App, op: Operator, target: Target, outer_count: u32) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    match target {
        Target::LineWise => {
            if matches!(op, Operator::Indent | Operator::Dedent) {
                let indent = app.indent_settings();
                let start_row = app.editor.cursor.row;
                let last = app.active_doc().lines.len().saturating_sub(1);
                let span = outer_count.max(1) as usize - 1;
                let end_row = start_row.saturating_add(span).min(last);
                for r in start_row..=end_row {
                    if matches!(op, Operator::Indent) {
                        ed_op!(app, indent_line(r, indent));
                    } else {
                        ed_op!(app, dedent_line(r, indent));
                    }
                }
                app.editor.cursor.row = start_row;
                let __r = app.editor.doc.clone();
                let __doc = app.documents.get(&__r).expect("active doc present");
                cursor_to_first_non_blank(&mut app.editor, __doc);
            } else if matches!(op, Operator::Comment | Operator::BlockComment) {
                let rows = comment_target_rows(app, outer_count);
                apply_comment_op(app, op, &rows, None, &mut cmds);
            } else {
                for _ in 0..outer_count {
                    match op {
                        Operator::Delete => ed_op!(app, delete_line()),
                        Operator::Yank => {
                            ed_op!(app, yank_line());
                            cmds.push(Cmd::SyncYank);
                            cmds.push(Cmd::ToastInfo("yanked".into()));
                        }
                        Operator::Change => {
                            cmds.push(Cmd::ToastError("change not implemented yet".into()));
                        }
                        Operator::Indent
                        | Operator::Dedent
                        | Operator::Comment
                        | Operator::BlockComment => unreachable!(),
                    }
                }
            }
        }
        Target::Motion(m) => {
            let (resolved, last_find_update) = resolve_motion_pure(m.motion, app.last_find);
            if let Some(lf) = last_find_update {
                cmds.push(Cmd::SetLastFind(lf));
            }
            let Some(resolved) = resolved else {
                cmds.push(Cmd::ToastError("no previous find".into()));
                return cmds;
            };
            let inclusive = is_inclusive_motion(resolved);
            for _ in 0..outer_count {
                let start = app.editor.cursor;
                let end = {
                    let doc = app.active_doc();
                    let target = doc.motion_target(start, resolved, m.count);
                    // Vim's inclusive motions (`e`, `f<c>`, `t<c>`, …)
                    // include the landing char in the operator range;
                    // `apply_op_range` takes an exclusive end, so push
                    // one past for these.
                    if inclusive {
                        doc.advance_one(target)
                    } else {
                        target
                    }
                };
                apply_op_range(app, op, start, end, &mut cmds);
            }
        }
        Target::SearchMatch { reverse } => {
            // The match range starts at the pattern hit, not at
            // the cursor — that's the whole point of having a
            // dedicated target. We read `app.search` and apply the
            // op to each match found in sequence; `outer_count > 1`
            // walks forward through successive matches (e.g. `2dgn`).
            let forward = app.search.last_forward ^ reverse;
            for _ in 0..outer_count {
                let Some((start, end_incl)) =
                    app.search
                        .find_match_range(&app.editor, app.active_doc(), forward)
                else {
                    cmds.push(Cmd::ToastError("pattern not found".into()));
                    break;
                };
                let end = app.active_doc().advance_one(end_incl);
                apply_op_range(app, op, start, end, &mut cmds);
            }
        }
        Target::TextObject { scope, object } => {
            for _ in 0..outer_count {
                match ed_op_ref!(app, text_object_range(scope, object)) {
                    Some((start, end)) => apply_op_range(app, op, start, end, &mut cmds),
                    None => {
                        cmds.push(Cmd::ToastError("no matching object".into()));
                        break;
                    }
                }
            }
        }
    }
    cmds
}

/// Apply an operator over the range [start, end). Shared by
/// motion-target, search-match, and text-object dispatch.
fn apply_op_range(app: &mut App, op: Operator, start: Cursor, end: Cursor, cmds: &mut Vec<Cmd>) {
    match op {
        Operator::Delete => ed_op!(app, delete_range(start, end)),
        Operator::Yank => {
            app.active_doc_mut().yank_range(start, end);
            cmds.push(Cmd::SyncYank);
            cmds.push(Cmd::ToastInfo("yanked".into()));
        }
        Operator::Change => {
            ed_op!(app, delete_range(start, end));
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        Operator::Indent | Operator::Dedent => {
            // `>` and `<` are line-wise even with a non-line target —
            // every row spanned by the motion gets one indent step.
            let indent = app.indent_settings();
            let (lo, hi) = if (start.row, start.col) <= (end.row, end.col) {
                (start.row, end.row)
            } else {
                (end.row, start.row)
            };
            for r in lo..=hi {
                if matches!(op, Operator::Indent) {
                    ed_op!(app, indent_line(r, indent));
                } else {
                    ed_op!(app, dedent_line(r, indent));
                }
            }
            app.editor.cursor.row = lo;
            let __r = app.editor.doc.clone();
            let __doc = app.documents.get(&__r).expect("active doc present");
            cursor_to_first_non_blank(&mut app.editor, __doc);
        }
        Operator::Comment => {
            let (lo, hi) = order_range(start, end);
            let rows: Vec<usize> = (lo.row..=hi.row).collect();
            apply_comment_op(app, op, &rows, None, cmds);
        }
        Operator::BlockComment => {
            let (lo, hi) = order_range(start, end);
            let rows: Vec<usize> = (lo.row..=hi.row).collect();
            apply_comment_op(app, op, &rows, Some((lo, hi)), cmds);
        }
    }
}

fn order_range(a: Cursor, b: Cursor) -> (Cursor, Cursor) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

/// Shared dispatch for `Operator::Comment` and `Operator::BlockComment`.
/// Delegates the mutation + token-fallback policy to `App` methods so
/// Visual mode picks up the same behavior; this function only owns the
/// Normal-mode-specific error surface (push `Cmd::ToastError`).
fn apply_comment_op(
    app: &mut App,
    op: Operator,
    rows: &[usize],
    range: Option<(Cursor, Cursor)>,
    cmds: &mut Vec<Cmd>,
) {
    let ok = match op {
        Operator::Comment => app.apply_line_comment(rows),
        Operator::BlockComment => app.apply_block_comment(rows, range),
        _ => unreachable!(),
    };
    if !ok {
        let msg = match op {
            Operator::Comment => "no comment token for this buffer",
            Operator::BlockComment => "no block- or line-comment tokens for this buffer",
            _ => unreachable!(),
        };
        cmds.push(Cmd::ToastError(msg.into()));
    }
}

/// Same row-collection logic as `direct.rs::comment_target_rows`: extra
/// cursors fan out; otherwise `count` rows starting at the primary.
fn comment_target_rows(app: &App, count: u32) -> Vec<usize> {
    if app.editor.extra_cursors.is_empty() {
        let start = app.editor.cursor.row;
        let max = app.active_doc().lines.len();
        (0..count as usize)
            .map(|i| start + i)
            .take_while(|&r| r < max)
            .collect()
    } else {
        std::iter::once(app.editor.cursor.row)
            .chain(app.editor.extra_cursors.iter().map(|c| c.row))
            .collect()
    }
}
