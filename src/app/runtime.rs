//! Apply a `Cmd` stream to `App`.
//!
//! The third stage of the input pipeline: where `handle_expr` produced a
//! list of non-buffer state changes, `run_cmds` actually performs them.
//! Most variants are thin shims over existing `App` helpers
//! (`enter_mode`, `open_prompt`, `buffer_cycle`, the `lsp_*` methods,
//! …); this module is the dispatcher, not the implementer.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use super::eval::word_under_cursor;
use super::{App, Toast, root_cause};
use crate::effect::{Cmd, ScrollAnchor};
use crate::lsp;

/// Upper bound on how long save waits for `textDocument/formatting`
/// before giving up and writing the un-formatted buffer. Generous
/// enough for rust-analyzer's first-format-after-startup; short enough
/// that a wedged server doesn't strand the user.
const LSP_FORMAT_TIMEOUT: Duration = Duration::from_secs(3);

impl App {
    pub(super) fn run_cmds(&mut self, cmds: Vec<Cmd>) -> Result<()> {
        for cmd in cmds {
            self.run_cmd(cmd)?;
        }
        Ok(())
    }

    fn run_cmd(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::EnterMode(m) => self.enter_mode(m),
            Cmd::ToastInfo(s) => self.push_toast(Toast::info(s)),
            Cmd::ToastError(s) => self.push_toast(Toast::error(s)),
            Cmd::OpenPrompt(k) => self.open_prompt(k),
            Cmd::OpenRenamePrompt => self.open_rename_prompt(),
            Cmd::SetSearch { pattern, forward } => self.search.set(pattern, forward),
            Cmd::JumpSearch { reverse } => {
                let forward = self.search.last_forward ^ reverse;
                self.run_jump_search(forward);
            }
            Cmd::SearchSelectMatch { reverse } => {
                let forward = self.search.last_forward ^ reverse;
                self.run_search_select(forward);
            }
            Cmd::SetLastFind(lf) => self.last_find = Some(lf),
            Cmd::Scroll(anchor) => self.run_scroll(anchor),
            Cmd::Save {
                path,
                then_quit,
                force,
            } => self.run_save(path.as_deref(), then_quit, force),
            Cmd::OpenPath(path) => self.open_path(&path)?,
            Cmd::Reload => self.run_reload(),
            Cmd::ReloadAll => self.run_reload_all(),
            Cmd::LspJump { method, label } => self.lsp_jump(method, label),
            Cmd::LspFindReferences => self.lsp_find_references(),
            Cmd::LspCodeAction => self.lsp_code_action(),
            Cmd::LspHover => self.lsp_hover(),
            Cmd::OpenLspStatus { all } => self.open_lsp_status(all),
            Cmd::GotoDiagnostic { forward, count } => self.run_goto_diagnostic(forward, count),
            Cmd::GotoConflict { forward, count } => self.run_goto_conflict(forward, count),
            Cmd::BufferCycle { forward } => self.buffer_cycle(forward)?,
            Cmd::BufferDelete { force } => self.buffer_delete(force)?,
            Cmd::BufferDeleteAll => self.buffer_delete_all()?,
            Cmd::NewScratchBuffer => {
                // Always mint a fresh id: `:new` is documented as "give
                // me a new buffer", which would feel broken if it kept
                // reusing the single Scratch(0) slot.
                let id = self.mint_scratch_id();
                self.switch_to_buffer(crate::buffer_ref::BufferRef::Scratch(id))?;
            }
            Cmd::Quit => self.should_quit = true,
            Cmd::StartJumpLabel => self.start_jump_label(),
            Cmd::JumpBack { count } => self.jump_back(count),
            Cmd::JumpForward { count } => self.jump_forward(count),
            Cmd::SelectWholeBuffer => self.run_select_whole_buffer(),
            Cmd::SyncYank => self.sync_yank_to_clipboard(),
            Cmd::SplitWindow { dir } => self.split_window(dir),
            Cmd::CloseWindow => self.close_window(),
            Cmd::FocusWindow { dir } => self.focus_window(dir),
            Cmd::CycleWindow => self.cycle_window(),
        }
        Ok(())
    }

    /// Push the current `Buffer.yank` onto the OS clipboard. Initializes
    /// the `arboard` handle on first use; both init failure and a failed
    /// `set_text` are swallowed silently so that headless / sandboxed
    /// environments don't surface a noisy error on every yank — the
    /// internal register keeps working and `p` paste-in-vorto is
    /// unaffected.
    pub(super) fn sync_yank_to_clipboard(&mut self) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        let yank = self.active_doc().yank.clone();
        if let Some(cb) = self.clipboard.as_mut() {
            let _ = cb.set_text(yank);
        }
    }

    /// `gA` — select every line in the buffer. Sets the visual anchor
    /// at (0, 0) directly rather than going through `enter_mode`, since
    /// the latter only pins the anchor on a Normal→Visual transition
    /// and we want a fresh selection even if we're already in some
    /// visual mode.
    fn run_select_whole_buffer(&mut self) {
        let last = self.active_doc().lines.len().saturating_sub(1);
        self.visual_anchor = Some(crate::editor::Cursor { row: 0, col: 0 });
        self.editor.mode = crate::mode::Mode::VisualLine;
        self.editor.cursor = crate::editor::Cursor { row: last, col: 0 };
    }

    /// `]d` / `[d` — move the cursor to the next/previous LSP
    /// diagnostic in the current buffer. Diagnostics are sorted by
    /// `(line, column)` upstream (in `lsp_coordinator`), so the walk
    /// is just a linear scan. Wraps at the buffer boundary so repeated
    /// presses cycle. Centers the viewport on the landing position
    /// and surfaces the diagnostic message as an info toast — the
    /// inline marker would otherwise be the only feedback that the
    /// jump fired, and that's easy to miss after a long jump.
    fn run_goto_diagnostic(&mut self, forward: bool, count: u32) {
        let Some(diags) = self.current_diagnostics() else {
            self.push_toast(Toast::info("no diagnostics"));
            return;
        };
        if diags.is_empty() {
            self.push_toast(Toast::info("no diagnostics"));
            return;
        }
        let n = diags.len();
        let cur = self.editor.cursor;
        // Index of the next match in `forward` direction strictly past
        // the cursor; falls back to wrap-around when the cursor sits
        // at/after the last (forward) or at/before the first (back).
        let start = if forward {
            diags
                .iter()
                .position(|d| {
                    let p = d.range.start;
                    (p.line as usize, p.character as usize) > (cur.row, cur.col)
                })
                .unwrap_or(0)
        } else {
            diags
                .iter()
                .rposition(|d| {
                    let p = d.range.start;
                    (p.line as usize, p.character as usize) < (cur.row, cur.col)
                })
                .unwrap_or(n - 1)
        };
        // Count: walk `count - 1` more steps in the same direction,
        // wrapping. count == 0 shouldn't reach here (the parser maps
        // bare `]d` to count == 1) but guard with max(1) just in case.
        let steps = count.max(1) as usize - 1;
        let idx = if forward {
            (start + steps) % n
        } else {
            // (start - steps) mod n, computed without going negative.
            (start + n - (steps % n)) % n
        };
        let target = &diags[idx];
        // Clamp against the live buffer — servers occasionally publish
        // diagnostics that point past EOF (in-flight edits race against
        // the next `publishDiagnostics`).
        let last_row = self.active_doc().lines.len().saturating_sub(1);
        let row = (target.range.start.line as usize).min(last_row);
        let col = target.range.start.character as usize;
        self.record_jump();
        self.editor.cursor = crate::editor::Cursor { row, col };
        ed_op_ref!(self, clamp_col(false));
        self.run_scroll(ScrollAnchor::Center);
        self.push_toast(Toast::info(target.message.clone()));
    }

    /// Body of `]c` / `[c`. Lands the cursor on the `<<<<<<<` line of the
    /// next / previous conflict, wrapping at
    /// the buffer boundary so repeated presses cycle. Centers the
    /// viewport and toasts the position (`conflict 2/3`) since a marker
    /// far off-screen would otherwise give no sign the jump fired.
    pub(super) fn run_goto_conflict(&mut self, forward: bool, count: u32) {
        let hunks = self.active_doc().conflict_hunks();
        if hunks.is_empty() {
            self.push_toast(Toast::info("no conflict markers in this buffer"));
            return;
        }
        let n = hunks.len();
        let row = self.editor.cursor.row;
        // First hunk strictly past the cursor (forward) or before it
        // (back); wrap to the far end when the cursor is already beyond
        // the last / first one.
        let start = if forward {
            hunks.iter().position(|h| h.start > row).unwrap_or(0)
        } else {
            hunks.iter().rposition(|h| h.start < row).unwrap_or(n - 1)
        };
        let steps = count.max(1) as usize - 1;
        let idx = if forward {
            (start + steps) % n
        } else {
            (start + n - (steps % n)) % n
        };
        let target = hunks[idx].start;
        self.record_jump();
        self.editor.cursor = crate::editor::Cursor {
            row: target,
            col: 0,
        };
        ed_op_ref!(self, clamp_col(false));
        self.run_scroll(ScrollAnchor::Center);
        self.push_toast(Toast::info(format!(
            "conflict {}/{} at line {}",
            idx + 1,
            n,
            target + 1
        )));
    }

    pub(super) fn run_jump_search(&mut self, forward: bool) {
        if let Some(c) = self
            .search
            .find_next(&self.editor, self.active_doc(), forward)
        {
            self.record_jump();
            self.editor.cursor = c;
        } else {
            self.push_toast(Toast::error("pattern not found"));
        }
    }

    /// Body of `gn` / `gN`. Looks up the next match in the requested
    /// direction; in Normal mode, drop the cursor on the match start
    /// and enter Visual (which pins the anchor there); in Visual,
    /// keep the existing anchor and only extend the active end. Either
    /// way, the cursor lands on the match's last char so the selection
    /// covers the whole match. Shared with Visual-mode key handling.
    pub(super) fn run_search_select(&mut self, forward: bool) {
        let Some((start, end_incl)) =
            self.search
                .find_match_range(&self.editor, self.active_doc(), forward)
        else {
            self.push_toast(Toast::error("pattern not found"));
            return;
        };
        if !self.editor.mode.is_visual() {
            self.record_jump();
            self.editor.cursor = start;
            self.enter_mode(crate::mode::Mode::Visual);
        }
        self.editor.cursor = end_incl;
    }

    /// Visual mode's `*` / `#` — extract the word under the cursor,
    /// seed the search state, then jump. The Normal-mode counterpart
    /// goes through `Cmd::SetSearch` + `Cmd::JumpSearch` from
    /// `handle_motion`; visual mode bypasses the Cmd pipeline so this
    /// shim collapses both into one call.
    pub(super) fn search_word_under_cursor(&mut self, forward: bool) {
        let Some(word) = word_under_cursor(&self.editor, self.active_doc()) else {
            self.push_toast(Toast::error("no word under cursor"));
            return;
        };
        self.search.set(word, forward);
        self.run_jump_search(forward);
    }

    pub(super) fn run_scroll(&mut self, anchor: ScrollAnchor) {
        let height = self.active_doc().viewport_height.get();
        if height == 0 {
            // Viewport size isn't known yet (most often: we just thawed
            // a sleeping buffer, which resets `viewport_height` to 0 by
            // design — see `SleepingBuffer::thaw`). Defer to the next
            // draw via `pending_center`; the sticky scroll path in
            // `compute_scroll` reads and clears it.
            if matches!(anchor, ScrollAnchor::Center) {
                self.active_doc().pending_center.set(true);
            }
            return;
        }
        let cur = self.editor.cursor.row;
        let last = self.active_doc().lines.len().saturating_sub(1);
        let scroll = match anchor {
            ScrollAnchor::Top => cur,
            ScrollAnchor::Center => cur.saturating_sub(height / 2),
            ScrollAnchor::Bottom => cur + 1 - height.min(cur + 1),
        };
        let max_scroll = last.saturating_sub(height.saturating_sub(1));
        self.active_doc().scroll.set(scroll.min(max_scroll));
    }

    /// Persist the active buffer to disk and, when `then_quit`, set
    /// `should_quit` only if the write succeeded. A failed save
    /// (no file name, missing parent dir, permission denied, …)
    /// surfaces as a toast and the editor stays open — propagating
    /// the error would tear down the run loop, which is the wrong
    /// response to a fat-fingered path. `force` enables `:w!`
    /// semantics: missing parent directories are created instead of
    /// reported as an error.
    fn run_save(&mut self, path: Option<&Path>, then_quit: bool, force: bool) {
        // Format-on-save runs only for in-place saves (not `:w <path>`):
        // for a save-as, the buffer's current language is ambiguous
        // with respect to the new path, and we'd rather avoid surprising
        // the user by rewriting their text right before changing where
        // it lives. In-place saves go through the formatter step,
        // which is no-op when no formatter is configured and no LSP
        // is attached.
        if path.is_none() && self.active_doc().path.is_some() {
            self.run_format_on_save();
        }

        let target = path
            .map(|p| p.to_path_buf())
            .or_else(|| self.active_doc().path.clone());
        let Some(target) = target else {
            self.push_toast(Toast::error("no file name (use :w <path>)"));
            return;
        };

        // External-edit guard: only for in-place saves where we have a
        // baseline `disk_meta` from load/reload. `:w <path>` deliberately
        // skips this — saving *to* a different file shouldn't be gated
        // on the original file's drift state. `:w!` bypasses too, since
        // forcing a write past a missing parent dir already implies
        // "I know what I'm doing." If the file has since vanished we
        // also refuse (without force) — silently re-creating it would
        // mask an external `rm`.
        if path.is_none()
            && !force
            && let Some(expected) = self.active_doc().disk_meta
        {
            match crate::editor::FileMeta::of(&target) {
                Some(actual) if actual != expected => {
                    self.push_toast(Toast::error(
                        "file changed on disk since open (use :w! or :reload)",
                    ));
                    return;
                }
                None => {
                    self.push_toast(Toast::error("file no longer on disk (use :w! to recreate)"));
                    return;
                }
                _ => {}
            }
        }

        let parent_missing = target
            .parent()
            .map(|p| !p.as_os_str().is_empty() && !p.exists())
            .unwrap_or(false);
        if parent_missing {
            if !force {
                self.push_toast(Toast::error(format!(
                    "no such directory: {} (use :w!)",
                    target.parent().unwrap().display()
                )));
                return;
            }
            if let Err(e) = std::fs::create_dir_all(target.parent().unwrap()) {
                self.push_toast(Toast::error(format!(
                    "mkdir {}: {}",
                    target.parent().unwrap().display(),
                    e
                )));
                return;
            }
        }

        let result = if path.is_some() {
            self.active_doc_mut().save_as(&target)
        } else {
            self.active_doc_mut().save()
        };
        let wrote = match result {
            Ok(()) => {
                let msg = if path.is_some() {
                    format!("written to {}", target.display())
                } else {
                    "written".to_string()
                };
                self.push_toast(Toast::info(msg));
                true
            }
            Err(e) => {
                self.push_toast(Toast::error(format!("save: {}", root_cause(&e))));
                false
            }
        };
        if wrote {
            // Many servers (rust-analyzer in particular) only re-run
            // their full checker on save, so this notify is what makes
            // fresh diagnostics arrive.
            self.run_notify_lsp_save();
            if then_quit {
                self.should_quit = true;
            }
        }
    }

    /// External formatter > LSP `textDocument/formatting` > no-op.
    /// Errors surface as toasts but never abort the save: the user
    /// asked to save and we'd rather write the un-formatted bytes
    /// than refuse the action. Format failures during save (e.g.
    /// rustfmt rejecting a syntax error) are common enough that
    /// blocking the save would be hostile.
    fn run_format_on_save(&mut self) {
        let eff = self.effective_editor();
        if !eff.format_on_save {
            return;
        }
        let formatter = self
            .active_doc()
            .path
            .as_deref()
            .and_then(|p| self.config.languages.by_path(p))
            .and_then(|lang| lang.formatter.clone());

        // A configured formatter wins when present: it's the user's
        // explicit choice, and the LSP would typically just shell out
        // to the same tool anyway (gopls → gofmt, rust-analyzer →
        // rustfmt).
        match formatter {
            Some(crate::config::Formatter::Command(fmt)) => {
                let cwd = self
                    .active_doc()
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| self.lsp.startup_cwd().to_path_buf());
                let text = self.active_doc().lines.join("\n");
                match crate::format::run_external(&fmt, &text, &cwd) {
                    Ok(formatted) => self.apply_formatted_text(formatted),
                    Err(e) => {
                        self.push_toast(Toast::fatal(format!(
                            "format `{}`: {}",
                            fmt.command,
                            root_cause(&e)
                        )));
                    }
                }
            }
            // Format via the named LSP servers in priority order. An
            // `Ok(None)` means none of the configured servers matched an
            // attached client — a typo in the name, or a server that
            // isn't in this language's `lsp` list (so it never attaches)
            // / hasn't finished starting up. Surface a toast rather than
            // silently skipping the format, since "save does nothing" is
            // otherwise impossible to diagnose.
            Some(crate::config::Formatter::Lsp(servers)) => {
                let options = self.formatting_options();
                match self
                    .lsp
                    .format_with_servers(&servers, options, LSP_FORMAT_TIMEOUT)
                {
                    Ok(Some(edits)) if !edits.is_empty() => self.apply_format_edits(edits),
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        self.push_toast(Toast::warn(format!(
                            "format: no configured formatter LSP attached ({})",
                            servers.join(", ")
                        )));
                    }
                    Err(e) => {
                        self.push_toast(Toast::fatal(format!("lsp format: {}", root_cause(&e))));
                    }
                }
            }
            // Fall through to LSP. `format_first_client` returns Ok(None)
            // when no client is attached — quietly do nothing in that
            // case so saves on plain-text buffers don't surface noise.
            None => {
                let options = self.formatting_options();
                match self.lsp.format_first_client(options, LSP_FORMAT_TIMEOUT) {
                    Ok(Some(edits)) if !edits.is_empty() => self.apply_format_edits(edits),
                    Ok(_) => {}
                    Err(e) => {
                        self.push_toast(Toast::fatal(format!("lsp format: {}", root_cause(&e))));
                    }
                }
            }
        }
    }

    /// Replace the buffer's text wholesale with the external
    /// formatter's stdout. Snapshots first so undo lands on the
    /// pre-format state. Cursor is clamped — the formatter typically
    /// only adds/removes whitespace so the row is usually still valid,
    /// but a wholesale rewrite is allowed to break that.
    fn apply_formatted_text(&mut self, formatted: String) {
        let new_lines: Vec<String> = formatted.split('\n').map(|s| s.to_string()).collect();
        let new_lines = if new_lines.is_empty() {
            vec![String::new()]
        } else {
            // External formatters typically end output with a trailing
            // newline, which `split('\n')` turns into a stray empty
            // last element. Drop it so the buffer doesn't grow an
            // extra blank line on every save.
            let mut v = new_lines;
            if v.len() > 1 && v.last().map(|s| s.is_empty()).unwrap_or(false) {
                v.pop();
            }
            v
        };
        if new_lines == self.active_doc().lines {
            return;
        }
        ed_op!(self, snapshot());
        let doc = self.active_doc_mut();
        doc.lines = new_lines;
        doc.bump_version();
        doc.dirty = true;
        self.clamp_cursor_to_buffer();
    }

    /// Apply a list of LSP `TextEdit`s to the buffer. Snapshots first
    /// so undo lands on the pre-format state; bumps the version so
    /// the highlighter re-runs against the rewritten text.
    fn apply_format_edits(&mut self, edits: Vec<lsp::TextEdit>) {
        ed_op!(self, snapshot());
        let doc = self.active_doc_mut();
        let mut lines = std::mem::take(&mut doc.lines);
        lsp::apply_text_edits(&mut lines, edits);
        if lines.is_empty() {
            lines.push(String::new());
        }
        let doc = self.active_doc_mut();
        doc.lines = lines;
        doc.bump_version();
        doc.dirty = true;
        self.clamp_cursor_to_buffer();
    }

    /// Pin the cursor inside the (possibly shrunken) buffer after a
    /// format rewrite. Conservative: just clamps row/col without
    /// trying to track the cursor's logical position through the
    /// edit — formatters mostly preserve structure, and the user
    /// can scroll back if the cursor lands somewhere unexpected.
    fn clamp_cursor_to_buffer(&mut self) {
        let last_row = self.active_doc().lines.len().saturating_sub(1);
        if self.editor.cursor.row > last_row {
            self.editor.cursor.row = last_row;
        }
        let row_len = self
            .active_doc()
            .lines
            .get(self.editor.cursor.row)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        if self.editor.cursor.col > row_len {
            self.editor.cursor.col = row_len;
        }
    }

    /// LSP `FormattingOptions` derived from the buffer's effective
    /// editor settings. Servers honour these to pick tab vs. space
    /// (gopls in particular needs `insertSpaces: false`).
    fn formatting_options(&self) -> serde_json::Value {
        let eff = self.effective_editor();
        serde_json::json!({
            "tabSize": eff.indent_width,
            "insertSpaces": !eff.use_tabs,
            "trimTrailingWhitespace": true,
            "insertFinalNewline": true,
            "trimFinalNewlines": true,
        })
    }

    /// `:reload` — re-read the active buffer's backing file from
    /// disk. Always reloads: `reload_from_disk` takes an undo
    /// snapshot before the content is replaced, so unsaved edits
    /// are recoverable with `u`. The version bump triggers the
    /// normal `didChange` sync so the LSP picks up the new content
    /// on the next tick.
    fn run_reload(&mut self) {
        let Some(path) = self.active_doc().path.clone() else {
            self.push_toast(Toast::error("no file name"));
            return;
        };
        match ed_op!(self, reload_from_disk()) {
            Ok(true) => self.push_toast(Toast::info(format!("reloaded {}", path.display()))),
            Ok(false) => self.push_toast(Toast::info("reloaded (no change)")),
            Err(e) => self.push_toast(Toast::error(format!("reload: {}", root_cause(&e)))),
        }
    }

    /// `:reload-all` — re-read every file-backed buffer.
    ///
    /// - Active and parked buffers always reload: `reload_from_disk`
    ///   snapshots first, so unsaved edits are recoverable via `u`
    ///   in each pane.
    /// - Clean sleeping entries are dropped so the next visit
    ///   triggers a fresh load; cheaper than thawing them just to
    ///   reload them back.
    /// - Dirty sleeping entries are left alone — their unsaved
    ///   edits live only inside the frozen snapshot, with no
    ///   undo stack reachable from the active pane, so dropping
    ///   would be unrecoverable data loss. They surface unchanged
    ///   on the next visit.
    fn run_reload_all(&mut self) {
        let mut reloaded = 0usize;
        let mut unchanged = 0usize;
        let mut errors: Vec<String> = Vec::new();

        if self.active_doc().path.is_some() {
            match ed_op!(self, reload_from_disk()) {
                Ok(true) => reloaded += 1,
                Ok(false) => unchanged += 1,
                Err(e) => errors.push(format!("active: {}", root_cause(&e))),
            }
        }

        // Inactive panes' sessions, each resolving its own document.
        // Two panes sharing one document reload it twice — the second
        // pass sees disk == buffer and reports "unchanged", so it's a
        // harmless no-op (no double snapshot).
        let inactive_panes: Vec<crate::app::PaneId> = self
            .pane_content
            .iter()
            .filter_map(|(id, c)| match c {
                crate::app::PaneContent::Editor(_) => Some(*id),
                crate::app::PaneContent::Agent => None,
            })
            .collect();
        for pane in inactive_panes {
            let doc_ref = match self.pane_content.get(&pane) {
                Some(crate::app::PaneContent::Editor(ed)) => ed.doc.clone(),
                _ => continue,
            };
            let has_path = self
                .documents
                .get(&doc_ref)
                .map(|d| d.path.is_some())
                .unwrap_or(false);
            if !has_path {
                continue;
            }
            // Disjoint fields: the pane's editor and its pooled document.
            let Some(crate::app::PaneContent::Editor(mut ed)) = self.pane_content.remove(&pane)
            else {
                continue;
            };
            let result = {
                let doc = self
                    .documents
                    .get_mut(&doc_ref)
                    .expect("pane doc present in pool");
                ed.reload_from_disk(doc)
            };
            self.pane_content
                .insert(pane, crate::app::PaneContent::Editor(ed));
            match result {
                Ok(true) => reloaded += 1,
                Ok(false) => unchanged += 1,
                Err(e) => {
                    let label = match &doc_ref {
                        crate::buffer_ref::BufferRef::File(p) => p.display().to_string(),
                        crate::buffer_ref::BufferRef::Scratch(id) => {
                            crate::buffer_ref::BufferRef::scratch_label(*id)
                        }
                    };
                    errors.push(format!("{label}: {}", root_cause(&e)));
                }
            }
        }

        let clean_keys: Vec<_> = self
            .sleeping
            .iter()
            .filter_map(|(k, s)| match k {
                crate::buffer_ref::BufferRef::File(_) if !s.dirty => Some(k.clone()),
                _ => None,
            })
            .collect();
        let preserved_dirty = self
            .sleeping
            .iter()
            .filter(|(k, s)| matches!(k, crate::buffer_ref::BufferRef::File(_)) && s.dirty)
            .count();
        for k in &clean_keys {
            self.sleeping.remove(k);
            reloaded += 1;
        }

        let mut parts = Vec::new();
        if reloaded > 0 {
            parts.push(format!("reloaded {reloaded}"));
        }
        if unchanged > 0 {
            parts.push(format!("{unchanged} unchanged"));
        }
        if preserved_dirty > 0 {
            parts.push(format!("{preserved_dirty} dirty kept"));
        }
        if parts.is_empty() && errors.is_empty() {
            self.push_toast(Toast::info("no file-backed buffers"));
        } else if errors.is_empty() {
            self.push_toast(Toast::info(parts.join(", ")));
        } else {
            let summary = if parts.is_empty() {
                String::from("reload-all failed")
            } else {
                parts.join(", ")
            };
            self.push_toast(Toast::error(format!(
                "{summary}; errors: {}",
                errors.join("; ")
            )));
        }
    }

    fn run_notify_lsp_save(&mut self) {
        let text = self.active_doc().lines.join("\n");
        if let Err(e) = self.lsp.did_save(&text) {
            self.push_toast(Toast::error(format!("lsp didSave: {}", root_cause(&e))));
        }
    }
}
