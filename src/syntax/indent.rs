//! Indents query handling.
//!
//! Drives the auto-indent behavior on `<newline>` / `o` / `O` and the
//! tree-sitter half of the indent-guide active-scope detection. Both
//! paths revolve around the `@indent.begin` capture from the
//! language's `indents.scm`.

use anyhow::Result;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator, Tree};

/// Compiled `indents.scm` query for one language.
pub struct IndentQuery {
    query: Query,
    capture_names: Vec<String>,
}

impl IndentQuery {
    pub(super) fn compile(language: &Language, src: &str) -> Result<Self> {
        let query = Query::new(language, src)?;
        let capture_names = query
            .capture_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        Ok(Self {
            query,
            capture_names,
        })
    }

    /// True when the indents query has an `@indent.begin` capture
    /// whose node *opens* on `row`. Used by auto-indent on newline /
    /// `o` / `O`.
    ///
    /// Two shapes are accepted:
    /// - `start_row == row && end_row > row`: the node already has a
    ///   body that wraps the following lines (e.g. `def f():\n    x`).
    /// - `start_row == row && end_row == row` with an empty `body`
    ///   child: the node is mid-construction — the user just typed
    ///   `def f():` and hasn't filled in the body yet, so tree-sitter
    ///   reports a zero-width body block on the same row. Without
    ///   this branch Python auto-indent never fires while typing,
    ///   only after-the-fact when there's already body content below.
    pub(super) fn begins_at(&self, source: &str, tree: &Tree, row: usize) -> bool {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let name = self
                    .capture_names
                    .get(cap.index as usize)
                    .map(String::as_str)
                    .unwrap_or("");
                if name != "indent.begin" {
                    continue;
                }
                let node = cap.node;
                let start_row = node.start_position().row;
                let end_row = node.end_position().row;
                if start_row != row {
                    continue;
                }
                if end_row > row {
                    return true;
                }
                // Same-row span: distinguish an incomplete header
                // (empty body, user about to type it) from a true
                // one-liner (`if x: y` — body has content, no
                // auto-indent wanted).
                if let Some(body) = node.child_by_field_name("body")
                    && body.start_byte() == body.end_byte()
                {
                    return true;
                }
            }
        }
        false
    }

    /// Indent scopes intersecting `[start_row, end_row]`. Returns
    /// `(scope_start_row, scope_end_row)` inclusive on both ends.
    /// Same-row scopes are dropped — they contribute no body rows to
    /// draw a guide on. The caller is responsible for sorting / merging
    /// with injection scopes.
    pub(super) fn scopes_in_rows(
        &self,
        source: &str,
        tree: &Tree,
        start_row: usize,
        end_row: usize,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let name = self
                    .capture_names
                    .get(cap.index as usize)
                    .map(String::as_str)
                    .unwrap_or("");
                if name != "indent.begin" {
                    continue;
                }
                let node = cap.node;
                let s = node.start_position().row;
                let e = node.end_position().row;
                if e <= s {
                    continue;
                }
                if e < start_row || s > end_row {
                    continue;
                }
                out.push((s, e));
            }
        }
        out
    }
}
