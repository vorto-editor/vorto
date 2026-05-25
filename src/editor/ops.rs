//! Range-level and line-level edits, plus the yank register.
//!
//! Buffer mutations that operate on a span (line, char range, column
//! block) and stash the deleted/copied text into `Buffer.yank`. Single-
//! character edits sit in [`super::insert`].

use super::{Buffer, Cursor, char_to_byte};

impl Buffer {
    pub fn delete_line(&mut self) {
        if self.lines.len() == 1 {
            self.yank = self.lines[0].clone();
            self.lines[0].clear();
        } else {
            self.yank = self.lines.remove(self.cursor.row);
            if self.cursor.row >= self.lines.len() {
                self.cursor.row = self.lines.len() - 1;
            }
        }
        self.clamp_col(false);
        self.touch();
    }

    pub fn yank_line(&mut self) {
        self.yank = self.lines[self.cursor.row].clone();
    }

    pub fn paste_after(&mut self) {
        if self.yank.is_empty() {
            return;
        }
        self.lines.insert(self.cursor.row + 1, self.yank.clone());
        self.cursor.row += 1;
        self.cursor.col = 0;
        self.touch();
    }

    /// Remove text between two cursors (inclusive of `from`, exclusive of
    /// `to`). The order of `from`/`to` doesn't matter — they're sorted
    /// internally. After deletion the cursor lands at the lower endpoint.
    pub fn delete_range(&mut self, from: Cursor, to: Cursor) {
        let (from, to) = order(from, to);
        if from == to {
            return;
        }
        if from.row == to.row {
            let line = &mut self.lines[from.row];
            let fb = char_to_byte(line, from.col);
            let tb = char_to_byte(line, to.col);
            line.replace_range(fb..tb, "");
        } else {
            let from_byte = char_to_byte(&self.lines[from.row], from.col);
            let to_byte = char_to_byte(&self.lines[to.row], to.col);
            let head: String = self.lines[from.row][..from_byte].to_string();
            let tail: String = self.lines[to.row][to_byte..].to_string();
            self.lines[from.row] = head + &tail;
            let drain_end = (to.row + 1).min(self.lines.len());
            self.lines.drain((from.row + 1)..drain_end);
        }
        self.cursor = from;
        self.clamp_col(false);
        self.touch();
    }

    /// The text between two cursors `[from, to)` (exclusive `to`), exactly
    /// as [`Self::yank_range`] would capture it. Read-only — callers that
    /// need the text without clobbering the yank register (e.g. building an
    /// agent prompt) use this.
    pub fn range_text(&self, from: Cursor, to: Cursor) -> String {
        let (from, to) = order(from, to);
        if from == to {
            return String::new();
        }
        if from.row == to.row {
            let line = &self.lines[from.row];
            let fb = char_to_byte(line, from.col);
            let tb = char_to_byte(line, to.col);
            line[fb..tb].to_string()
        } else {
            let mut text = String::new();
            let from_byte = char_to_byte(&self.lines[from.row], from.col);
            text.push_str(&self.lines[from.row][from_byte..]);
            text.push('\n');
            for i in (from.row + 1)..to.row {
                text.push_str(&self.lines[i]);
                text.push('\n');
            }
            let to_byte = char_to_byte(&self.lines[to.row], to.col);
            text.push_str(&self.lines[to.row][..to_byte]);
            text
        }
    }

    /// Copy text between two cursors into the yank register.
    pub fn yank_range(&mut self, from: Cursor, to: Cursor) {
        self.yank = self.range_text(from, to);
    }

    /// The text of a run of whole lines (inclusive of both endpoints).
    /// Read-only counterpart to [`Self::yank_lines`].
    pub fn lines_text(&self, from_row: usize, to_row: usize) -> String {
        let (a, b) = (from_row.min(to_row), from_row.max(to_row));
        let b = b.min(self.lines.len().saturating_sub(1));
        self.lines[a..=b].join("\n")
    }

    /// Yank a run of whole lines (inclusive of both endpoints).
    pub fn yank_lines(&mut self, from_row: usize, to_row: usize) {
        self.yank = self.lines_text(from_row, to_row);
    }

