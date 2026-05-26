//! Undo / redo stack management.

use super::{Buffer, Editor, Snapshot};

const MAX_UNDO_DEPTH: usize = 200;

impl Editor {
    /// Save the current buffer state to the undo stack and clear redo.
    /// Callers should invoke this immediately *before* a mutation so the
    /// stored state represents "what to come back to" on undo.
    pub fn snapshot(&mut self, buf: &mut Buffer) {
        buf.undo_stack.push(Snapshot {
            lines: buf.lines.clone(),
            cursor: self.cursor,
            extra_cursors: self.extra_cursors.clone(),
            dirty: buf.dirty,
        });
        buf.redo_stack.clear();
        if buf.undo_stack.len() > MAX_UNDO_DEPTH {
            buf.undo_stack.remove(0);
        }
    }

    /// Step back one snapshot. Returns false when the undo stack is empty.
    pub fn undo(&mut self, buf: &mut Buffer) -> bool {
        let Some(prev) = buf.undo_stack.pop() else {
            return false;
        };
        buf.redo_stack.push(Snapshot {
            lines: std::mem::replace(&mut buf.lines, prev.lines),
            cursor: std::mem::replace(&mut self.cursor, prev.cursor),
            extra_cursors: std::mem::replace(&mut self.extra_cursors, prev.extra_cursors),
            dirty: std::mem::replace(&mut buf.dirty, prev.dirty),
        });
        buf.version = buf.version.wrapping_add(1);
        true
    }

    /// Reapply the most recently undone snapshot.
    pub fn redo(&mut self, buf: &mut Buffer) -> bool {
        let Some(next) = buf.redo_stack.pop() else {
            return false;
        };
        buf.undo_stack.push(Snapshot {
            lines: std::mem::replace(&mut buf.lines, next.lines),
            cursor: std::mem::replace(&mut self.cursor, next.cursor),
            extra_cursors: std::mem::replace(&mut self.extra_cursors, next.extra_cursors),
            dirty: std::mem::replace(&mut buf.dirty, next.dirty),
        });
        buf.version = buf.version.wrapping_add(1);
        true
    }
}
