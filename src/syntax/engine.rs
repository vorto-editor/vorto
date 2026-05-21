//! Per-buffer tree-sitter facade.
//!
//! [`Engine`] owns the parser, the cached tree, the source the tree
//! was built from, and the per-concern query objects (highlights,
//! indents, text-objects, injections). The struct itself is mostly a
//! container — each per-concern module ([`super::highlight`],
//! [`super::indent`], [`super::textobject`], [`super::bracket`],
//! [`super::injection`]) holds the query logic; the engine just
//! delegates the facade methods that the rest of the editor calls.
//!
//! Re-parses lazily through [`Engine::refresh`]: when called with a
//! newer `version` than the cached one, the byte-level diff against
//! the previous source is fed to tree-sitter's incremental parser, so
//! single-keystroke edits stay sub-millisecond even on large files.
//!
//! All facade methods are safe to call when the tree hasn't been
//! built yet (fresh buffer, parse error) — they return empty
//! collections / `None` in that case.
//!
//! Failures during construction (`.so` load error, query compile
//! error, ABI mismatch) surface as `anyhow::Error` from
//! [`Engine::new`] so the caller can fall back to plain text.

use anyhow::{Context, Result};
use tree_sitter::{InputEdit, Language, Parser, Point, Tree};

use super::highlight::{Capture, HighlightQuery};
use super::indent::IndentQuery;
use super::injection::InjectionEngine;
use super::textobject::TextObjectQuery;
use super::{bracket, highlight, indent, textobject};

/// Per-buffer tree-sitter state. Owns the parser, the last-parsed
/// tree, and the per-concern query objects. Refreshes the tree only
/// when [`Self::refresh`] is called with a version newer than the one
/// already cached, so callers can poke at it freely from a hot draw
/// loop.
pub struct Engine {
    parser: Parser,
    tree: Option<Tree>,
    source: String,
    parsed_version: Option<u64>,
    highlight: HighlightQuery,
    indent: Option<IndentQuery>,
    textobject: Option<TextObjectQuery>,
    injection: Option<InjectionEngine>,
    /// Non-fatal warnings collected during construction (e.g. an
    /// `indents.scm` that failed to compile). The TUI drains these
    /// into toasts; writing to stderr from a worker thread would
    /// corrupt the alt-screen display.
    pub warnings: Vec<String>,
}

impl Engine {
    pub(super) fn new(
        language: Language,
        highlights_src: &str,
        textobjects_src: Option<&str>,
        indents_src: Option<&str>,
        injection: Option<InjectionEngine>,
    ) -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .context("setting parser language (ABI mismatch?)")?;
        let highlight = highlight::HighlightQuery::compile(&language, highlights_src)
            .context("compiling highlights query")?;

        let textobject = match textobjects_src {
            Some(src) => Some(
                textobject::TextObjectQuery::compile(&language, src)
                    .context("compiling textobjects query")?,
            ),
            None => None,
        };

        // Indents query failures are non-fatal: a bad node name in
        // indents.scm just disables auto-indent for the language —
        // we still want highlighting/textobjects to work.
        let mut warnings = Vec::new();
        let indent = match indents_src {
            Some(src) => match indent::IndentQuery::compile(&language, src) {
                Ok(q) => Some(q),
                Err(e) => {
                    warnings.push(format!(
                        "indents.scm compile failed, auto-indent disabled: {e}"
                    ));
                    None
                }
            },
            None => None,
        };