    /// Delete a run of whole lines (inclusive). Also stashes them in
    /// the yank register, matching vim's `dd` / visual-line `d`.
    pub fn delete_lines(&mut self, from_row: usize, to_row: usize) {
        let (a, b) = (from_row.min(to_row), from_row.max(to_row));
        let b = b.min(self.lines.len().saturating_sub(1));
        self.yank = self.lines[a..=b].join("\n");
        if a == 0 && b + 1 >= self.lines.len() {
            self.lines.clear();
            self.lines.push(String::new());
            self.cursor.row = 0;
        } else {
            self.lines.drain(a..=b);
            self.cursor.row = a.min(self.lines.len().saturating_sub(1));
        }
        self.cursor.col = 0;
        self.clamp_col(false);
        self.touch();
    }

    /// The text of a column rectangle `[r0..=r1] × [c0..=c1]`, rows joined
    /// by `\n`. Lines shorter than `c1` contribute their truncated slice.
    /// Read-only counterpart to [`Self::yank_block`].
    pub fn block_text(&self, r0: usize, c0: usize, r1: usize, c1: usize) -> String {
        let (r0, r1) = (r0.min(r1), r0.max(r1));
        let (c0, c1) = (c0.min(c1), c0.max(c1));
        let r1 = r1.min(self.lines.len().saturating_sub(1));
        let mut text = String::new();
        for r in r0..=r1 {
            if r > r0 {
                text.push('\n');
            }
            let line = &self.lines[r];
            let chars: Vec<char> = line.chars().collect();
            let lo = c0.min(chars.len());
            let hi = (c1 + 1).min(chars.len());
            if lo < hi {
                text.extend(&chars[lo..hi]);
            }
        }
        text
    }

    /// Yank a column rectangle `[r0..=r1] × [c0..=c1]` into the yank
    /// register, rows joined by `\n`.
    pub fn yank_block(&mut self, r0: usize, c0: usize, r1: usize, c1: usize) {
        self.yank = self.block_text(r0, c0, r1, c1);
    }

    /// Delete a column rectangle, stashing into yank. Shorter lines are
    /// trimmed at their end rather than padded.
    pub fn delete_block(&mut self, r0: usize, c0: usize, r1: usize, c1: usize) {
        let (r0, r1) = (r0.min(r1), r0.max(r1));
        let (c0, c1) = (c0.min(c1), c0.max(c1));
        let r1 = r1.min(self.lines.len().saturating_sub(1));
        self.yank_block(r0, c0, r1, c1);
        for r in r0..=r1 {
            let line = self.lines[r].clone();
            let nchars = line.chars().count();
            let lo = c0.min(nchars);
            let hi = (c1 + 1).min(nchars);
            if lo >= hi {
                continue;
            }
            let lo_b = char_to_byte(&line, lo);
            let hi_b = char_to_byte(&line, hi);
            self.lines[r].replace_range(lo_b..hi_b, "");
        }
        self.cursor.row = r0;
        self.cursor.col = c0;
        self.clamp_col(false);
        self.touch();
    }
}

impl Buffer {
    /// Apply a per-character transform across the half-open range
    /// `[from, to)`. The two endpoints may sit on different rows.
    /// Backs the visual-mode `~` / `u` / `U` family.
    pub fn transform_case_range(&mut self, from: Cursor, to: Cursor, f: fn(char) -> char) {
        let (from, to) = order(from, to);
        if from == to {
            return;
        }
        for row in from.row..=to.row {
            let chars: Vec<char> = self.lines[row].chars().collect();
            let lo = if row == from.row { from.col } else { 0 };
            let hi = if row == to.row {
                to.col.min(chars.len())
            } else {
                chars.len()
            };
            if lo >= hi {
                continue;
            }
            self.lines[row] = chars
                .iter()
                .enumerate()
                .map(|(i, c)| if i >= lo && i < hi { f(*c) } else { *c })
                .collect();
        }
        self.touch();
    }

    /// Apply a per-character transform to every char on rows
    /// `[from_row..=to_row]`.
    pub fn transform_case_lines(&mut self, from_row: usize, to_row: usize, f: fn(char) -> char) {
        let (a, b) = (from_row.min(to_row), from_row.max(to_row));
        let b = b.min(self.lines.len().saturating_sub(1));
        for row in a..=b {
            self.lines[row] = self.lines[row].chars().map(f).collect();
        }
        self.touch();
    }

