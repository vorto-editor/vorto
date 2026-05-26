//! App-side bookmark actions: capture the current location (`<space>ma`)
//! and jump to / remove a mark (driven by the `<space>mm` picker).
//!
//! The list and its persistence live on [`crate::bookmark::BookmarkStore`]
//! (held as `App::bookmarks`); these methods just bridge it to the
//! editor's live cursor/buffer and the toast queue.

use crate::app::{App, Toast, root_cause};
use crate::bookmark::Bookmark;
use crate::buffer_ref::BufferRef;

impl App {
    /// `<space>ma` — bookmark the active buffer at the cursor's row.
    /// De-duplicates by buffer (re-marking a file already in the list is
    /// a no-op), and toasts either way so the keystroke always has
    /// visible feedback.
    pub(super) fn add_current_bookmark(&mut self) {
        let target = self.active_ref();
        let line = self.editor.cursor.row;
        let label = self.bookmark_label(&target);
        if self.bookmarks.add(target, line) {
            self.push_toast(Toast::info(format!("bookmarked {label}:{}", line + 1)));
        } else {
            self.push_toast(Toast::info(format!("already bookmarked {label}")));
        }
    }

    /// `<space>md` — remove the active buffer's bookmark, if any (marks
    /// dedup by buffer, so there's at most one). Toasts either way.
    pub(super) fn remove_current_bookmark(&mut self) {
        let target = self.active_ref();
        let label = self.bookmark_label(&target);
        let existed = self.bookmarks.marks.iter().any(|m| m.target == target);
        self.bookmarks.remove_target(&target);
        if existed {
            self.push_toast(Toast::info(format!("removed bookmark {label}")));
        } else {
            self.push_toast(Toast::info(format!("no bookmark on {label}")));
        }
    }

    /// Jump to `mark`: switch to its buffer, then park the cursor on the
    /// saved row (clamped to the buffer's length). Centers the landing
    /// row the same way the other picker-driven jumps do.
    pub(super) fn goto_bookmark(&mut self, mark: &Bookmark) {
        if let Err(e) = self.switch_to_buffer(mark.target.clone()) {
            self.push_toast(Toast::error(format!("bookmark: {}", root_cause(&e))));
            return;
        }
        let last = self.active_doc().lines.len().saturating_sub(1);
        self.editor.cursor.row = mark.line.min(last);
        self.editor.cursor.col = 0;
        ed_op_ref!(self, clamp_col(false));
        self.run_scroll(crate::effect::ScrollAnchor::Center);
    }

    /// Remove the mark for `target` (the picker's `d`).
    pub(super) fn remove_bookmark(&mut self, target: &BufferRef) {
        self.bookmarks.remove_target(target);
    }

    /// `<space>mm` — open the bookmark picker (a [`FuzzyKind::Bookmarks`]
    /// finder, so it gets fuzzy filtering and a preview pane for free).
    /// Builds the `path:line` display labels parallel to the marks.
    /// Toasts instead of opening an empty picker when there's nothing to
    /// jump to.
    ///
    /// [`FuzzyKind::Bookmarks`]: crate::finder::FuzzyKind::Bookmarks
    pub(super) fn open_bookmark_picker(&mut self) {
        if self.bookmarks.marks.is_empty() {
            self.push_toast(Toast::info("no bookmarks (set one with <space>ma)"));
            return;
        }
        let marks = self.bookmarks.marks.clone();
        let labels = marks
            .iter()
            .map(|m| format!("{}:{}", self.bookmark_label(&m.target), m.line + 1))
            .collect();
        self.prompt.open_bookmarks(labels, marks);
    }

    /// Display label for a bookmark target: the file path relative to
    /// the startup cwd when possible (falling back to the full path),
    /// or the scratch label for unnamed buffers.
    pub(super) fn bookmark_label(&self, target: &BufferRef) -> String {
        match target {
            BufferRef::File(path) => path
                .strip_prefix(&self.startup_cwd)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned(),
            BufferRef::Scratch(id) => BufferRef::scratch_label(*id),
        }
    }
}
