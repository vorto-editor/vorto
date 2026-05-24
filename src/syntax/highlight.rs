//! Highlights query handling.
//!
//! [`HighlightQuery`] compiles a language's `highlights.scm` once at
//! construction and exposes [`HighlightQuery::captures_in_rows`] to
//! produce the styled spans the UI overlay layer consumes. Capture
//! names are returned as-is — the theme module (`super::theme`) does
//! name → style resolution downstream.

use anyhow::Result;
use tree_sitter::{Language, Point, Query, QueryCursor, StreamingIterator, Tree};

use super::engine::byte_to_char_col_indexed;

/// Compiled `highlights.scm` query for one language.
pub struct HighlightQuery {
    query: Query,
    capture_names: Vec<String>,
}

impl HighlightQuery {
    /// Compile `src` against `language`. Errors when the query is
    /// malformed or references nodes that don't exist in the grammar.
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

    /// Return all captures intersecting rows `[start_row..=end_row]`.
    /// Columns are converted from byte offsets to character offsets so
    /// callers can directly index into character-based line strings.
    /// Caller is responsible for sorting / merging when other capture
    /// sources (injections) are combined in.
    pub(super) fn captures_in_rows(
        &self,
        source: &str,
        line_starts: &[usize],
        tree: &Tree,
        start_row: usize,
        end_row: usize,
    ) -> Vec<Capture> {
        let mut cursor = QueryCursor::new();
        // Restrict the query to the visible row window. Without this
        // the cursor walks every match in the whole document on every
        // frame and the row filter below discards all but ~viewport
        // rows — so highlight cost scaled with file size, not what's
        // on screen. `end_row + 1` makes the range cover `end_row`'s
        // full line; column 0 of the row past the window is the first
        // byte we no longer care about.
        cursor.set_point_range(
            Point {
                row: start_row,
                column: 0,
            }..Point {
                row: end_row.saturating_add(1),
                column: 0,
            },
        );
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        let mut out = Vec::new();
        // `QueryMatches` is a streaming iterator in tree-sitter 0.25+,
        // so we drive it with an explicit `.next()` loop rather than
        // `for ... in`.
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let node = cap.node;
                let start = node.start_position();
                let end = node.end_position();
                if end.row < start_row || start.row > end_row {
                    continue;
                }
                let name = self
                    .capture_names
                    .get(cap.index as usize)
                    .cloned()
                    .unwrap_or_default();
                out.push(Capture {
                    start_row: start.row,
                    start_col: byte_to_char_col_indexed(
                        source,
                        line_starts,
                        start.row,
                        start.column,
                    ),
                    end_row: end.row,
                    end_col: byte_to_char_col_indexed(source, line_starts, end.row, end.column),
                    name,
                });
            }
        }
        out
    }
}

/// One styled range delivered by the query engine. Coordinates are
/// inclusive on `start`, exclusive on `end`, in *characters* (not
/// bytes) — already converted by [`HighlightQuery::captures_in_rows`].
#[derive(Debug, Clone)]
pub struct Capture {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    pub name: String,
}