    /// Apply a per-character transform across a column rectangle.
    pub fn transform_case_block(
        &mut self,
        r0: usize,
        c0: usize,
        r1: usize,
        c1: usize,
        f: fn(char) -> char,
    ) {
        let (r0, r1) = (r0.min(r1), r0.max(r1));
        let (c0, c1) = (c0.min(c1), c0.max(c1));
        let r1 = r1.min(self.lines.len().saturating_sub(1));
        for row in r0..=r1 {
            let chars: Vec<char> = self.lines[row].chars().collect();
            self.lines[row] = chars
                .iter()
                .enumerate()
                .map(|(i, c)| if i >= c0 && i <= c1 { f(*c) } else { *c })
                .collect();
        }
        self.touch();
    }
}

/// Lowercase a char, keeping its column width. Multi-char expansions
/// (eg. Turkish `İ` → two codepoints) fall back to the original so
/// column counts stay stable.
pub fn to_lower_keep_width(c: char) -> char {
    if c.is_uppercase() {
        let mut it = c.to_lowercase();
        let first = it.next().unwrap_or(c);
        if it.next().is_some() { c } else { first }
    } else {
        c
    }
}

/// Uppercase a char, keeping its column width. See [`to_lower_keep_width`].
pub fn to_upper_keep_width(c: char) -> char {
    if c.is_lowercase() {
        let mut it = c.to_uppercase();
        let first = it.next().unwrap_or(c);
        if it.next().is_some() { c } else { first }
    } else {
        c
    }
}

/// Flip a single character's case: upper→lower, lower→upper, others
/// unchanged. For chars whose case expansion is multi-char (a tiny
/// minority — eg. German `ß` → `SS`) we fall back to the original
/// char to keep column counts stable.
pub fn flip_case_char_keep_width(c: char) -> char {
    if c.is_uppercase() {
        let mut it = c.to_lowercase();
        let first = it.next().unwrap_or(c);
        if it.next().is_some() { c } else { first }
    } else if c.is_lowercase() {
        let mut it = c.to_uppercase();
        let first = it.next().unwrap_or(c);
        if it.next().is_some() { c } else { first }
    } else {
        c
    }
}

fn order(a: Cursor, b: Cursor) -> (Cursor, Cursor) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

// ────────────────────────────────────────────────────────────────────────
// Line-level edits.
// ────────────────────────────────────────────────────────────────────────

impl Buffer {
    /// Join the next line into the current one with a single space
    /// separator (vim's `J`). Strips leading whitespace on the joined
    /// line; if the current line ends in whitespace or is empty, no
    /// space is inserted. Cursor lands on the join boundary.
    pub fn join_next_line(&mut self) {
        if self.cursor.row + 1 >= self.lines.len() {
            return;
        }
        let next = self.lines.remove(self.cursor.row + 1);
        let next_trimmed = next.trim_start();
        let cur = &mut self.lines[self.cursor.row];
        let needs_space = !cur.is_empty()
            && !cur
                .chars()
                .last()
                .map(|c| c.is_whitespace())
                .unwrap_or(false)
            && !next_trimmed.is_empty();
        let join_col = cur.chars().count();
        if needs_space {
            cur.push(' ');
        }
        cur.push_str(next_trimmed);
        self.cursor.col = join_col;
        self.touch();
    }

    /// Toggle the case of the character under the cursor, then advance
    /// one column (vim's `~`). No-op on an empty line.
    pub fn toggle_case_under_cursor(&mut self) {
        let line = &mut self.lines[self.cursor.row];
        if self.cursor.col >= line.chars().count() {
            return;
        }
        let byte_idx = char_to_byte(line, self.cursor.col);
        let ch = line[byte_idx..].chars().next().unwrap();
        let replacement: String = if ch.is_uppercase() {
            ch.to_lowercase().collect()
        } else if ch.is_lowercase() {
            ch.to_uppercase().collect()
        } else {
            return; // not a cased letter — leave it and don't advance
        };
        line.replace_range(byte_idx..byte_idx + ch.len_utf8(), &replacement);
        self.touch();
        // Advance, allowing past-end only inside Insert (we're in Normal
        // here, so clamp to last col).
        let max = self.current_line_len().saturating_sub(1);
        if self.cursor.col < max {
            self.cursor.col += 1;
        }
    }

    /// Delete from `cursor` to the end of the current line (vim's `D`).
    /// The deleted text goes into the yank register.
    pub fn delete_to_eol(&mut self) {
        let line = self.lines[self.cursor.row].clone();
        let byte_idx = char_to_byte(&line, self.cursor.col);
        self.yank = line[byte_idx..].to_string();
        self.lines[self.cursor.row].truncate(byte_idx);
        self.touch();
        self.clamp_col(false);
    }

