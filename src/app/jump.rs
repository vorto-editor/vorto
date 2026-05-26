//! Two-character label jump (`gw`) — the "easymotion / hop / leap"
//! style overlay.
//!
//! When the user presses `gw`, every word start in the visible viewport
//! gets a 2-character label drawn over its first few cells. The user
//! then types the label to jump:
//!
//! - First keypress filters to labels starting with that char. If only
//!   one matches, the jump fires immediately.
//! - Second keypress disambiguates within that filtered set and jumps.
//! - Esc (or any key that matches no remaining label) cancels.
//!
//! Targets are word starts (vim's `\w` char-class: alphanumeric + `_`).
//! Labels are drawn from an ergonomics-first alphabet (home row first)
//! and assigned by `i % N` for the first char, `i / N` for the second
//! so consecutive targets get distinct first chars — meaning a small
//! number of targets all jump on a single keypress.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer_ref::BufferRef;
use crate::editor::{Buffer, Cursor};
use crate::effect::ScrollAnchor;
use crate::lsp::{Location, Position, Range, path_to_uri};

use super::lsp_apply::format_location_label;
use super::{App, Toast, root_cause};

/// Alphabet used to construct labels. Home row first, then top row,
/// then bottom row — same ergonomics ordering hop/leap converged on.
/// 26 chars, so `26 * 26 = 676` distinct labels — more than fits in any
/// reasonable viewport.
const ALPHABET: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

#[derive(Debug, Clone)]
pub struct JumpLabel {
    pub pos: Cursor,
    pub first: char,
    /// `None` when fewer targets than the alphabet size — a single
    /// keypress is enough to pick the target.
    pub second: Option<char>,
}

#[derive(Debug)]
pub struct JumpState {
    pub labels: Vec<JumpLabel>,
    /// `Some` after the user has typed the first character. The render
    /// path then hides labels whose `first` doesn't match and shows the
    /// remaining ones as just their `second` char.
    pub typed_first: Option<char>,
}

impl App {
    /// Enter jump-label mode. Scans every visible line for word starts
    /// and assigns labels. Cancels (with a status message) when there
    /// is nothing in the viewport to label.
    pub(super) fn start_jump_label(&mut self) {
        let targets = collect_jump_targets(self.active_doc());
        if targets.is_empty() {
            self.push_toast(Toast::info("no jump targets"));
            return;
        }
        let labels = assign_labels(targets);
        self.jump_state = Some(JumpState {
            labels,
            typed_first: None,
        });
        self.push_toast(Toast::info("jump: type label (Esc to cancel)"));
    }

    /// Handle a key while jump-label mode is active. Always consumes
    /// the key (the caller routes here unconditionally when
    /// `self.jump_state` is `Some`). Returns silently — state changes
    /// are mutations to `self.jump_state` / `self.editor.cursor`.
    pub(super) fn handle_jump_key(&mut self, key: KeyEvent) {
        // Esc / Ctrl-C / Ctrl-G — cancel.
        if key.code == KeyCode::Esc
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('g')))
        {
            self.cancel_jump();
            return;
        }
        let KeyCode::Char(ch) = key.code else {
            self.cancel_jump();
            return;
        };

        let Some(state) = self.jump_state.as_mut() else {
            return;
        };

        match state.typed_first {
            None => {
                // First keystroke. Filter labels by `first == ch`.
                let mut matched: Vec<&JumpLabel> =
                    state.labels.iter().filter(|l| l.first == ch).collect();
                if matched.is_empty() {
                    self.cancel_jump();
                    return;
                }
                // If only one (or all share a `None` second), jump now.
                if matched.len() == 1 {
                    let pos = matched.remove(0).pos;
                    self.finish_jump(pos);
                    return;
                }
                state.typed_first = Some(ch);
            }
            Some(first) => {
                let target = state
                    .labels
                    .iter()
                    .find(|l| l.first == first && l.second == Some(ch))
                    .map(|l| l.pos);
                match target {
                    Some(pos) => self.finish_jump(pos),
                    None => self.cancel_jump(),
                }
            }
        }
    }

    fn finish_jump(&mut self, pos: Cursor) {
        // Record where we were before the label jump so `Ctrl-O` can
        // come back. The label overlay doesn't move the cursor, so the
        // live position is still the origin.
        self.record_jump();
        self.editor.cursor = pos;
        self.jump_state = None;
        // The "jump: type label" hint is left to expire on its own —
        // wiping it would also wipe unrelated toasts the user might
        // have queued just before jumping.
    }

    fn cancel_jump(&mut self) {
        self.jump_state = None;
        self.push_toast(Toast::info("jump cancelled"));
    }
}

