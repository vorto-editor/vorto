//! Undo / redo stack management.

use super::{Editor, Snapshot};

const MAX_UNDO_DEPTH: usize = 200;

impl Editor {
    /// Save the current buffer state to the undo stack and clear redo.
    /// Callers should invoke this immediately *before* a mutation so the
    /// stored state represents "what to come back to" on undo.
    pub fn snapshot(&mut self) {
        self.buffer.undo_stack.push(Snapshot {
            lines: self.buffer.lines.clone(),
            cursor: self.cursor,
            extra_cursors: self.extra_cursors.clone(),
            dirty: self.buffer.dirty,
        });
        self.buffer.redo_stack.clear();
        if self.buffer.undo_stack.len() > MAX_UNDO_DEPTH {
            self.buffer.undo_stack.remove(0);
        }
    }

    /// Step back one snapshot. Returns false when the undo stack is empty.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.buffer.undo_stack.pop() else {
            return false;
        };
        self.buffer.redo_stack.push(Snapshot {
            lines: std::mem::replace(&mut self.buffer.lines, prev.lines),
            cursor: std::mem::replace(&mut self.cursor, prev.cursor),
            extra_cursors: std::mem::replace(&mut self.extra_cursors, prev.extra_cursors),
            dirty: std::mem::replace(&mut self.buffer.dirty, prev.dirty),
        });
        self.buffer.version = self.buffer.version.wrapping_add(1);
        true
    }

    /// Reapply the most recently undone snapshot.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.buffer.redo_stack.pop() else {
            return false;
        };
        self.buffer.undo_stack.push(Snapshot {
            lines: std::mem::replace(&mut self.buffer.lines, next.lines),
            cursor: std::mem::replace(&mut self.cursor, next.cursor),
            extra_cursors: std::mem::replace(&mut self.extra_cursors, next.extra_cursors),
            dirty: std::mem::replace(&mut self.buffer.dirty, next.dirty),
        });
        self.buffer.version = self.buffer.version.wrapping_add(1);
        true
    }
}