    /// Replace the entire current line with an empty string (vim's
    /// `S`). The full line content goes into the yank register.
    pub fn clear_current_line(&mut self) {
        self.yank = self.lines[self.cursor.row].clone();
        self.lines[self.cursor.row].clear();
        self.cursor.col = 0;
        self.touch();
    }

    /// Toggle a block-aligned line comment across `rows` using `token`
    /// (e.g. `"//"`, `"#"`).
    ///
    /// Block semantics: the operation finds the **shallowest indent
    /// among the non-blank target rows** and uses that single column as
    /// the comment anchor for every row. So a mixed-indent block
    /// commented together stays visually aligned (the deeper rows get
    /// `// ` inserted *before* their own extra indent, not at the
    /// per-row first-non-blank position).
    ///
    /// Toggle direction: if every non-blank target row already starts
    /// with `token` at the shared anchor, the prefix (and a single
    /// trailing space, when present) is stripped from each; otherwise
    /// `token + " "` is inserted at the anchor on every row. Blank
    /// lines are skipped from both the indent calculation and the
    /// mutation — vim-commentary semantics generalized to a block.
    ///
    /// Cursor bookkeeping: the primary cursor and every extra cursor
    /// on a mutated row gets shifted by the column delta. Cursors that
    /// land inside a deleted range collapse to the anchor column.
    /// Single-row callers can just pass `&[row]`.
    pub fn toggle_block_comment(&mut self, token: &str, rows: &[usize]) {
        let mut rows: Vec<usize> = rows
            .iter()
            .copied()
            .filter(|&r| r < self.lines.len())
            .collect();
        rows.sort_unstable();
        rows.dedup();

        // (row, indent_chars) for rows with any non-whitespace content.
        let non_blank: Vec<(usize, usize)> = rows
            .iter()
            .filter_map(|&row| {
                let line = &self.lines[row];
                let indent_chars = line.chars().take_while(|c| c.is_whitespace()).count();
                if indent_chars == line.chars().count() {
                    None
                } else {
                    Some((row, indent_chars))
                }
            })
            .collect();
        if non_blank.is_empty() {
            return;
        }

        let anchor = non_blank.iter().map(|&(_, i)| i).min().unwrap();
        let token_bytes = token.len();
        let token_chars = token.chars().count();

        // Note: anchor <= every row's indent_chars by construction, so
        // the byte at column `anchor` is always whitespace or the first
        // non-blank char — never the middle of a multi-byte cluster.
        let all_commented = non_blank.iter().all(|&(row, _)| {
            let line = &self.lines[row];
            let byte = char_to_byte(line, anchor);
            line[byte..].starts_with(token)
        });

        // Per-row signed char delta at column `anchor`.
        let mut deltas: Vec<(usize, i32)> = Vec::with_capacity(non_blank.len());
        if all_commented {
            for &(row, _) in &non_blank {
                let line = &mut self.lines[row];
                let byte = char_to_byte(line, anchor);
                let after_token = &line[byte + token_bytes..];
                let trim_bytes = if after_token.starts_with(' ') {
                    token_bytes + 1
                } else {
                    token_bytes
                };
                let removed_chars = token_chars + (trim_bytes - token_bytes);
                line.replace_range(byte..byte + trim_bytes, "");
                deltas.push((row, -(removed_chars as i32)));
            }
        } else {
            let insert = format!("{} ", token);
            let added_chars = insert.chars().count();
            for &(row, _) in &non_blank {
                let line = &mut self.lines[row];
                let byte = char_to_byte(line, anchor);
                line.insert_str(byte, &insert);
                deltas.push((row, added_chars as i32));
            }
        }

        self.for_each_cursor(|c| shift_cursor_for_block_comment(c, &deltas, anchor));
        self.clamp_col(false);
        self.touch();
    }

    /// Apply `f` to the primary cursor first, then every extra cursor
    /// in insertion order. Convenience for the post-edit fan-out pattern
    /// used by multi-cursor-aware mutations — saves writing the
    /// `&mut self.cursor` call + `for c in extra_cursors.iter_mut()`
    /// loop everywhere a structural edit shifts cursor positions.
    pub fn for_each_cursor(&mut self, mut f: impl FnMut(&mut Cursor)) {
        f(&mut self.cursor);
        for c in self.extra_cursors.iter_mut() {
            f(c);
        }
    }

