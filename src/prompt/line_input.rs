//! Single-line text input and the readline-ish key handling shared by
//! the `:`, `/`, rename, and fuzzy-query prompts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Single-line text input with a movable insertion point. `cursor` is a
/// char index in `[0, char_count]`; methods keep it in that range and
/// operate at char boundaries so multi-byte input behaves correctly.
#[derive(Default)]
pub struct LineInput {
    buf: String,
    cursor: usize,
}

impl LineInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    /// Cell-column the insertion point sits at — i.e. the total
    /// terminal-cell width of the text before the cursor. Status-bar
    /// placement needs this so the on-screen caret follows fullwidth
    /// characters correctly (CJK glyphs take two cells, not one).
    pub fn cursor_cell_col(&self) -> usize {
        let cut_byte = self.byte_idx(self.cursor);
        crate::text_width::str_cell_width(&self.buf[..cut_byte])
    }

    fn char_len(&self) -> usize {
        self.buf.chars().count()
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.buf
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.buf.len())
    }

    pub fn insert(&mut self, c: char) {
        let byte = self.byte_idx(self.cursor);
        self.buf.insert(byte, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_idx(self.cursor);
        let start = self.byte_idx(self.cursor - 1);
        self.buf.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_idx(self.cursor);
        let end = self.byte_idx(self.cursor + 1);
        self.buf.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.char_len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.char_len();
    }

    pub fn into_string(self) -> String {
        self.buf
    }
}

/// Apply a single key event to a [`LineInput`]. Handles the standard
/// readline-ish bindings the user already expects in `:`, `/`, rename,
/// and the fuzzy picker query (left/right, home/end, Ctrl-A/E/B/F,
/// backspace/delete, plain char insertion).
pub(crate) fn apply_line_key(input: &mut LineInput, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Left => input.left(),
        KeyCode::Right => input.right(),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Char('b') if ctrl => input.left(),
        KeyCode::Char('f') if ctrl => input.right(),
        KeyCode::Char('a') if ctrl => input.home(),
        KeyCode::Char('e') if ctrl => input.end(),
        KeyCode::Char(c) if !ctrl => input.insert(c),
        _ => {}
    }
}