        Ok(Self {
            parser,
            tree: None,
            source: String::new(),
            parsed_version: None,
            highlight,
            indent,
            textobject,
            injection,
            warnings,
        })
    }

    /// Re-parse `source` if it's newer than the cached tree.
    ///
    /// When a previous tree is cached, computes the byte-range diff
    /// against the old source, applies it via `Tree::edit`, and asks
    /// tree-sitter to reuse the existing tree — incremental parsing is
    /// the whole point of `tree-sitter`. With incremental, edits past
    /// the affected node are O(1).
    pub fn refresh(&mut self, source: &str, version: u64) {
        if self.parsed_version == Some(version) {
            return;
        }
        let old_tree = match self.tree.as_mut() {
            Some(tree) if !self.source.is_empty() => {
                let edit = compute_input_edit(&self.source, source);
                tree.edit(&edit);
                Some(&*tree)
            }
            _ => None,
        };
        self.tree = self.parser.parse(source, old_tree);
        self.source = source.to_string();
        self.parsed_version = Some(version);
    }

    /// All highlight captures intersecting rows `[start_row..=end_row]`.
    /// Includes captures contributed by embedded sub-languages when an
    /// injection engine is configured. Columns are returned in
    /// characters (not bytes).
    pub fn captures_in_rows(&self, start_row: usize, end_row: usize) -> Vec<Capture> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let mut out = self
            .highlight
            .captures_in_rows(&self.source, tree, start_row, end_row);
        // Layer in sub-language captures *after* the host's so the
        // renderer's patch-merge stacks sub-language fg/bold on top of
        // any host capture covering the same region. The subsequent
        // stable sort keeps everything in document order while
        // preserving intra-row priority.
        if let Some(inj) = self.injection.as_ref() {
            out.extend(inj.captures_in_rows(&self.source, tree, start_row, end_row));
        }
        out.sort_by_key(|c| (c.start_row, c.start_col));
        out
    }

    /// True when the language's `indents.scm` reports an
    /// `@indent.begin` whose node opens on `row`. Used by auto-indent
    /// on newline / `o` / `O`.
    pub fn indent_begins_at(&self, row: usize) -> bool {
        let Some(tree) = &self.tree else {
            return false;
        };
        let Some(q) = self.indent.as_ref() else {
            return false;
        };
        q.begins_at(&self.source, tree, row)
    }

    /// Indent scopes intersecting `[start_row, end_row]`. Each tuple is
    /// `(scope_start_row, scope_end_row)` inclusive. Same-row scopes
    /// are dropped. Injection scopes from embedded sub-languages are
    /// merged so e.g. a TS function inside a Vue `<script>` shows up
    /// the same as in a standalone `.ts` file.
    pub fn indent_scopes_in_rows(&self, start_row: usize, end_row: usize) -> Vec<(usize, usize)> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(q) = self.indent.as_ref() {
            out.extend(q.scopes_in_rows(&self.source, tree, start_row, end_row));
        }
        if let Some(inj) = self.injection.as_ref() {
            out.extend(inj.indent_scopes_in_rows(&self.source, tree, start_row, end_row));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        out
    }

    /// Smallest text-object range matching `target` (e.g.
    /// `"function.outer"`) that contains `(cursor_row,
    /// cursor_col_chars)`. Returns row/col coordinates with `end`
    /// exclusive — ready to feed into `Buffer::delete_range` /
    /// `yank_range`.
    pub fn find_text_object(
        &self,
        target: &str,
        cursor_row: usize,
        cursor_col_chars: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        let tree = self.tree.as_ref()?;
        let q = self.textobject.as_ref()?;
        q.find(&self.source, tree, target, cursor_row, cursor_col_chars)
    }

    /// Bracket-pair mate of the character at `(row, col_chars)`, when
    /// tree-sitter resolved that position to a bracket token (i.e.
    /// brackets inside strings and comments are skipped automatically).
    pub fn matching_bracket(&self, row: usize, col_chars: usize) -> Option<(usize, usize)> {
        let tree = self.tree.as_ref()?;
        bracket::matching(&self.source, tree, row, col_chars)
    }
}

// ────────────────────────────────────────────────────────────────────
// Shared coordinate / edit helpers used across the per-concern modules.
// Tree-sitter speaks in bytes; the rest of the editor indexes lines
// by character — these helpers bridge between the two.
// ────────────────────────────────────────────────────────────────────

/// Translate a byte column on `row` into a character column. Tree-sitter
/// reports byte columns; the UI wants char columns to match how the
/// rest of the editor indexes into lines.
pub(super) fn byte_to_char_col(source: &str, row: usize, byte_col: usize) -> usize {
    let line = source.lines().nth(row).unwrap_or("");
    let take = byte_col.min(line.len());
    line[..take].chars().count()
}

