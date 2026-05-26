//! Jump history (vim's jumplist) data structure — the list backing
//! `Ctrl-O` / `Ctrl-I` navigation and the `:jumps` / `<space>j` picker.
//!
//! Lives on [`super::Editor`] (per-pane session), so each pane keeps its
//! own history. Entries still carry a [`BufferRef`], because within one
//! pane the user roams across several buffers and `Ctrl-O` must be able
//! to step back into a position in a buffer it has since switched away
//! from — the document swap underneath the persistent session leaves the
//! list intact. The App-side navigation methods (`record_jump`,
//! `jump_back`, …) live in `crate::app::jump`.

use crate::buffer_ref::BufferRef;
use crate::editor::Cursor;

/// Cap on retained jump entries, matching vim's default `'jumplist'`.
const JUMPLIST_CAP: usize = 100;

/// One recorded position: which document and where in it.
#[derive(Debug, Clone)]
pub struct JumpEntry {
    pub doc: BufferRef,
    pub cursor: Cursor,
}

/// Browser-style jump history. `current` indexes the position the user
/// is conceptually "at"; it equals `entries.len()` while sitting at the
/// tip (no backward navigation in progress), so the first `Ctrl-O`
/// snapshots the live position to make it reachable again via `Ctrl-I`.
#[derive(Debug, Default, Clone)]
pub struct JumpList {
    entries: Vec<JumpEntry>,
    current: usize,
}

impl JumpList {
    /// Record `from` as a jump origin. Drops any forward history (a new
    /// jump after going back discards the "redo" tail, like a browser),
    /// collapses consecutive jumps that leave the same line, and caps
    /// the list at [`JUMPLIST_CAP`].
    pub fn push(&mut self, from: JumpEntry) {
        self.entries.truncate(self.current);
        if let Some(last) = self.entries.last_mut()
            && last.doc == from.doc
            && last.cursor.row == from.cursor.row
        {
            // Same line as the previous origin — refresh the column
            // rather than growing the list (vim's same-line collapse).
            last.cursor = from.cursor;
            self.current = self.entries.len();
            return;
        }
        self.entries.push(from);
        self.enforce_cap();
        self.current = self.entries.len();
    }

    /// Drop oldest entries past [`JUMPLIST_CAP`]. Returns how many were
    /// removed from the front so callers can fix up `current`.
    fn enforce_cap(&mut self) -> usize {
        if self.entries.len() > JUMPLIST_CAP {
            let overflow = self.entries.len() - JUMPLIST_CAP;
            self.entries.drain(0..overflow);
            overflow
        } else {
            0
        }
    }

    /// Step up to `count` entries back (`Ctrl-O`). A count larger than the
    /// available history clamps to the oldest entry rather than no-opping
    /// (vim's behaviour). `here` is the live position; the first backward
    /// step from the tip snapshots it so a later `Ctrl-I` returns to where
    /// the user started. Returns the entry to land on, or `None` when
    /// already at the oldest entry.
    pub fn backward(&mut self, count: usize, here: JumpEntry) -> Option<JumpEntry> {
        if self.current == 0 {
            return None;
        }
        if self.current == self.entries.len() {
            // Snapshot the live position. It's a distinct return point,
            // not a duplicate origin, so the same-line collapse in
            // `push` deliberately doesn't apply — but the cap does, and
            // dropping from the front shifts `current` left.
            self.entries.push(here);
            self.current -= self.enforce_cap();
        }
        let target = self.current.saturating_sub(count);
        self.current = target;
        self.entries.get(target).cloned()
    }

    /// Step up to `count` entries forward (`Ctrl-I` / `Tab`). A count
    /// larger than the available forward history clamps to the newest
    /// reachable entry. Returns `None` when already at the newest entry.
    pub fn forward(&mut self, count: usize) -> Option<JumpEntry> {
        let last = self.entries.len().checked_sub(1)?;
        if self.current >= last {
            return None;
        }
        let target = (self.current + count).min(last);
        self.current = target;
        self.entries.get(target).cloned()
    }

