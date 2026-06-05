//! Key dispatch + text-editing helpers for the explorer.
//!
//! Routes each [`KeyEvent`] to the per-mode handler and houses the
//! small readline-ish query editing primitives.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{ExplorerMode, ExplorerState};

impl ExplorerState {
    // ── query input ───────────────────────────────────────────────
    //
    // Mirrors the readline-ish subset used by the fuzzy picker so
    // typing behaviour is consistent across both prompts.

    fn char_len(&self) -> usize {
        self.query.chars().count()
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len())
    }

    fn insert(&mut self, c: char) {
        let byte = self.byte_idx(self.cursor);
        self.query.insert(byte, c);
        self.cursor += 1;
        self.refilter();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_idx(self.cursor);
        let start = self.byte_idx(self.cursor - 1);
        self.query.replace_range(start..end, "");
        self.cursor -= 1;
        self.refilter();
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_idx(self.cursor);
        let end = self.byte_idx(self.cursor + 1);
        self.query.replace_range(start..end, "");
        self.refilter();
    }

    pub fn apply_key(&mut self, key: KeyEvent) {
        match self.mode {
            ExplorerMode::Selection => self.apply_selection_key(key),
            ExplorerMode::Filter => self.apply_filter_key(key),
            ExplorerMode::PendingCreate
            | ExplorerMode::PendingRename
            | ExplorerMode::PendingMove => self.apply_action_input_key(key),
            ExplorerMode::PendingDelete => self.apply_delete_key(key),
        }
    }

    /// Selection mode — single-key navigation and op triggers. The
    /// arrow / Ctrl-N/P bindings remain too so users who reach for them
    /// out of habit still get the expected motion.
    fn apply_selection_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.collapse_or_parent(),
            KeyCode::Right | KeyCode::Char('l') if !ctrl => self.expand_or_descend(),
            KeyCode::Up | KeyCode::Char('k') if !ctrl => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') if !ctrl => self.move_down(),
            KeyCode::Char('p') if ctrl => self.move_up(),
            KeyCode::Char('n') if ctrl => self.move_down(),
            KeyCode::Char('.') => self.toggle_hidden(),
            KeyCode::Char('h') if !ctrl => self.toggle_vcs(),
            KeyCode::Char('/') => self.enter_filter_mode(),
            KeyCode::Char('a') => self.enter_create_mode(),
            KeyCode::Char('d') => self.enter_delete_mode(),
            KeyCode::Char('r') => self.enter_rename_mode(),
            KeyCode::Char('m') => self.enter_move_mode(),
            _ => {}
        }
    }

    /// Filter mode — the legacy readline-style query input. Esc returns
    /// to selection mode (handled by the prompt controller, not here).
    fn apply_filter_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right if self.cursor < self.char_len() => self.cursor += 1,
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.char_len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Char('p') if ctrl => self.move_up(),
            KeyCode::Char('n') if ctrl => self.move_down(),
            KeyCode::Char('b') if ctrl => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl && self.cursor < self.char_len() => self.cursor += 1,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.char_len(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
    }

    /// Shared key handler for the create/rename/move input prompts.
    /// Enter and Esc are intercepted upstream (the prompt controller
    /// needs filesystem access to submit, and Esc has to know which
    /// mode to fall back to).
    fn apply_action_input_key(&mut self, key: KeyEvent) {
        // Editing the input invalidates any sticky error from the
        // previous submission attempt — clear it so the user isn't
        // staring at a stale complaint while they fix the path.
        self.error = None;
        let Some(input) = self.action.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => input.cursor = input.cursor.saturating_sub(1),
            KeyCode::Right if input.cursor < input.char_len() => input.cursor += 1,
            KeyCode::Home => input.cursor = 0,
            KeyCode::End => input.cursor = input.char_len(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Char('b') if ctrl => input.cursor = input.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl && input.cursor < input.char_len() => input.cursor += 1,
            KeyCode::Char('a') if ctrl => input.cursor = 0,
            KeyCode::Char('e') if ctrl => input.cursor = input.char_len(),
            KeyCode::Char(c) if !ctrl => input.insert(c),
            _ => {}
        }
    }

    /// Delete confirmation: `y`/`Y` deletes via the controller, anything
    /// else (besides Enter/Esc, which are handled upstream) cancels.
    fn apply_delete_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Mark intent — the controller drains it after each key
                // event and runs the filesystem mutation.
            }
            _ => {
                self.cancel_pending();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_state, paths};
    use super::*;
    use crate::finder::explorer::tree::build_nodes;

    #[test]
    fn selection_mode_swallows_chars_no_query_change() {
        // Confirms `j` / `a` / random text in Selection mode never
        // leaks into the query input — the failure mode the user hit
        // when this initially shipped.
        let nodes = build_nodes(&paths(), &[], false);
        let mut s = make_state(nodes, "");
        assert_eq!(s.mode, ExplorerMode::Selection);
        s.apply_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        s.apply_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(s.query, "");
        assert_eq!(s.mode, ExplorerMode::Selection);
    }

    #[test]
    fn slash_enters_filter_mode() {
        let nodes = build_nodes(&paths(), &[], false);
        let mut s = make_state(nodes, "");
        s.apply_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(s.mode, ExplorerMode::Filter);
        s.apply_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(s.query, "l");
    }
}