/// Inverse of [`byte_to_char_col`]: given a character column, return
/// the byte column. Saturates at end-of-line.
pub(super) fn char_to_byte_col(source: &str, row: usize, char_col: usize) -> usize {
    let line = source.lines().nth(row).unwrap_or("");
    line.char_indices()
        .nth(char_col)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

/// Byte-level diff of `old` vs `new` packaged as a tree-sitter
/// [`InputEdit`]. Finds the longest shared prefix and suffix and
/// treats everything in between as the changed region. For a
/// one-keystroke insertion this collapses to a single-byte edit at
/// the cursor, which is exactly what makes incremental reparse fast.
fn compute_input_edit(old: &str, new: &str) -> InputEdit {
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    let common_prefix = old_bytes
        .iter()
        .zip(new_bytes.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let max_suffix = old_bytes
        .len()
        .min(new_bytes.len())
        .saturating_sub(common_prefix);
    let common_suffix = old_bytes
        .iter()
        .rev()
        .zip(new_bytes.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let start_byte = common_prefix;
    let old_end_byte = old_bytes.len() - common_suffix;
    let new_end_byte = new_bytes.len() - common_suffix;
    InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position: byte_to_point(old_bytes, start_byte),
        old_end_position: byte_to_point(old_bytes, old_end_byte),
        new_end_position: byte_to_point(new_bytes, new_end_byte),
    }
}

/// Convert a byte offset within `bytes` to a `(row, byte-column)`
/// [`Point`]. Linear scan from the start.
fn byte_to_point(bytes: &[u8], offset: usize) -> Point {
    let offset = offset.min(bytes.len());
    let mut row = 0usize;
    let mut line_start = 0usize;
    for (i, &b) in bytes[..offset].iter().enumerate() {
        if b == b'\n' {
            row += 1;
            line_start = i + 1;
        }
    }
    Point {
        row,
        column: offset - line_start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_to_char_handles_ascii() {
        let src = "let x = 1\nprintln!(\"hi\")";
        assert_eq!(byte_to_char_col(src, 0, 0), 0);
        assert_eq!(byte_to_char_col(src, 0, 4), 4);
        assert_eq!(byte_to_char_col(src, 1, 9), 9);
    }

    #[test]
    fn byte_to_char_handles_multibyte() {
        // "あ" is 3 bytes in UTF-8 → 1 char.
        let src = "あ x";
        assert_eq!(byte_to_char_col(src, 0, 3), 1);
        assert_eq!(byte_to_char_col(src, 0, 5), 3);
    }

    #[test]
    fn input_edit_single_byte_insertion() {
        let edit = compute_input_edit("abc", "abXc");
        assert_eq!(edit.start_byte, 2);
        assert_eq!(edit.old_end_byte, 2);
        assert_eq!(edit.new_end_byte, 3);
        assert_eq!(edit.start_position, Point { row: 0, column: 2 });
        assert_eq!(edit.new_end_position, Point { row: 0, column: 3 });
    }

    #[test]
    fn input_edit_no_change_is_noop_range() {
        let edit = compute_input_edit("hello", "hello");
        assert_eq!(edit.start_byte, 5);
        assert_eq!(edit.old_end_byte, 5);
        assert_eq!(edit.new_end_byte, 5);
    }

    #[test]
    fn input_edit_multi_line_replacement() {
        let edit = compute_input_edit("fn a() {\n  1\n}\n", "fn a() {\n  42\n}\n");
        assert_eq!(edit.start_byte, 11);
        assert_eq!(edit.old_end_byte, 15 - 3);
        assert_eq!(edit.new_end_byte, 16 - 3);
        assert_eq!(edit.start_position, Point { row: 1, column: 2 });
    }

    #[test]
    fn input_edit_full_replacement() {
        let edit = compute_input_edit("abc", "xyz");
        assert_eq!(edit.start_byte, 0);
        assert_eq!(edit.old_end_byte, 3);
        assert_eq!(edit.new_end_byte, 3);
    }
}
