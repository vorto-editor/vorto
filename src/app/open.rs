//! File-open orchestration: load the buffer synchronously and stash
//! the previous one. The expensive follow-up work (tree-sitter
//! highlighter build, LSP server spawn) is fanned out via
//! [`super::workers`]; multi-buffer cycling and deletion live in
//! [`super::buffer_list`].

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::editor::Buffer;

use crate::buffer_ref::BufferRef;

use super::{App, SleepingBuffer, Toast};

impl App {
    /// Dispatch a buffer-picker selection. Scratch and File both go
    /// through the same stash-and-restore flow as
    /// [`Self::open_path`] — if the target has a sleeping snapshot
    /// (with preserved unsaved edits, cursor, undo history), we
    /// restore it; otherwise we fall through to a fresh load.
    pub fn switch_to_buffer(&mut self, r: BufferRef) -> Result<()> {
        match r {
            BufferRef::Scratch(id) => {
                if self.active_doc().path.is_none() && self.current_scratch_id == Some(id) {
                    return Ok(());
                }
                // Switching to a scratch buffer is a jump (the File arm
                // records via `open_path`; this arm has no such path).
                self.record_jump();
                self.lsp.detach_current();
                let key = BufferRef::Scratch(id);
                // Ensure the target document is in the pool — another
                // pane may already show it live; otherwise thaw from
                // `sleeping`, or mint a fresh empty buffer.
                // `Buffer::new` (one empty line) ≠ `Buffer::default`
                // (zero lines), so we can't use `unwrap_or_default`
                // here — the wrong default would leave the buffer
                // with an empty `lines` Vec and crash motions.
                self.ensure_doc_pooled(&key, Buffer::new);
                self.stash_and_install(key.clone());
                self.current_scratch_id = Some(id);
                self.open_gen = self.open_gen.wrapping_add(1);
                self.lsp.set_last_synced_version(self.active_doc().version);
                self.record_opened(key);
                self.push_toast(Toast::info(BufferRef::scratch_label(id)));
                Ok(())
            }
            BufferRef::File(path) => {
                // Already on this file? Leave cursor/unsaved state alone.
                let current = self
                    .active_doc()
                    .path
                    .as_ref()
                    .and_then(|p| p.canonicalize().ok());
                if current.as_ref() == Some(&path) {
                    return Ok(());
                }
                self.open_path(&path)
            }
        }
    }

    /// Ensure the document named by `key` lives in the pool. If it's
    /// already present (e.g. another pane shows it live) nothing
    /// happens; if it's sleeping it's thawed back; otherwise `build`
    /// mints a fresh document. The document's session (cursor/mode) is
    /// NOT part of the pool — callers wrap it in a fresh [`Editor`].
    pub(super) fn ensure_doc_pooled(&mut self, key: &BufferRef, build: impl FnOnce() -> Buffer) {
        if self.documents.contains_key(key) {
            return;
        }
        let doc = match self.sleeping.remove(key) {
            Some(s) => s.thaw(),
            None => build(),
        };
        self.documents.insert(key.clone(), doc);
    }

    /// Switch the active session to the document named by `next` (which
    /// must already be present in the pool), then retire the document the
    /// session was previously editing if nothing else references it.
    ///
    /// The session itself persists across the switch — only its `doc` is
    /// re-pointed (see [`Self::swap_active_doc`]) — so its jumplist and
    /// per-buffer cursor memory survive, matching vim's per-window
    /// jumplist. The outgoing document goes to `sleeping` (compressed;
    /// highlighter dropped, rebuilt on restore) when no remaining session
    /// — the active one or any inactive pane — names its ref. The version
    /// counter is preserved so LSP `didChange` sequencing re-anchors
    /// cleanly when the document wakes up again. When some pane still
    /// shows it, it stays live in the pool untouched.
    pub(super) fn stash_and_install(&mut self, next: BufferRef) {
        let prev_ref = self.editor.doc.clone();
        self.swap_active_doc(next);
        self.retire_doc_if_unreferenced(prev_ref);
    }

