//! Text-objects query handling.
//!
//! [`TextObjectQuery`] resolves names like `"function.outer"` or
//! `"class.inner"` into byte ranges inside the buffer, with support for
//! both direct captures and `#make-range!` predicates (the
//! nvim-treesitter convention for `.inner` ranges that exclude
//! delimiting braces).

use anyhow::Result;
use tree_sitter::{Language, Query, QueryCursor, QueryPredicateArg, StreamingIterator, Tree};

use super::engine::{byte_to_char_col, char_to_byte_col};

/// Compiled `textobjects.scm` query for one language.
pub struct TextObjectQuery {
    query: Query,
    capture_names: Vec<String>,
}

impl TextObjectQuery {
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

    /// Walk every match and invoke `yield_range` once per range captured
    /// as `target` — both direct captures (`@function.inner`) and ranges
    /// synthesized by a `#make-range!` predicate (`@function.outer` built
    /// from `_start` / `_end` captures). The callback receives the byte
    /// range plus the `(row, byte_col)` start/end points. Shared by
    /// [`Self::find`] (cursor filter) and [`Self::all`] (collect all) so
    /// the predicate parsing lives in one place.
    fn for_each_range(
        &self,
        source: &str,
        tree: &Tree,
        target: &str,
        mut yield_range: impl FnMut(std::ops::Range<usize>, (usize, usize), (usize, usize)),
    ) {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            // 1. Direct captures with the target name.
            for cap in m.captures {
                let name = self
                    .capture_names
                    .get(cap.index as usize)
                    .map(String::as_str)
                    .unwrap_or("");
                if name != target {
                    continue;
                }
                let node = cap.node;
                yield_range(
                    node.start_byte()..node.end_byte(),
                    point(node.start_position()),
                    point(node.end_position()),
                );
            }

            // 2. Ranges synthesized via `#make-range!` predicates on
            //    this pattern.
            for pred in self.query.general_predicates(m.pattern_index) {
                if pred.operator.as_ref() != "make-range!" {
                    continue;
                }
                let (name, start_idx, end_idx) = match pred.args.as_ref() {
                    [
                        QueryPredicateArg::String(n),
                        QueryPredicateArg::Capture(s),
                        QueryPredicateArg::Capture(e),
                    ] => (n.as_ref(), *s, *e),
                    _ => continue,
                };
                if name != target {
                    continue;
                }
                // Span = (min start across all `_start` captures) ..
                //        (max end across all `_end` captures).
                let mut span_start: Option<tree_sitter::Node> = None;
                let mut span_end: Option<tree_sitter::Node> = None;
                for cap in m.captures {
                    if cap.index == start_idx {
                        span_start = match span_start {
                            None => Some(cap.node),
                            Some(prev) if cap.node.start_byte() < prev.start_byte() => {
                                Some(cap.node)
                            }
                            other => other,
                        };
                    }
                    if cap.index == end_idx {
                        span_end = match span_end {
                            None => Some(cap.node),
                            Some(prev) if cap.node.end_byte() > prev.end_byte() => Some(cap.node),
                            other => other,
                        };
                    }
                }
                if let (Some(s), Some(e)) = (span_start, span_end) {
                    yield_range(
                        s.start_byte()..e.end_byte(),
                        point(s.start_position()),
                        point(e.end_position()),
                    );
                }
            }
        }
    }

    /// Smallest range matching `target` that contains the cursor.
    /// Returns `(start_row, start_col_chars, end_row, end_col_chars)`
    /// with `end` exclusive.
    pub(super) fn find(
        &self,
        source: &str,
        tree: &Tree,
        target: &str,
        cursor_row: usize,
        cursor_col_chars: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        // Cursor as a tree-sitter Point: row is line index, column is
        // byte offset within that line.
        let cursor_pt = (
            cursor_row,
            char_to_byte_col(source, cursor_row, cursor_col_chars),
        );

        // Best candidate so far, tracked by byte-length so we pick the
        // innermost. (Multiple matches can contain the cursor when
        // text objects nest, e.g. inner function inside outer impl.)
        let mut best: Option<Candidate> = None;
        self.for_each_range(source, tree, target, |bytes, start, end| {
            consider(&mut best, bytes, start, end, cursor_pt);
        });

        let c = best?;
        Some((
            c.start.0,
            byte_to_char_col(source, c.start.0, c.start.1),
            c.end.0,
            byte_to_char_col(source, c.end.0, c.end.1),
        ))
    }

    /// Every range matching `target` anywhere in the tree, regardless of
    /// cursor position. Unlike [`Self::find`] (which returns the single
    /// innermost range under the cursor), this enumerates the whole file
    /// so a golden test can snapshot all objects of a kind. Ranges are
    /// `(start_row, start_col_chars, end_row, end_col_chars)`, end
    /// exclusive, sorted and de-duplicated.
    #[cfg(test)]
    pub(super) fn all(
        &self,
        source: &str,
        tree: &Tree,
        target: &str,
    ) -> Vec<(usize, usize, usize, usize)> {
        let mut out: Vec<(usize, usize, usize, usize)> = Vec::new();
        self.for_each_range(source, tree, target, |_bytes, start, end| {
            out.push((
                start.0,
                byte_to_char_col(source, start.0, start.1),
                end.0,
                byte_to_char_col(source, end.0, end.1),
            ));
        });
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Candidate range during the inner search. Keeps both byte and Point
/// info so we can compare sizes cheaply while still returning row/col
/// coordinates at the end.
struct Candidate {
    bytes: std::ops::Range<usize>,
    start: (usize, usize),
    end: (usize, usize),
}

fn point(p: tree_sitter::Point) -> (usize, usize) {
    (p.row, p.column)
}

/// Replace `best` with `range` when it contains the cursor and is
/// strictly smaller than what's there. "Smaller" is by byte count, so
/// nested objects (e.g. inner function inside an outer impl) resolve
/// to the innermost one.
fn consider(
    best: &mut Option<Candidate>,
    bytes: std::ops::Range<usize>,
    start: (usize, usize),
    end: (usize, usize),
    cursor: (usize, usize),
) {
    if !(start <= cursor && cursor < end) {
        return;
    }
    let len = bytes.end - bytes.start;
    let take = match best {
        None => true,
        Some(c) => len < c.bytes.end - c.bytes.start,
    };
    if take {
        *best = Some(Candidate { bytes, start, end });
    }
}
