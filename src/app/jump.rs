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
use crate::editor::{Buffer, Cursor, JumpEntry};
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
        self.editor.jumps.push(entry);
    }

    /// `Ctrl-O` — move back through the jump history.
    pub(super) fn jump_back(&mut self, count: u32) {
        let here = JumpEntry {
            doc: self.editor.doc.clone(),
            cursor: self.editor.cursor,
        };
        match self.editor.jumps.backward(count.max(1) as usize, here) {
            Some(target) => self.goto_jump_entry(target),
            None => self.push_toast(Toast::info("at oldest jump")),
        }
    }

    /// `Ctrl-I` / `Tab` — move forward through the jump history.
    pub(super) fn jump_forward(&mut self, count: u32) {
        match self.editor.jumps.forward(count.max(1) as usize) {
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
            self.editor
                .jumps
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