    /// Re-point the active session at `next` (which must already be
    /// pooled) without touching document retirement. Stashes the
    /// outgoing buffer's cursor into this session's `cursor_memory`, then
    /// restores `next`'s remembered cursor — the origin on a first visit
    /// — clamped to the incoming document. Multi-cursors and pending
    /// command tokens are dropped and the mode reset to Normal, matching
    /// a fresh view; the jumplist and cursor memory ride along on the
    /// persistent session.
    fn swap_active_doc(&mut self, next: BufferRef) {
        let prev = self.editor.doc.clone();
        if prev != next {
            self.editor.cursor_memory.insert(prev, self.editor.cursor);
        }
        let doc = self
            .documents
            .get(&next)
            .expect("target doc present in pool");
        self.editor.adopt_doc(next, doc);
    }

    /// Move `r`'s document to `sleeping` when no live session (the
    /// active editor or any inactive pane) references it. A no-op when
    /// the document is still shown somewhere — it stays live in the
    /// pool so that pane keeps rendering.
    pub(super) fn retire_doc_if_unreferenced(&mut self, r: BufferRef) {
        if self.editor.doc == r || self.ref_used_by_inactive_pane(&r) {
            return;
        }
        if let Some(mut doc) = self.documents.remove(&r) {
            doc.highlighter = None;
            self.sleeping.insert(r, SleepingBuffer::freeze(doc));
        }
    }

    /// Install the document named by `successor` (already pooled) as the
    /// active session after a `:bd`, where the deleted buffer is supposed
    /// to vanish entirely. Callers must have already removed the outgoing
    /// document from the pool and cleaned up any MRU / sleeping entries
    /// that refer to it. Unlike [`Self::stash_and_install`] the outgoing
    /// document is NOT retired to `sleeping` — it's gone for good.
    pub(super) fn install_buffer(&mut self, successor: BufferRef) {
        let deleted = self.editor.doc.clone();
        self.swap_active_doc(successor.clone());
        // The deleted document is gone for good — purge its remembered
        // cursor from every session so a later buffer reusing the ref
        // can't inherit a stale position. (`swap_active_doc` just stashed
        // it; undo that here.)
        self.forget_cursor_memory(&deleted);
        // Any inactive pane still showing the deleted buffer (e.g. after
        // `:split`) would be left with a dangling `doc` ref and panic on
        // its next render/focus, so move those sessions onto the
        // successor too — matching vim, where a window showing a
        // bdeleted buffer switches to the replacement. Route them through
        // the same `adopt_doc` normalization as the active session so an
        // inactive pane doesn't keep stale `mode`/`tokens` (it gets no
        // input until refocused, and `focus_pane` doesn't normalize) and
        // restores its own remembered cursor for the successor instead of
        // snapping to the origin.
        if deleted != successor {
            let doc = self
                .documents
                .get(&successor)
                .expect("successor doc present in pool");
            for content in self.pane_content.values_mut() {
                if let crate::app::PaneContent::Editor(ed) = content
                    && ed.doc == deleted
                {
                    ed.adopt_doc(successor.clone(), doc);
                }
            }
        }
    }

    /// Drop `r`'s remembered cursor from every editor session (the active
    /// one and each inactive pane), so a `:bd`-deleted buffer leaves no
    /// stale position behind.
    fn forget_cursor_memory(&mut self, r: &BufferRef) {
        self.editor.cursor_memory.remove(r);
        for content in self.pane_content.values_mut() {
            if let crate::app::PaneContent::Editor(ed) = content {
                ed.cursor_memory.remove(r);
            }
        }
    }

    /// [`BufferRef`] for the currently-active document — simply the
    /// active session's `doc`, the authoritative pool key.
    pub(super) fn active_ref(&self) -> BufferRef {
        self.editor.doc.clone()
    }

