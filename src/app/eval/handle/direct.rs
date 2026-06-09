//! `Expr::Direct` evaluation — the kitchen-sink dispatch for keystrokes
//! that don't fit the motion / operator framework (mode transitions,
//! prompt opens, buffer-wide ops, ex-style commands, multi-cursor and
//! window controls). Buffer mutations happen inline; everything else is
//! emitted as a `Cmd` for the runtime to apply.

use std::path::PathBuf;

use super::push_word_search;
use crate::action::{Ctx, DirectKind, PromptKind};
use crate::app::App;
use crate::app::eval::format_dirty_list;
use crate::buffer_ref::BufferRef;
use crate::editor::Cursor;
use crate::effect::{Cmd, ScrollAnchor};
use crate::mode::Mode;

pub(super) fn handle_direct(app: &mut App, kind: DirectKind, count: u32, ctx: Ctx) -> Vec<Cmd> {
    use DirectKind as D;
    let mut cmds = Vec::new();
    match kind {
        D::EnterMode(m) => cmds.push(Cmd::EnterMode(m)),
        D::OpenPrompt(k) => cmds.push(Cmd::OpenPrompt(k)),
        D::OpenLineBelow => {
            let indent = app.indent_settings();
            ed_op!(app, insert_line_below(indent));
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::OpenLineAbove => {
            let indent = app.indent_settings();
            ed_op!(app, insert_line_above(indent));
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::AppendAfterCursor => {
            // Past-the-end is allowed in Insert, so step right with
            // that permission rather than the Normal-mode clamp.
            ed_op_ref!(app, move_right(true));
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::AppendAtLineEnd => {
            app.editor.cursor.col = ed_op_ref!(app, current_line_len());
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::InsertAtLineStart => {
            let col = {
                let line = ed_op_ref!(app, current_line());
                line.chars().position(|c| !c.is_whitespace()).unwrap_or(0)
            };
            app.editor.cursor.col = col;
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::ChangeToEol => {
            ed_op!(app, delete_to_eol());
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::DeleteToEol => ed_op!(app, delete_to_eol()),
        D::YankLine => {
            // `NY` yanks N whole lines as one run — capture the span in a
            // single call so the register holds every line, not just the
            // last.
            let start = app.editor.cursor.row;
            let last = app.active_doc().lines.len().saturating_sub(1);
            let end = start.saturating_add(count.max(1) as usize - 1).min(last);
            app.active_doc_mut().yank_lines(start, end);
            cmds.push(Cmd::SyncYank);
            cmds.push(Cmd::ToastInfo("yanked".into()));
        }
        D::JoinLines => {
            for _ in 0..count {
                ed_op!(app, join_next_line());
            }
        }
        D::ToggleCase => {
            for _ in 0..count {
                ed_op!(app, toggle_case_under_cursor());
            }
        }
        D::SubstituteChar => {
            for _ in 0..count {
                ed_op!(app, delete_char_under_cursor());
            }
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::SubstituteLine => {
            ed_op!(app, clear_current_line());
            cmds.push(Cmd::EnterMode(Mode::Insert));
        }
        D::ReplaceChar { ch } => {
            for _ in 0..count {
                ed_op!(app, replace_char(ch));
                // After each replacement, vim leaves the cursor on
                // the replaced char; a count > 1 walks forward one
                // step per replacement.
                ed_op_ref!(app, move_right(false));
            }
            // Final cursor: vim leaves it on the LAST replaced
            // char, not past it.
            app.editor.move_left();
        }
        D::ViewportCenter => cmds.push(Cmd::Scroll(ScrollAnchor::Center)),
        D::ViewportTopAtCursor => cmds.push(Cmd::Scroll(ScrollAnchor::Top)),
        D::ViewportBottomAtCursor => cmds.push(Cmd::Scroll(ScrollAnchor::Bottom)),
        D::FoldToggle => app.fold_toggle_at_cursor(),
        D::FoldOpen => app.fold_open_at_cursor(),
        D::FoldClose => app.fold_close_at_cursor(),
        D::FoldOpenAll => app.fold_open_all(),
        D::FoldCloseAll => app.fold_close_all(),
        D::Paste => {
            // `Np` repeats the register as one contiguous block, so the
            // count goes *into* the paste rather than re-running it (which
            // would interleave a multi-line register).
            ed_op!(app, paste_after(count as usize));
        }
        D::Undo => {
            if !ed_op!(app, undo()) {
                cmds.push(Cmd::ToastError("already at oldest change".into()));
            }
        }
        D::Redo => {
            if !ed_op!(app, redo()) {
                cmds.push(Cmd::ToastError("already at newest change".into()));
            }
        }
        D::DeleteCharUnderCursor => {
            for _ in 0..count {
                ed_op!(app, delete_char_under_cursor());
            }
        }
        D::Quit => cmds.push(plan_quit(app)),
        D::QuitForce => cmds.push(Cmd::Quit),
        D::BufferNext => cmds.push(Cmd::BufferCycle { forward: true }),
        D::BufferPrev => cmds.push(Cmd::BufferCycle { forward: false }),
        D::BufferDelete => cmds.push(Cmd::BufferDelete { force: false }),
        D::BufferDeleteForce => cmds.push(Cmd::BufferDelete { force: true }),
        D::BufferDeleteAll => cmds.push(Cmd::BufferDeleteAll),
        D::BufferList => {
            cmds.push(Cmd::OpenPrompt(PromptKind::Fuzzy(
                crate::finder::FuzzyKind::Buffers,
            )));
        }
        D::NewScratchBuffer => cmds.push(Cmd::NewScratchBuffer),
        D::SaveAndQuit => cmds.push(Cmd::Save {
            path: parse_save_path(ctx.rest),
            then_quit: true,
            force: false,
        }),
        D::Save => cmds.push(Cmd::Save {
            path: parse_save_path(ctx.rest),
            then_quit: false,
            force: false,
        }),
        D::SaveForce => cmds.push(Cmd::Save {
            path: parse_save_path(ctx.rest),
            then_quit: false,
            force: true,
        }),
        D::Open => {
            if ctx.rest.is_empty() {
                cmds.push(Cmd::ToastError("missing path".into()));
            } else {
                cmds.push(Cmd::OpenPath(PathBuf::from(ctx.rest)));
            }
        }
        D::OpenLog => match crate::log::default_path() {
            Some(p) => cmds.push(Cmd::OpenPath(p)),
            None => cmds.push(Cmd::ToastError("log path unresolved".into())),
        },
        D::Reload => cmds.push(Cmd::Reload),
        D::ReloadAll => cmds.push(Cmd::ReloadAll),
        D::ConfigReload => cmds.push(Cmd::ConfigReload),
        D::GotoLine => match ctx.rest.parse::<usize>() {
            Ok(n) if n >= 1 => app.goto_line_n_pure(n),
            _ => cmds.push(Cmd::ToastError("usage: :goto <line>".into())),
        },
        D::GotoDefinition => cmds.push(Cmd::LspJump {
            method: "textDocument/definition",
            label: "definition",
        }),
        D::GotoDeclaration => cmds.push(Cmd::LspJump {
            method: "textDocument/declaration",
            label: "declaration",
        }),
        D::GotoImplementation => cmds.push(Cmd::LspJump {
            method: "textDocument/implementation",
            label: "implementation",
        }),
        D::FindReferences => cmds.push(Cmd::LspFindReferences),
        D::Rename => cmds.push(Cmd::OpenRenamePrompt),
        D::CodeAction => cmds.push(Cmd::LspCodeAction),
        D::Hover => cmds.push(Cmd::LspHover),
        D::LspStatus => {
            // `:lsp all` / `:lsp a` widens the scope to every
            // configured language. Anything else (including the
            // common no-arg case) is buffer-scoped — `open_lsp_status`
            // filters down to the active file's language.
            let all = matches!(ctx.rest.trim(), "all" | "a");
            cmds.push(Cmd::OpenLspStatus { all });
        }
        D::GotoDiagnostic { forward } => {
            cmds.push(Cmd::GotoDiagnostic { forward, count });
        }
        D::GotoConflict { forward } => {
            cmds.push(Cmd::GotoConflict { forward, count });
        }
        // Intercepted by `App::evaluate` before reaching here.
        D::RepeatLast => unreachable!("RepeatLast handled in App::evaluate"),
        D::SearchSelectNext { reverse } => {
            cmds.push(Cmd::SearchSelectMatch { reverse });
        }
        D::SearchWordKeep { forward } => {
            push_word_search(app, &mut cmds, forward, false);
        }
        D::ClearSearch => {
            cmds.push(Cmd::SetSearch {
                pattern: String::new(),
                forward: true,
            });
        }
        D::Substitute => run_substitute(app, ctx.rest, &mut cmds),
        D::MultiCursorAddNext => add_next_cursor(app, &mut cmds),
        D::MultiCursorAddBelow => add_cursor_below(app, &mut cmds),
        D::MultiCursorPop => {
            if let Some(c) = app.editor.extra_cursors.pop() {
                app.editor.cursor = c;
            } else {
                cmds.push(Cmd::ToastInfo("no extra cursor to remove".into()));
            }
        }
        D::MultiCursorClear => {
            if app.editor.extra_cursors.is_empty() {
                cmds.push(Cmd::ToastInfo("no extra cursors".into()));
            } else {
                let n = app.editor.extra_cursors.len();
                app.editor.extra_cursors.clear();
                cmds.push(Cmd::ToastInfo(format!("cleared {n} extra cursors")));
            }
        }
        D::BookmarkAdd => app.add_current_bookmark(),
        D::BookmarkRemoveCurrent => app.remove_current_bookmark(),
        D::JumpLabel => cmds.push(Cmd::StartJumpLabel),
        D::SelectWholeBuffer => cmds.push(Cmd::SelectWholeBuffer),
        D::SplitWindowHorizontal => cmds.push(Cmd::SplitWindow {
            dir: crate::app::SplitDir::Horizontal,
        }),
        D::SplitWindowVertical => cmds.push(Cmd::SplitWindow {
            dir: crate::app::SplitDir::Vertical,
        }),
        D::CloseWindow => cmds.push(Cmd::CloseWindow),
        D::FocusWindow { dir } => cmds.push(Cmd::FocusWindow { dir }),
        D::CycleWindow => cmds.push(Cmd::CycleWindow),
        D::JumpBack => cmds.push(Cmd::JumpBack { count }),
        D::JumpForward => cmds.push(Cmd::JumpForward { count }),
        D::JumpList => {
            cmds.push(Cmd::OpenPrompt(PromptKind::Fuzzy(
                crate::finder::FuzzyKind::Jumps,
            )));
        }
    }
    cmds
}

/// Plan the response to a bare `:q`. Refuses with an error status
/// while there are unsaved edits in the active or any sleeping
/// buffer; otherwise emits the actual quit command.
fn plan_quit(app: &App) -> Cmd {
    if app.active_doc().dirty {
        return Cmd::ToastError("unsaved changes (use :q!)".into());
    }
    let sleeping_dirty: Vec<&BufferRef> = app
        .sleeping
        .iter()
        .filter(|(_, b)| b.dirty)
        .map(|(r, _)| r)
        .collect();
    if !sleeping_dirty.is_empty() {
        return Cmd::ToastError(format!(
            "unsaved changes in {} (use :q!)",
            format_dirty_list(&sleeping_dirty)
        ));
    }
    Cmd::Quit
}

/// `<C-n>` body. Pulls the word under the cursor, finds its next
/// occurrence forward from primary (wrapping around the buffer), and
/// pushes primary as a new extra cursor before jumping primary to the
/// match. Also seeds `App.search` via `Cmd::SetSearch` so `n` / `N`
/// keep working on the same pattern after the user is done adding
/// cursors. When the cursor isn't on a word (e.g. sitting on `[`),
/// drops into Visual mode at the current position instead of erroring
/// — the user can extend the selection and try again, or just operate
/// on the highlighted char. No-ops with a status message when the
/// next match would land on a cursor that's already tracked.
fn add_next_cursor(app: &mut App, cmds: &mut Vec<Cmd>) {
    let Some(word) = crate::app::eval::word_under_cursor(&app.editor, app.active_doc()) else {
        app.enter_mode(Mode::Visual);
        return;
    };
    // Use a throwaway SearchState for the lookup so we can act on the
    // result this turn — `Cmd::SetSearch` is only applied after
    // `handle_expr` returns, so reading `app.search` here would see
    // the pre-Ctrl-N pattern.
    let mut tmp = crate::editor::SearchState::default();
    tmp.set(word.clone(), true);
    let Some(next) = tmp.find_next(&app.editor, app.active_doc(), true) else {
        cmds.push(Cmd::ToastError("no further match".into()));
        return;
    };
    let primary = app.editor.cursor;
    if next == primary || app.editor.extra_cursors.contains(&next) {
        cmds.push(Cmd::ToastInfo("no further match".into()));
        return;
    }
    app.editor.extra_cursors.push(primary);
    app.editor.cursor = next;
    cmds.push(Cmd::SetSearch {
        pattern: word,
        forward: true,
    });
    let n = app.editor.extra_cursors.len() + 1;
    cmds.push(Cmd::ToastInfo(format!("{n} cursors")));
}

/// Push the current primary into `extra_cursors` and move primary one
/// row down at the same column (clamped to the new line's length).
/// No-ops with a toast when already on the last line, or when the
/// landing cursor would collide with primary or an existing extra.
fn add_cursor_below(app: &mut App, cmds: &mut Vec<Cmd>) {
    let primary = app.editor.cursor;
    if primary.row + 1 >= app.active_doc().lines.len() {
        cmds.push(Cmd::ToastError("no line below".into()));
        return;
    }
    let next_row = primary.row + 1;
    let next_line_len = app.active_doc().lines[next_row].chars().count();
    let next_col = primary.col.min(next_line_len.saturating_sub(1));
    let next = Cursor {
        row: next_row,
        col: next_col,
    };
    if next == primary || app.editor.extra_cursors.contains(&next) {
        cmds.push(Cmd::ToastInfo("cursor already there".into()));
        return;
    }
    app.editor.extra_cursors.push(primary);
    app.editor.cursor = next;
    let n = app.editor.extra_cursors.len() + 1;
    cmds.push(Cmd::ToastInfo(format!("{n} cursors")));
}

fn parse_save_path(rest: &str) -> Option<PathBuf> {
    if rest.is_empty() {
        None
    } else {
        Some(PathBuf::from(rest))
    }
}

/// Body of `:s/pat/repl/[g]` and `:%s/...`. Parses the raw command
/// string off `ctx.rest`, falls back to the active search pattern when
/// the user passed an empty pattern (vim convention: `:%s//new/g`
/// after a `/old`), applies the substitution against the buffer, and
/// pushes a status toast plus a `SetSearch` so `n`/`hlsearch` track
/// what was replaced.
fn run_substitute(app: &mut App, raw: &str, cmds: &mut Vec<Cmd>) {
    let Some(parsed) = crate::editor::parse_substitute(raw) else {
        cmds.push(Cmd::ToastError("usage: :s/pat/repl/[g]".into()));
        return;
    };
    let args = match parsed {
        Ok(a) => a,
        Err(msg) => {
            cmds.push(Cmd::ToastError(msg.into()));
            return;
        }
    };

    // Empty pattern → reuse the last search pattern. Saves typing
    // after `/foo<CR>:%s//bar/g`.
    let fallback;
    let pattern = if args.pattern.is_empty() {
        if app.search.query.is_empty() {
            cmds.push(Cmd::ToastError("no previous search pattern".into()));
            return;
        }
        fallback = app.search.query.clone();
        fallback.as_str()
    } else {
        args.pattern
    };

    let resolved = crate::editor::SubsArgs {
        range: args.range,
        pattern,
        replacement: args.replacement,
        global: args.global,
    };
    let outcome = ed_op!(app, substitute(&resolved));

    if outcome.matches == 0 {
        cmds.push(Cmd::ToastError(format!("pattern not found: {}", pattern)));
        return;
    }
    cmds.push(Cmd::SetSearch {
        pattern: pattern.to_string(),
        forward: true,
    });
    cmds.push(Cmd::ToastInfo(format!(
        "{} substitution{} on {} line{}",
        outcome.matches,
        if outcome.matches == 1 { "" } else { "s" },
        outcome.lines_changed,
        if outcome.lines_changed == 1 { "" } else { "s" },
    )));
}