    /// All recorded entries, oldest first.
    pub fn entries(&self) -> &[JumpEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod jumplist_tests {
    use super::{JumpEntry, JumpList};
    use crate::buffer_ref::BufferRef;
    use crate::editor::Cursor;

    fn entry(row: usize) -> JumpEntry {
        JumpEntry {
            doc: BufferRef::Scratch(0),
            cursor: Cursor { row, col: 0 },
        }
    }

    fn rows(j: &JumpList) -> Vec<usize> {
        j.entries().iter().map(|e| e.cursor.row).collect()
    }

    #[test]
    fn back_then_forward_returns_to_origin() {
        let mut j = JumpList::default();
        j.push(entry(10));
        j.push(entry(20));
        // Standing at row 30; step back twice, then forward back to 30.
        assert_eq!(j.backward(1, entry(30)).unwrap().cursor.row, 20);
        assert_eq!(j.backward(1, entry(30)).unwrap().cursor.row, 10);
        assert_eq!(j.forward(1).unwrap().cursor.row, 20);
        // The live position was snapshotted on the first Ctrl-O, so a
        // final Ctrl-I lands back where we started.
        assert_eq!(j.forward(1).unwrap().cursor.row, 30);
        assert!(j.forward(1).is_none(), "already at newest");
    }

    #[test]
    fn back_past_oldest_returns_none() {
        let mut j = JumpList::default();
        j.push(entry(5));
        assert_eq!(j.backward(1, entry(9)).unwrap().cursor.row, 5);
        assert!(j.backward(1, entry(9)).is_none());
    }

    #[test]
    fn new_jump_after_going_back_truncates_forward_history() {
        let mut j = JumpList::default();
        j.push(entry(10));
        j.push(entry(20));
        j.push(entry(30));
        // Go back to the 10 entry (snapshots live row 40 at the tip).
        j.backward(2, entry(40));
        // A fresh jump discards the 20/30/40 "redo" tail.
        j.push(entry(99));
        assert_eq!(rows(&j), vec![10, 99]);
        assert!(j.forward(1).is_none());
    }

    #[test]
    fn large_count_clamps_to_oldest_and_newest() {
        let mut j = JumpList::default();
        j.push(entry(10));
        j.push(entry(20));
        j.push(entry(30));
        // 10<C-o> with only 3 origins: clamp to the oldest, not no-op.
        assert_eq!(j.backward(10, entry(40)).unwrap().cursor.row, 10);
        // 10<C-i> from the oldest: clamp to the newest reachable (the
        // snapshot taken on the first backward step).
        assert_eq!(j.forward(10).unwrap().cursor.row, 40);
        assert!(j.forward(1).is_none(), "already at newest");
    }

    #[test]
    fn tip_snapshot_respects_cap() {
        let mut j = JumpList::default();
        for r in 0..super::JUMPLIST_CAP {
            j.push(entry(r));
        }
        assert_eq!(j.entries().len(), super::JUMPLIST_CAP);
        // Ctrl-O from the tip snapshots the live position; the list must
        // still honour the cap (drop the oldest) rather than grow to +1.
        let landed = j.backward(1, entry(9999)).unwrap();
        assert_eq!(j.entries().len(), super::JUMPLIST_CAP);
        // Lands on the most-recent origin; the snapshot is reachable
        // again via a forward step.
        assert_eq!(landed.cursor.row, super::JUMPLIST_CAP - 1);
        assert_eq!(j.forward(1).unwrap().cursor.row, 9999);
    }

    #[test]
    fn consecutive_same_line_jumps_collapse() {
        let mut j = JumpList::default();
        j.push(entry(10));
        // Same row again — refreshes in place rather than growing.
        j.push(JumpEntry {
            doc: BufferRef::Scratch(0),
            cursor: Cursor { row: 10, col: 7 },
        });
        assert_eq!(j.entries().len(), 1);
        assert_eq!(j.entries()[0].cursor.col, 7);
    }

    #[test]
    fn caps_at_jumplist_cap() {
        let mut j = JumpList::default();
        for r in 0..super::JUMPLIST_CAP + 50 {
            j.push(entry(r));
        }
        assert_eq!(j.entries().len(), super::JUMPLIST_CAP);
        // Oldest entries dropped; newest retained.
        assert_eq!(
            j.entries().last().unwrap().cursor.row,
            super::JUMPLIST_CAP + 49
        );
    }
}