    /// Wrap (or unwrap) the half-open range `[from, to)` with the
    /// `open` / `close` token pair — i.e. block-comment toggle for
    /// languages with a `(prefix, suffix)` comment syntax like
    /// `/* … */` or `<!-- … -->`.
    ///
    /// Two layout forms, picked automatically from the range shape:
    ///
    /// - **Inline** (default): `open` is inserted at `lo`, `close` at
    ///   `hi`, both on the same rows the range already spans. Used for
    ///   `gbiw`, `gbi(`, and any non-line-aligned target.
    /// - **Own-line**: when the range is multi-row AND `lo` sits at
    ///   column 0 AND `hi` is at a line boundary (column 0 of the row
    ///   *after* the content, or end-of-line of the last row), `open`
    ///   and `close` each go on their own new rows above / below the
    ///   content. So `gbap`, `gbi{` over multi-line braces, etc. lay
    ///   out as a clean block:
    ///
    ///   ```text
    ///   /*
    ///   foo
    ///   bar
    ///   */
    ///   ```
    ///
    /// Toggle direction:
    ///
    /// - Inline strip when the range starts with `open` at `lo` and
    ///   ends with `close` at `hi` (same-row tokens).
    /// - Own-line strip when the entire row at `lo.row` equals `open`
    ///   and the row just before `hi` equals `close` — the natural
    ///   shape after a previous own-line wrap, including the case
    ///   where the text-object re-selects the wrapped block.
    /// - Otherwise: wrap (own-line when the shape allows, inline
    ///   otherwise).
    ///
    /// Cursor bookkeeping: every cursor (primary + extras) gets its
    /// row and column adjusted per edit. Same-row column shifts go
    /// through `shift_cursor_for_edit`; whole-row insertions and
    /// removals go through `shift_cursor_for_row_delta`.
    pub fn toggle_block_wrap(&mut self, open: &str, close: &str, from: Cursor, to: Cursor) {
        let (lo, hi) = order(from, to);
        if lo == hi {
            return;
        }
        let open_chars = open.chars().count();
        let close_chars = close.chars().count();

        // Where does the close token actually live on disk for an
        // own-line layout? If `hi.col == 0` the range exclusive end
        // sits on the row *after* the content (paragraph text-object
        // convention from `range_for_full_lines`), so the close row is
        // hi.row − 1. Otherwise hi is on the content's last row.
        let own_close_row = if hi.col == 0 && hi.row > 0 {
            hi.row - 1
        } else {
            hi.row
        };

        // Own-line strip: open alone on lo.row, close alone on
        // own_close_row, with at least one content row in between. The
        // exact equality on `lines[r]` is intentional — anything else
        // means a user edited the wrap or it isn't ours to strip.
        let own_line_wrapped = lo.col == 0
            && own_close_row > lo.row
            && self.lines[lo.row] == open
            && self.lines[own_close_row] == close;

        let starts_open = !own_line_wrapped && {
            let line = &self.lines[lo.row];
            let byte = char_to_byte(line, lo.col);
            line[byte..].starts_with(open)
        };
        let ends_close = !own_line_wrapped && hi.col >= close_chars && {
            let line = &self.lines[hi.row];
            let a = char_to_byte(line, hi.col - close_chars);
            let b = char_to_byte(line, hi.col);
            &line[a..b] == close
        };

        if own_line_wrapped {
            // Remove the close row first (higher index, doesn't
            // invalidate lo.row), then the open row.
            self.lines.remove(own_close_row);
            self.for_each_cursor(|c| shift_cursor_for_row_delta(c, own_close_row, -1));
            self.lines.remove(lo.row);
            self.for_each_cursor(|c| shift_cursor_for_row_delta(c, lo.row, -1));
        } else if starts_open && ends_close {
            // Inline strip — close first so the open's coords stay
            // valid for the second edit.
            let close_start = hi.col - close_chars;
            let a = char_to_byte(&self.lines[hi.row], close_start);
            let b = char_to_byte(&self.lines[hi.row], hi.col);
            self.lines[hi.row].replace_range(a..b, "");
            self.for_each_cursor(|c| shift_cursor_for_edit(c, hi.row, close_start, close_chars, 0));

            let a = char_to_byte(&self.lines[lo.row], lo.col);
            let b = char_to_byte(&self.lines[lo.row], lo.col + open_chars);
            self.lines[lo.row].replace_range(a..b, "");
            self.for_each_cursor(|c| shift_cursor_for_edit(c, lo.row, lo.col, open_chars, 0));
        } else if should_use_own_line(&self.lines, lo, hi) {
            // Insert close row first (higher index keeps lo coords
            // valid). `own_close_row` here points at the row *after*
            // the last content row, where we want the close to land.
            let close_insert_row = own_close_row + 1;
            self.lines.insert(close_insert_row, close.to_string());
            self.for_each_cursor(|c| shift_cursor_for_row_delta(c, close_insert_row, 1));
            self.lines.insert(lo.row, open.to_string());
            self.for_each_cursor(|c| shift_cursor_for_row_delta(c, lo.row, 1));
        } else {
            // Inline wrap — close first so open's coords aren't shifted
            // (same-row case: lo.col < hi.col, close insert at hi.col
            // leaves lo.col untouched).
            let hi_byte = char_to_byte(&self.lines[hi.row], hi.col);
            self.lines[hi.row].insert_str(hi_byte, close);
            self.for_each_cursor(|c| shift_cursor_for_edit(c, hi.row, hi.col, 0, close_chars));

            let lo_byte = char_to_byte(&self.lines[lo.row], lo.col);
            self.lines[lo.row].insert_str(lo_byte, open);
            self.for_each_cursor(|c| shift_cursor_for_edit(c, lo.row, lo.col, 0, open_chars));
        }
        self.clamp_col(false);
        self.touch();
    }
}

