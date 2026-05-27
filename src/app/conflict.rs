//! `:conflict` — resolve the git conflict under the cursor.
//!
//! Parsing of the `<<<<<<<` … `>>>>>>>` shape lives in
//! [`crate::editor::conflict`] (pure, unit-tested); this method bridges
//! it to the live cursor / document and the toast queue. Resolution
//! (`ours`/`theirs`/`both`/`none`) acts on the conflict under the cursor
//! and goes through the undo stack like any edit. Navigation is on the
//! `]c` / `[c` keys (see [`App::run_goto_conflict`]), not a subcommand.

use crate::app::{App, Toast};
use crate::editor::conflict::Resolution;

impl App {
    /// `:conflict <ours|theirs|both|none>` — resolve the conflict under
    /// the cursor. Navigation is on the `]c` / `[c` keys, not here. A bare
    /// `:conflict` (no subcommand) just shows the available actions.
    /// Names/aliases resolve through the unified
    /// [`crate::config::CONFLICT_SUBCOMMANDS`] so the hint panel and the
    /// dispatcher never drift.
    pub(super) fn run_conflict_command(&mut self, sub: &str) {
        let sub = sub.trim();
        let (cmd, rest) = match sub.split_once(char::is_whitespace) {
            Some((c, r)) => (c, r.trim()),
            None => (sub, ""),
        };
        if cmd.is_empty() {
            self.push_toast(Toast::info(
                ":conflict ours | theirs | both | none (]c / [c to move)",
            ));
            return;
        }
        let Some(canonical) =
            crate::config::resolve_subcommand(crate::config::CONFLICT_SUBCOMMANDS, cmd)
        else {
            self.push_toast(Toast::error(format!("unknown conflict subcommand: {cmd}")));
            return;
        };
        // None of the subcommands take an argument — they act on the
        // cursor's conflict. Reject trailing tokens so a typo surfaces.
        if !rest.is_empty() {
            self.push_toast(Toast::error(format!(
                ":conflict {canonical} takes no arguments"
            )));
            return;
        }
        match canonical {
            "ours" => self.conflict_resolve(Resolution::Ours),
            "theirs" => self.conflict_resolve(Resolution::Theirs),
            "both" => self.conflict_resolve(Resolution::Both),
            "none" => self.conflict_resolve(Resolution::None),
            _ => unreachable!("canonical comes from CONFLICT_SUBCOMMANDS"),
        }
    }

    /// Collapse the conflict under the cursor to `res`, splicing the
    /// chosen side(s) in place of the whole `<<<<<<<` … `>>>>>>>` run.
    /// Takes an undo snapshot first (so a single `u` restores the
    /// markers) and leaves the cursor at the start of the resolved
    /// region. Toasts how many conflicts remain.
    fn conflict_resolve(&mut self, res: Resolution) {
        let row = self.editor.cursor.row;
        let hunk = self
            .active_doc()
            .conflict_hunks()
            .into_iter()
            .find(|h| h.contains(row));
        let Some(h) = hunk else {
            self.push_toast(Toast::warn(
                "no conflict at the cursor — use ]c / [c to jump to one",
            ));
            return;
        };

        // Splice the replacement in, threading the active document out of
        // the pool so the `&mut Editor` (snapshot) and `&mut Buffer`
        // borrows stay disjoint — same dance as the `ed_op!` macro.
        let doc_ref = self.editor.doc.clone();
        let buf = self
            .documents
            .get_mut(&doc_ref)
            .expect("active doc present in pool");
        let replacement = h.replacement(&buf.lines, res);
        self.editor.snapshot(buf);
        buf.lines.splice(h.start..=h.end, replacement);
        if buf.lines.is_empty() {
            buf.lines.push(String::new());
        }
        buf.dirty = true;
        buf.bump_version();

        // Land on the start of where the resolved region now sits.
        let last = self.active_doc().lines.len().saturating_sub(1);
        self.editor.cursor.row = h.start.min(last);
        self.editor.cursor.col = 0;
        ed_op_ref!(self, clamp_col(false));

        let kept = match res {
            Resolution::Ours => "kept ours",
            Resolution::Theirs => "kept theirs",
            Resolution::Both => "kept both sides",
            Resolution::None => "removed the conflict",
        };
        let remaining = self.active_doc().conflict_hunks().len();
        self.push_toast(Toast::info(if remaining == 0 {
            format!("{kept}; no conflicts left")
        } else {
            format!("{kept}; {remaining} conflict(s) left")
        }));
    }
}
