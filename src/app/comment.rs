//! Shared comment-toggle entry points used by both the Normal-mode
//! operator dispatch (`gc` / `gb`) and Visual-mode application.
//!
//! Keeping the language-token lookup and the line-vs-block toggle
//! decision here means both call sites pick up the same fallback
//! behavior (e.g. `gb` on Python collapsing to per-row `#`) without
//! re-implementing the policy.

use crate::app::App;
use crate::editor::Cursor;

impl App {
    /// Single-line comment prefix for the active buffer's language, if
    /// any. Returns `None` when the buffer has no path, the language is
    /// unknown, or `comment_token` is unset (e.g. JSON).
    pub fn line_comment_token(&self) -> Option<String> {
        let path = self.active_doc().path.as_deref()?;
        let lang = self.config.languages.by_path(path)?;
        lang.comment_token.clone()
    }

    /// Block-comment `(open, close)` pair for the active buffer's
    /// language, if any. `None` for languages with no native block
    /// comment (e.g. Python, shell).
    pub fn block_comment_tokens(&self) -> Option<(String, String)> {
        let path = self.active_doc().path.as_deref()?;
        let lang = self.config.languages.by_path(path)?;
        lang.block_comment_token.clone()
    }

    /// Toggle line comments across `rows` using the active language's
    /// `comment_token`. Returns `false` (with the buffer untouched)
    /// when the language has no line-comment token configured.
    pub fn apply_line_comment(&mut self, rows: &[usize]) -> bool {
        let Some(tok) = self.line_comment_token() else {
            return false;
        };
        let r = self.editor.doc.clone();
        let doc = self.documents.get_mut(&r).expect("active doc present");
        self.editor.toggle_block_comment(doc, &tok, rows);
        true
    }

    /// Toggle a block-comment wrap around `range` (or the spanning
    /// rectangle of `rows` if `range` is `None`). Falls back to
    /// `apply_line_comment` when the language has no block tokens, so
    /// `gb` on Python still does *something* sensible.
    ///
    /// Returns `false` when neither block- nor line-comment tokens are
    /// available — the caller decides whether to surface that as a
    /// toast / `Cmd::ToastError`.
    pub fn apply_block_comment(&mut self, rows: &[usize], range: Option<(Cursor, Cursor)>) -> bool {
        match self.block_comment_tokens() {
            Some((open, close)) => {
                let (lo, hi) = range.unwrap_or_else(|| {
                    let lo_row = rows.iter().min().copied().unwrap_or(0);
                    let hi_row = rows.iter().max().copied().unwrap_or(0);
                    let hi_col = self.active_doc().lines[hi_row].chars().count();
                    (
                        Cursor {
                            row: lo_row,
                            col: 0,
                        },
                        Cursor {
                            row: hi_row,
                            col: hi_col,
                        },
                    )
                });
                let r = self.editor.doc.clone();
                let doc = self.documents.get_mut(&r).expect("active doc present");
                let (lo, hi) = trim_blank_edges(&doc.lines, lo, hi);
                self.editor.toggle_block_wrap(doc, &open, &close, lo, hi);
                true
            }
            None => self.apply_line_comment(rows),
        }
    }
}

/// Pull `lo` / `hi` inward past any all-whitespace rows on either edge,
/// but only when the range is already line-aligned (lo at column 0 and
/// hi at a line boundary). Driven by `gbap`-style cases where vim's
/// `Around Paragraph` greedily swallows the leading blank lines when
/// there are no trailing blanks — without trimming, the block wrap
/// would put `/*` above unrelated whitespace.
///
/// Mid-line ranges (`gbi(`, `gbiw`, etc.) and ranges where every row
/// is blank are returned unchanged: nothing meaningful to trim.
fn trim_blank_edges(lines: &[String], lo: Cursor, hi: Cursor) -> (Cursor, Cursor) {
    if lo.col != 0 {
        return (lo, hi);
    }
    let hi_at_boundary = hi.col == 0 || hi.col == lines[hi.row].chars().count();
    if !hi_at_boundary {
        return (lo, hi);
    }
    let last_content_row = if hi.col == 0 && hi.row > 0 {
        hi.row - 1
    } else {
        hi.row
    };
    let is_blank = |r: usize| lines[r].chars().all(|c| c.is_whitespace());

    let mut first = lo.row;
    while first <= last_content_row && is_blank(first) {
        first += 1;
    }
    if first > last_content_row {
        return (lo, hi);
    }
    let mut last = last_content_row;
    while last > first && is_blank(last) {
        last -= 1;
    }

    let new_lo = Cursor { row: first, col: 0 };
    let new_hi = if hi.col == 0 {
        Cursor {
            row: last + 1,
            col: 0,
        }
    } else {
        Cursor {
            row: last,
            col: lines[last].chars().count(),
        }
    };
    (new_lo, new_hi)
}