    /// Open `path`. Lookup order:
    ///
    /// 1. **Already pooled** (`App.documents`): another pane already
    ///    shows this document. The active session re-points at the same
    ///    live ref, sharing it.
    /// 2. **Sleeping**: the user previously visited and switched
    ///    away. Wake the compressed snapshot into the pool.
    /// 3. **Fresh disk read** as fallback.
    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        // Leaving the current buffer is a jump — record the origin so
        // `Ctrl-O` returns here. No-op while navigating the jumplist.
        self.record_jump();
        let path = self.absolutize(path);
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        let key = BufferRef::File(canon);
        if self.documents.contains_key(&key) {
            // Another pane already shows this document live — share it.
            self.lsp.detach_current();
            self.stash_and_install(key.clone());
            self.current_scratch_id = None;
            self.record_opened(key);
            self.lsp.set_last_synced_version(self.active_doc().version);
            self.push_toast(Toast::info(format!("opened {} (shared)", path.display())));
            // Buffer was live — highlighter survives; LSP resync only.
            self.spawn_lsp_worker(&path);
            return Ok(());
        }
        if self.sleeping.contains_key(&key) {
            self.lsp.detach_current();
            self.ensure_doc_pooled(&key, Buffer::new);
            self.stash_and_install(key.clone());
            self.current_scratch_id = None;
            self.record_opened(key);
            self.open_gen = self.open_gen.wrapping_add(1);
            self.lsp.set_last_synced_version(self.active_doc().version);
            self.push_toast(Toast::info(format!("restored {}", path.display())));
            self.spawn_engine_worker(&path);
            self.spawn_vcs_worker();
            self.spawn_lsp_worker(&path);
            return Ok(());
        }
        self.open_path_force(&path)
    }

    /// Resolve a user-supplied path to an absolute path against
    /// `startup_cwd`. Doesn't touch the filesystem — works for files
    /// that don't exist yet, which `canonicalize()` rejects. Critical
    /// for `:e new_file.rs`: without absolutizing, the relative path
    /// flows into [`crate::lsp::path_to_uri`] which produces a broken
    /// `file:///new_file.rs` URI (no directory), and the LSP server
    /// silently ignores the document.
    fn absolutize(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.startup_cwd.join(path)
        }
    }

    /// Open `path` from disk, discarding any sleeping copy. Used on
    /// the initial command-line load and as the fall-through for
    /// `open_path` when there's no sleeping snapshot to restore.
    pub fn open_path_force(&mut self, path: &Path) -> Result<()> {
        // Load up front — if this fails we want to leave the active
        // buffer alone. Missing files are treated as a new, unsaved
        // buffer attached to `path` so `:w` materializes the file.
        let (loaded, is_new) = match Buffer::load(path) {
            Ok(b) => (b, false),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                let mut b = Buffer::new();
                b.path = Some(path.to_path_buf());
                (b, true)
            }
            Err(e) => return Err(e),
        };
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let key = BufferRef::File(canon.clone());
        // Tell the previous LSP client we're done with that document so
        // it can drop diagnostics and stop watching it.
        self.lsp.detach_current();
        // Re-loading a path drops any previously-sleeping copy of it
        // — the user explicitly asked for the disk version.
        self.sleeping.remove(&key);
        // Pool the freshly-loaded document under its ref, then point a
        // new active session at it.
        self.documents.insert(key.clone(), loaded);
        self.stash_and_install(key.clone());
        self.current_scratch_id = None;
        self.record_opened(key);
        // Bump the generation: any in-flight worker thread from a
        // previous `open_path` is now stale. Its result will be dropped
        // when it lands instead of clobbering this buffer.
        self.open_gen = self.open_gen.wrapping_add(1);
        // Pre-seed the LSP sync version so the first `didChange` after
        // open is a no-op when nothing has changed since load.
        self.lsp.set_last_synced_version(self.active_doc().version);
        self.push_toast(if is_new {
            Toast::info(format!("{} [new file]", path.display()))
        } else {
            Toast::info(format!("opened {}", path.display()))
        });
        // If the fuzzy preview worker already built a highlighter for
        // this path, steal it: we're about to render the buffer and the
        // tree is ready right now. Saves a worker round-trip and the
        // "plain text → highlighted" flash. Re-`refresh` against the
        // buffer's source/version so the cached tree's incremental diff
        // re-anchors on whatever `Buffer::load` just read (usually a
        // no-op because the file hasn't changed since the preview ran).
        let preview_entry = self.preview_lru.borrow_mut().take(path);
        if let Some(entry) = preview_entry {
            let doc = self.active_doc_mut();
            doc.highlighter = None;
            let mut h = entry.highlighter;
            let source = doc.lines.join("\n");
            h.refresh(&source, doc.version);
            doc.highlighter = Some(h);
        } else {
            self.spawn_engine_worker(path);
        }
        self.spawn_vcs_worker();
        self.spawn_lsp_worker(path);
        Ok(())
    }

    /// Re-point every buffer that referenced `old` (or anything under
    /// it, for a directory move) at `new`. Used after the explorer's
    /// rename/move so an open buffer's next save lands at the moved
    /// file's new location instead of resurrecting the source.
    ///
    /// Touches: every pooled document's path and its hashmap key, the
    /// active session's `doc` ref and every inactive pane session's
    /// `doc` ref, every sleeping buffer's stored path and hashmap key,
    /// and the MRU list. The order matters only in that we collect
    /// remap targets first and apply them in a second pass — mutating a
    /// hashmap while iterating it is otherwise rejected by the borrow
    /// checker.
    pub fn rewrite_buffer_paths(&mut self, old: &Path, new: &Path) {
        // Pooled documents — rekey + update each document's own `path`.
        let doc_remap: Vec<(BufferRef, PathBuf)> = self
            .documents
            .keys()
            .filter_map(|k| match k {
                BufferRef::File(p) => remap_path(p, old, new).map(|np| (k.clone(), np)),
                _ => None,
            })
            .collect();
        for (old_ref, new_path) in doc_remap {
            let new_ref = BufferRef::File(new_path.clone());
            if let Some(mut doc) = self.documents.remove(&old_ref) {
                doc.path = Some(new_path);
                self.documents.insert(new_ref.clone(), doc);
            }
            // Re-point any session naming the old ref at the new one.
            if self.editor.doc == old_ref {
                self.editor.doc = new_ref.clone();
            }
            for content in self.pane_content.values_mut() {
                if let crate::app::PaneContent::Editor(ed) = content
                    && ed.doc == old_ref
                {
                    ed.doc = new_ref.clone();
                }
            }
        }

        // Sleeping buffers — the path lives
        // behind setter/getter so we can keep the freeze-compressed
        // payload untouched.
        let sleep_remap: Vec<(BufferRef, PathBuf)> = self
            .sleeping
            .iter()
            .filter_map(|(k, s)| match (k, s.path()) {
                (BufferRef::File(kp), Some(_)) => {
                    remap_path(kp, old, new).map(|np| (k.clone(), np))
                }
                _ => None,
            })
            .collect();
        for (old_ref, new_path) in sleep_remap {
            if let Some(mut sleep) = self.sleeping.remove(&old_ref) {
                sleep.set_path(Some(new_path.clone()));
                self.sleeping.insert(BufferRef::File(new_path), sleep);
            }
        }

        // MRU list — keep order, just rewrite the path inside each ref.
        for r in &mut self.opened_paths {
            if let BufferRef::File(p) = r
                && let Some(np) = remap_path(p, old, new)
            {
                *r = BufferRef::File(np);
            }
        }
    }
}

/// Remap a single path against an `old → new` move. Returns `Some(new
/// path)` when `p` is either exactly `old` (a single-file rename) or
/// lies under `old/` (caught by a directory move). Returns `None` when
/// the path is unaffected so the caller can skip it cheaply.
fn remap_path(p: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    if p == old {
        return Some(new.to_path_buf());
    }
    p.strip_prefix(old).ok().map(|suffix| new.join(suffix))
}
