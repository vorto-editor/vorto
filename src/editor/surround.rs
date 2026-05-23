//! Buffer primitives for vim-surround: wrap a range with a pair, strip
//! the boundary chars of an existing pair, replace them with another.
//!
//! All three operate on a half-open `[lo, hi)` range and assume `hi`
//! points just past the closing delimiter (the convention used by
//! `text_object_range(Around, _)`).

use super::ops::shift_cursor_for_edit;
use super::{Buffer, Cursor, char_to_byte};

impl Buffer {
    /// Wrap the half-open range `[from, to)` with `open` / `close`.
    /// The close is inserted first so the open's coordinates stay
    /// valid; cursors on the same row shift to preserve their relative
    /// positions.
    pub fn surround_wrap(&mut self, open: &str, close: &str, from: Cursor, to: Cursor) {
        let (lo, hi) = order(from, to);
        if lo == hi {
            return;
        }
        let open_chars = open.chars().count();
        let close_chars = close.chars().count();

        if hi.row < self.lines.len() {
            let hi_byte = char_to_byte(&self.lines[hi.row], hi.col);
            self.lines[hi.row].insert_str(hi_byte, close);
            self.for_each_cursor(|c| {
                shift_cursor_for_edit(c, hi.row, hi.col, 0, close_chars)
            });
        }
        let lo_byte = char_to_byte(&self.lines[lo.row], lo.col);
        self.lines[lo.row].insert_str(lo_byte, open);
        self.for_each_cursor(|c| {
            shift_cursor_for_edit(c, lo.row, lo.col, 0, open_chars)
        });

        self.clamp_col(false);
        self.touch();
    }

    /// Remove a single character at `(row, col)` and `(row, col+1)`.
    /// Used by [`Self::surround_strip`] / [`Self::surround_replace`] to
    /// peel boundary delimiters from a `(text_object Around)` range.
    /// `lo` is the open-delimiter position; `hi` is one past the close
    /// (half-open). Open and close may live on different rows for
    /// multi-line pairs like `( …\n… )`.
    pub fn surround_strip(&mut self, lo: Cursor, hi: Cursor) {
        let (lo, hi) = order(lo, hi);
        if lo == hi || hi.col == 0 && hi.row == lo.row {
            return;
        }
        // Close character sits at hi.col - 1 on hi.row. We mutate that
        // row first because it's the higher position; the open at lo
        // stays at its original coordinates.
        let close_col = if hi.col == 0 {
            // Pathological — shouldn't happen for a well-formed pair —
            // bail rather than panic.
            return;
        } else {
            hi.col - 1
        };
        delete_one_char(self, hi.row, close_col);
        delete_one_char(self, lo.row, lo.col);
        self.clamp_col(false);
        self.touch();
    }

    /// Replace the boundary delimiters of `[lo, hi)` with `new_open` /
    /// `new_close`. Inner content is preserved verbatim — no attempt to
    /// trim / add the asymmetric-bracket space convention beyond what
    /// the caller bakes into `new_open` / `new_close`.
    pub fn surround_replace(
        &mut self,
        lo: Cursor,
        hi: Cursor,
        new_open: &str,
        new_close: &str,
    ) {
        let (lo, hi) = order(lo, hi);
        if lo == hi {
            return;
        }
        let close_col = if hi.col == 0 { return } else { hi.col - 1 };
        replace_one_char(self, hi.row, close_col, new_close);
        replace_one_char(self, lo.row, lo.col, new_open);
        self.clamp_col(false);
        self.touch();
    }
}

fn delete_one_char(buf: &mut Buffer, row: usize, col: usize) {
    let line = &mut buf.lines[row];
    let nchars = line.chars().count();
    if col >= nchars {
        return;
    }
    let start = char_to_byte(line, col);
    let end = char_to_byte(line, col + 1);
    line.replace_range(start..end, "");
    buf.for_each_cursor(|c| shift_cursor_for_edit(c, row, col, 1, 0));
}

fn replace_one_char(buf: &mut Buffer, row: usize, col: usize, with: &str) {
    let line = &mut buf.lines[row];
    let nchars = line.chars().count();
    if col >= nchars {
        return;
    }
    let start = char_to_byte(line, col);
    let end = char_to_byte(line, col + 1);
    line.replace_range(start..end, with);
    let inserted = with.chars().count();
    buf.for_each_cursor(|c| shift_cursor_for_edit(c, row, col, 1, inserted));
}

fn order(a: Cursor, b: Cursor) -> (Cursor, Cursor) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_of(lines: &[&str]) -> Buffer {
        let mut b = Buffer::new();
        b.lines = lines.iter().map(|s| s.to_string()).collect();
        b.cursor = Cursor { row: 0, col: 0 };
        b
    }

    #[test]
    fn wrap_inline_with_quotes() {
        let mut b = buf_of(&["foo bar"]);
        b.surround_wrap("\"", "\"", Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 3 });
        assert_eq!(b.lines[0], "\"foo\" bar");
    }

    #[test]
    fn wrap_accepts_multi_char_delimiters() {
        // The primitive doesn't care about the delimiter shape — a
        // future block-comment-style caller could feed it `/* … */`.
        let mut b = buf_of(&["foo bar"]);
        b.surround_wrap("/*", "*/", Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 3 });
        assert_eq!(b.lines[0], "/*foo*/ bar");
    }

    #[test]
    fn strip_quotes() {
        let mut b = buf_of(&["\"foo\""]);
        b.surround_strip(Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 5 });
        assert_eq!(b.lines[0], "foo");
    }

    #[test]
    fn replace_quotes_with_parens_tight() {
        let mut b = buf_of(&["\"foo\""]);
        b.surround_replace(
            Cursor { row: 0, col: 0 },
            Cursor { row: 0, col: 5 },
            "(",
            ")",
        );
        assert_eq!(b.lines[0], "(foo)");
    }

    #[test]
    fn replace_accepts_multi_char_delimiters() {
        // Same as wrap: surround_replace is just a boundary swap and
        // shouldn't care about delimiter length.
        let mut b = buf_of(&["\"foo\""]);
        b.surround_replace(
            Cursor { row: 0, col: 0 },
            Cursor { row: 0, col: 5 },
            "/*",
            "*/",
        );
        assert_eq!(b.lines[0], "/*foo*/");
    }

    #[test]
    fn wrap_multi_row_keeps_content() {
        let mut b = buf_of(&["foo", "bar"]);
        b.surround_wrap("/*", "*/", Cursor { row: 0, col: 0 }, Cursor { row: 1, col: 3 });
        assert_eq!(b.lines[0], "/*foo");
        assert_eq!(b.lines[1], "bar*/");
    }

    #[test]
    fn cursor_shifts_after_wrap() {
        let mut b = buf_of(&["foo bar"]);
        b.cursor = Cursor { row: 0, col: 1 }; // on the 'o'
        b.surround_wrap("\"", "\"", Cursor { row: 0, col: 0 }, Cursor { row: 0, col: 3 });
        // After inserting `"` at col 0, cursor on col 1 (the 'o' at original col 1)
        // should land at col 2 since the open `"` shifted us right by one.
        assert_eq!(b.cursor, Cursor { row: 0, col: 2 });
    }
}