/// True when a fresh wrap of `[lo, hi)` should lay out open/close on
/// their own rows. Requires multi-row span, `lo` at column 0, and `hi`
/// at a line boundary (either column 0 of the next row or end-of-line
/// of the last content row).
fn should_use_own_line(lines: &[String], lo: Cursor, hi: Cursor) -> bool {
    if lo.col != 0 || lo.row == hi.row {
        return false;
    }
    hi.col == 0 || hi.col == lines[hi.row].chars().count()
}

/// Shift a cursor's row after a whole-row insertion or removal at
/// `pivot`. Positive `delta` = rows inserted at `pivot` (existing rows
/// at >= pivot move down); negative = a single row at `pivot` was
/// removed (rows after move up). Cursors strictly above `pivot` are
/// untouched; a cursor on the removed row clamps to `pivot` (the next
/// surviving row, or the new content if rows were inserted).
fn shift_cursor_for_row_delta(c: &mut Cursor, pivot: usize, delta: i32) {
    if delta > 0 {
        if c.row >= pivot {
            c.row += delta as usize;
        }
    } else if delta < 0 {
        let d = (-delta) as usize;
        if c.row > pivot {
            c.row = c.row.saturating_sub(d);
        }
        // c.row == pivot: the row was removed. Leave c.row at pivot so
        // it points at whatever now occupies that index.
    }
}

/// Adjust `c` for an edit on `row` that, starting at char column
/// `col_start`, removed `delete` chars and inserted `insert` in their
/// place. Pure column arithmetic — no row changes, no clamping.
pub(super) fn shift_cursor_for_edit(
    c: &mut Cursor,
    row: usize,
    col_start: usize,
    delete: usize,
    insert: usize,
) {
    if c.row != row {
        return;
    }
    if c.col >= col_start + delete {
        c.col = c.col + insert - delete;
    } else if c.col > col_start {
        // Cursor was inside the deleted span — pull it back to the
        // edit's start (where the inserted text now begins, if any).
        c.col = col_start + insert;
    }
}

/// Apply the per-row column delta from `toggle_block_comment` to a
/// single cursor. Cursors before the anchor are untouched; cursors
/// inside a deletion range collapse to the anchor column.
fn shift_cursor_for_block_comment(c: &mut Cursor, deltas: &[(usize, i32)], anchor: usize) {
    let Some(&(_, delta)) = deltas.iter().find(|(r, _)| *r == c.row) else {
        return;
    };
    if delta > 0 {
        if c.col >= anchor {
            c.col = c.col.saturating_add(delta as usize);
        }
    } else if delta < 0 {
        let d = (-delta) as usize;
        if c.col >= anchor + d {
            c.col -= d;
        } else if c.col > anchor {
            c.col = anchor;
        }
    }
}
