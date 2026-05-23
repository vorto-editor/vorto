//! vim-surround dispatch: `ys{target}{ch}` / `cs{from}{to}` / `ds{ch}`.
//!
//! All three commands collapse to a small set of buffer primitives:
//! resolve a range (via motion or text object), then either wrap it,
//! replace the wrapping pair, or strip it.

use super::resolve_motion_pure;
use crate::action::{Object, Scope, Target};
use crate::app::App;
use crate::app::eval::is_inclusive_motion;
use crate::editor::Cursor;
use crate::effect::Cmd;

pub(super) fn handle_add(app: &mut App, target: Target, ch: char) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    let Some((open, close)) = pair_for(ch) else {
        cmds.push(Cmd::ToastError(format!("no surround pair for `{}`", ch)));
        return cmds;
    };

    let range = match target {
        Target::Motion(m) => {
            let (resolved, last_find_update) = resolve_motion_pure(m.motion, app.last_find);
            if let Some(lf) = last_find_update {
                cmds.push(Cmd::SetLastFind(lf));
            }
            let Some(resolved) = resolved else {
                cmds.push(Cmd::ToastError("no previous find".into()));
                return cmds;
            };
            let start = app.buffer.cursor;
            let end_raw = app.buffer.motion_target(start, resolved, m.count);
            let end = if is_inclusive_motion(resolved) {
                app.buffer.advance_one(end_raw)
            } else {
                end_raw
            };
            Some(order(start, end))
        }
        Target::TextObject { scope, object } => app.buffer.text_object_range(scope, object),
        Target::LineWise => {
            let row = app.buffer.cursor.row;
            let line_len = app.buffer.lines[row].chars().count();
            Some((Cursor { row, col: 0 }, Cursor { row, col: line_len }))
        }
        Target::SearchMatch { .. } => {
            cmds.push(Cmd::ToastError("surround over search match not supported".into()));
            return cmds;
        }
    };

    let Some((lo, hi)) = range else {
        cmds.push(Cmd::ToastError("no matching object".into()));
        return cmds;
    };
    app.buffer.surround_wrap(&open, &close, lo, hi);
    cmds
}

pub(super) fn handle_delete(app: &mut App, ch: char) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    let Some(object) = object_for(ch) else {
        cmds.push(Cmd::ToastError(format!("no surround pair for `{}`", ch)));
        return cmds;
    };
    let Some((lo, hi)) = app.buffer.text_object_range(Scope::Around, object) else {
        cmds.push(Cmd::ToastError("no surrounding pair".into()));
        return cmds;
    };
    app.buffer.surround_strip(lo, hi);
    cmds
}

pub(super) fn handle_change(app: &mut App, from: char, to: char) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    let Some(object) = object_for(from) else {
        cmds.push(Cmd::ToastError(format!("no surround pair for `{}`", from)));
        return cmds;
    };
    let Some((new_open, new_close)) = pair_for(to) else {
        cmds.push(Cmd::ToastError(format!("no surround pair for `{}`", to)));
        return cmds;
    };
    let Some((lo, hi)) = app.buffer.text_object_range(Scope::Around, object) else {
        cmds.push(Cmd::ToastError("no surrounding pair".into()));
        return cmds;
    };
    app.buffer.surround_replace(lo, hi, &new_open, &new_close);
    cmds
}

/// Map a vim-surround char to the `(open, close)` literal pair to insert.
///
/// Unlike tpope's vim-surround we do *not* distinguish "spaced" vs
/// "tight" by which side of the bracket pair was typed: every variant
/// wraps tightly. Inserting interior spaces is a formatter's job, not
/// surround's. `b` / `B` remain aliases for `)` / `}` for muscle-memory.
fn pair_for(ch: char) -> Option<(String, String)> {
    Some(match ch {
        '(' | ')' | 'b' => ("(".into(), ")".into()),
        '{' | '}' | 'B' => ("{".into(), "}".into()),
        '[' | ']' => ("[".into(), "]".into()),
        '<' | '>' => ("<".into(), ">".into()),
        '"' => ("\"".into(), "\"".into()),
        '\'' => ("'".into(), "'".into()),
        '`' => ("`".into(), "`".into()),
        _ => return None,
    })
}

/// Map a vim-surround char to the text-object kind that locates the
/// existing pair around the cursor. Both opening and closing variants
/// of asymmetric brackets resolve to the same object.
fn object_for(ch: char) -> Option<Object> {
    Some(match ch {
        '(' | ')' | 'b' => Object::Paren,
        '{' | '}' | 'B' => Object::Brace,
        '[' | ']' => Object::Bracket,
        '<' | '>' => Object::AngleBracket,
        '"' => Object::DoubleQuote,
        '\'' => Object::SingleQuote,
        '`' => Object::Backtick,
        _ => return None,
    })
}

fn order(a: Cursor, b: Cursor) -> (Cursor, Cursor) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}