/// Walk every visible row and emit a `Cursor` at every word start
/// (`\w` char-class: alphanumeric or `_`, preceded by a non-word char
/// or line start). Order is top-to-bottom, left-to-right.
fn collect_jump_targets(buffer: &Buffer) -> Vec<Cursor> {
    let scroll = buffer.scroll.get();
    let height = buffer.viewport_height.get();
    if height == 0 {
        return Vec::new();
    }
    let last = (scroll + height).min(buffer.lines.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for row in scroll..last {
        let mut prev_word = false;
        for (col, c) in buffer.lines[row].chars().enumerate() {
            let cur_word = is_word(c);
            if cur_word && !prev_word {
                out.push(Cursor { row, col });
            }
            prev_word = cur_word;
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────────
// Jump history (vim's jumplist) — `Ctrl-O` / `Ctrl-I` navigation and the
// `:jumps` / `<space>j` picker.
// ────────────────────────────────────────────────────────────────────────

/// Cap on retained jump entries, matching vim's default `'jumplist'`.
const JUMPLIST_CAP: usize = 100;

/// One recorded position: which document and where in it. Cross-buffer
/// because the list is app-global (a buffer switch replaces `App::editor`
/// wholesale, so per-`Editor` state wouldn't survive one).
#[derive(Debug, Clone)]
pub struct JumpEntry {
    pub doc: BufferRef,
    pub cursor: Cursor,
}

/// Browser-style jump history. `current` indexes the position the user
/// is conceptually "at"; it equals `entries.len()` while sitting at the
/// tip (no backward navigation in progress), so the first `Ctrl-O`
/// snapshots the live position to make it reachable again via `Ctrl-I`.
#[derive(Debug, Default)]
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

impl App {
    /// Snapshot the live position into the jump history. A no-op while
    /// navigating the list itself (so `Ctrl-O` / `Ctrl-I` and picker
    /// jumps don't re-record the positions they're moving through).
    pub(super) fn record_jump(&mut self) {
        if self.navigating_jumplist {
            return;
        }
        let entry = JumpEntry {
            doc: self.editor.doc.clone(),
            cursor: self.editor.cursor,
        };
        self.jumps.push(entry);
    }

    /// `Ctrl-O` — move back through the jump history.
    pub(super) fn jump_back(&mut self, count: u32) {
        let here = JumpEntry {
            doc: self.editor.doc.clone(),
            cursor: self.editor.cursor,
        };
        match self.jumps.backward(count.max(1) as usize, here) {
            Some(target) => self.goto_jump_entry(target),
            None => self.push_toast(Toast::info("at oldest jump")),
        }
    }

    /// `Ctrl-I` / `Tab` — move forward through the jump history.
    pub(super) fn jump_forward(&mut self, count: u32) {
        match self.jumps.forward(count.max(1) as usize) {
            Some(target) => self.goto_jump_entry(target),
            None => self.push_toast(Toast::info("at newest jump")),
        }
    }

    /// Land on a recorded jump entry, switching buffers if needed. The
    /// `navigating_jumplist` guard keeps the switch/cursor moves from
    /// being re-recorded as fresh jumps.
    fn goto_jump_entry(&mut self, entry: JumpEntry) {
        self.navigating_jumplist = true;
        let res = self.goto_jump_entry_inner(&entry);
        self.navigating_jumplist = false;
        if let Err(e) = res {
            self.push_toast(Toast::error(format!("jump: {}", root_cause(&e))));
        }
    }

    fn goto_jump_entry_inner(&mut self, entry: &JumpEntry) -> Result<()> {
        if self.editor.doc != entry.doc {
            self.switch_to_buffer(entry.doc.clone())?;
        }
        let last = self.active_doc().lines.len().saturating_sub(1);
        self.editor.cursor.row = entry.cursor.row.min(last);
        self.editor.cursor.col = entry.cursor.col;
        ed_op_ref!(self, clamp_col(false));
        self.run_scroll(ScrollAnchor::Center);
        Ok(())
    }

    /// `:jumps` / `<space>j` — open the fuzzy picker over the jump
    /// history. The list is newest-first and starts with the *current*
    /// position: the jumplist only stores pre-jump origins, so without
    /// this the last landing point (where the cursor sits now) would
    /// have no selectable entry. Only file-backed positions are listed
    /// (the picker jumps via `Location`, which needs a URI);
    /// scratch-buffer positions stay reachable through `Ctrl-O` /
    /// `Ctrl-I`.
    pub(super) fn open_jump_list(&mut self) {
        let here = JumpEntry {
            doc: self.editor.doc.clone(),
            cursor: self.editor.cursor,
        };
        // Current position on top, then origins newest-first. Skip any
        // stored origin that is the same line as the current position so
        // it isn't listed twice.
        let entries = std::iter::once(here.clone()).chain(
            self.jumps
                .entries()
                .iter()
                .rev()
                .filter(|e| !(e.doc == here.doc && e.cursor.row == here.cursor.row))
                .cloned(),
        );

        let mut items: Vec<String> = Vec::new();
        let mut locations: Vec<Location> = Vec::new();
        for entry in entries {
            let BufferRef::File(path) = &entry.doc else {
                continue;
            };
            let line = entry.cursor.row as u32;
            let character = entry.cursor.col as u32;
            let loc = Location {
                uri: path_to_uri(path),
                range: Range {
                    start: Position { line, character },
                    end: Position { line, character },
                },
            };
            items.push(format_location_label(&loc, &self.startup_cwd));
            locations.push(loc);
        }
        if items.is_empty() {
            // The list may be non-empty yet hold only scratch-buffer
            // positions, which the picker can't show (no URI to jump to).
            self.push_toast(Toast::info("no file-backed jumps to show"));
            return;
        }
        self.prompt.open_jumps(items, locations);
    }
}

/// Assign a label to each target.
///
/// - When there are no more targets than alphabet letters, every label
///   is single-char (`second = None`) and one keystroke jumps.
/// - Beyond that, labels become two-char. First char varies fastest
///   (`i % a`) so consecutive targets get distinct first chars — when
///   the user's intended target is the only one with its first char,
///   the unique-match branch in `handle_jump_key` jumps after a single
///   keystroke even though a two-char label is drawn.
///
/// Targets past `a * a` aren't labelled — the viewport would need to
/// be > 676 word starts before that mattered.
fn assign_labels(targets: Vec<Cursor>) -> Vec<JumpLabel> {
    let a = ALPHABET.len();
    let n = targets.len();
    let max = a * a;
    targets
        .into_iter()
        .take(max)
        .enumerate()
        .map(|(i, pos)| {
            let (first, second) = if n <= a {
                (ALPHABET[i], None)
            } else {
                (ALPHABET[i % a], Some(ALPHABET[i / a]))
            };
            JumpLabel { pos, first, second }
        })
        .collect()
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
