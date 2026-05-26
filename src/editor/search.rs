use crate::editor::{Buffer, Cursor, Editor};

#[derive(Debug, Default)]
pub struct SearchState {
    pub query: String,
    pub last_forward: bool,
}

impl SearchState {
    pub fn set(&mut self, query: String, forward: bool) {
        self.query = query;
        self.last_forward = forward;
    }

    pub fn find_next(&self, editor: &Editor, buf: &Buffer, forward: bool) -> Option<Cursor> {
        if self.query.is_empty() {
            return None;
        }
        if forward {
            find_forward(editor, buf, &self.query)
        } else {
            find_backward(editor, buf, &self.query)
        }
    }

    /// Find the next/previous match's full range relative to the cursor.
    /// Returns `(start, end_inclusive)` — the cursor positions of the
    /// first and last char of the match. Used by `gn` / `gN` to select
    /// the match in Visual mode.
    ///
    /// Unlike `find_next`, the forward variant searches from the cursor
    /// position (inclusive), so a cursor sitting at the start of a match
    /// selects that match instead of skipping to the next one.
    pub fn find_match_range(
        &self,
        editor: &Editor,
        buf: &Buffer,
        forward: bool,
    ) -> Option<(Cursor, Cursor)> {
        if self.query.is_empty() {
            return None;
        }
        let start = if forward {
            find_forward_inclusive(editor, buf, &self.query)
        } else {
            find_backward_inclusive(editor, buf, &self.query)
        }?;
        let end = match_end_inclusive(buf, &self.query, start)?;
        Some((start, end))
    }
}

fn find_forward(editor: &Editor, buf: &Buffer, query: &str) -> Option<Cursor> {
    let start_row = editor.cursor.row;
    let start_col = editor.cursor.col + 1;

    for (offset, _) in buf
        .lines
        .iter()
        .enumerate()
        .cycle()
        .take(buf.lines.len() + 1)
    {
        let row = (start_row + offset) % buf.lines.len();
        let line = &buf.lines[row];
        let search_from_byte = if offset == 0 {
            char_to_byte(line, start_col)
        } else {
            0
        };
        if search_from_byte > line.len() {
            continue;
        }
        if let Some(byte_idx) = line[search_from_byte..].find(query) {
            let abs_byte = search_from_byte + byte_idx;
            let col = byte_to_char(line, abs_byte);
            return Some(Cursor { row, col });
        }
    }
    None
}

fn find_backward(editor: &Editor, buf: &Buffer, query: &str) -> Option<Cursor> {
    let n = buf.lines.len();
    let start_row = editor.cursor.row;
    let start_col = editor.cursor.col;

    for offset in 0..=n {
        let row = (start_row + n - offset) % n;
        let line = &buf.lines[row];
        let search_until_byte = if offset == 0 {
            char_to_byte(line, start_col)
        } else {
            line.len()
        };
        if let Some(byte_idx) = line[..search_until_byte].rfind(query) {
            let col = byte_to_char(line, byte_idx);
            return Some(Cursor { row, col });
        }
    }
    None
}

fn find_forward_inclusive(editor: &Editor, buf: &Buffer, query: &str) -> Option<Cursor> {
    let start_row = editor.cursor.row;
    let start_col = editor.cursor.col;

    for (offset, _) in buf
        .lines
        .iter()
        .enumerate()
        .cycle()
        .take(buf.lines.len() + 1)
    {
        let row = (start_row + offset) % buf.lines.len();
        let line = &buf.lines[row];
        let search_from_byte = if offset == 0 {
            char_to_byte(line, start_col)
        } else {
            0
        };
        if search_from_byte > line.len() {
            continue;
        }
        if let Some(byte_idx) = line[search_from_byte..].find(query) {
            let abs_byte = search_from_byte + byte_idx;
            let col = byte_to_char(line, abs_byte);
            return Some(Cursor { row, col });
        }
    }
    None
}

fn find_backward_inclusive(editor: &Editor, buf: &Buffer, query: &str) -> Option<Cursor> {
    let n = buf.lines.len();
    let start_row = editor.cursor.row;
    let start_col = editor.cursor.col;
    let query_chars = query.chars().count();

    for offset in 0..=n {
        let row = (start_row + n - offset) % n;
        let line = &buf.lines[row];
        // On the cursor row, extend the search window forward by the
        // query length so a match starting at or before the cursor is
        // still inside the slice `rfind` scans.
        let search_until_byte = if offset == 0 {
            char_to_byte(line, start_col + query_chars).min(line.len())
        } else {
            line.len()
        };
        if let Some(byte_idx) = line[..search_until_byte].rfind(query) {
            let col = byte_to_char(line, byte_idx);
            return Some(Cursor { row, col });
        }
    }
    None
}

/// Given a match start, compute the cursor of the last char of the
/// match. Assumes `query` matches at `start` on a single line.
fn match_end_inclusive(buf: &Buffer, query: &str, start: Cursor) -> Option<Cursor> {
    let line = buf.lines.get(start.row)?;
    let len = query.chars().count();
    if len == 0 {
        return None;
    }
    let line_chars = line.chars().count();
    let end_col = start.col + len - 1;
    let end_col = end_col.min(line_chars.saturating_sub(1));
    Some(Cursor {
        row: start.row,
        col: end_col,
    })
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn byte_to_char(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx].chars().count()
}
