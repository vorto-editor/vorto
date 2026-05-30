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
                // Direct captures are raw AST nodes, so the editor's
                // notion of the object may differ (a `parameter.outer`
                // should swallow its comma; a brace-block `*.inner` should
                // drop its braces). `adjust_for_node` applies those using
                // the node's AST context.
                let (b, sp, ep) = adjust_for_node(target, source.as_bytes(), cap.node);
                yield_range(b, sp, ep);
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
                    // `#make-range!` predicates already define their exact
                    // intended boundaries (comma-swallowing outers,
                    // brace-trimmed inners), so they are yielded as-is — no
                    // post-processing, which would double-apply.
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

/// Post-process a raw direct-capture node into the editor's notion of the
/// text object, using the node's AST context:
///
/// * `parameter.outer` grows to swallow its list separator — but only when
///   an adjacent sibling is an actual `,` token, so a literal comma
///   *argument* (e.g. fish's `string join ,`) is left alone.
/// * `function.inner` / `class.inner` shrink to the body inside their
///   braces — but only when `{`/`}` are direct children of the captured
///   node, so a body that merely *contains* a brace expression (Lua's
///   `return { … }`, a Nix lambda returning an attrset) is left whole.
///
/// Every other target is returned as-is. `#make-range!` results never reach
/// here: they already define their exact boundaries, so the caller yields
/// them unmodified rather than risk double-trimming.
fn adjust_for_node(
    target: &str,
    src: &[u8],
    node: tree_sitter::Node,
) -> (std::ops::Range<usize>, (usize, usize), (usize, usize)) {
    match target {
        "parameter.outer" => extend_separator(src, node),
        "function.inner" | "class.inner" => shrink_braces(src, node),
        _ => raw_range(node),
    }
}

/// The node's own byte range and start/end Points, unmodified.
fn raw_range(node: tree_sitter::Node) -> (std::ops::Range<usize>, (usize, usize), (usize, usize)) {
    (
        node.start_byte()..node.end_byte(),
        point(node.start_position()),
        point(node.end_position()),
    )
}

/// Grow a `parameter.outer` node to swallow its list separator, so `daa`
/// removes the argument *and* one comma. Prefers a trailing `,` sibling
/// (plus the blanks after it); for the last item it takes the leading `,`.
/// The separator must be an actual `,` *token* sibling — a comma that is
/// itself an argument (fish's `string join ,`) is a named sibling, so it is
/// ignored and the node is returned unchanged.
fn extend_separator(
    src: &[u8],
    node: tree_sitter::Node,
) -> (std::ops::Range<usize>, (usize, usize), (usize, usize)) {
    let (s, e) = (node.start_byte(), node.end_byte());
    let (start, end) = (point(node.start_position()), point(node.end_position()));
    let is_sep = |n: &tree_sitter::Node| !n.is_named() && n.kind() == ",";

    if let Some(comma) = node.next_sibling().filter(is_sep) {
        // `a, b` on `a`: take the comma and the blanks trailing it.
        let mut i = comma.end_byte();
        while i < src.len() && matches!(src[i], b' ' | b'\t') {
            i += 1;
        }
        return (s..i, start, move_point(src, end, e, i));
    }
    if let Some(comma) = node.prev_sibling().filter(is_sep) {
        // `a, b` on `b` (last item): take the comma and the blanks before it.
        let mut j = comma.start_byte();
        while j > 0 && matches!(src[j - 1], b' ' | b'\t') {
            j -= 1;
        }
        return (j..e, move_point(src, start, s, j), end);
    }
    (s..e, start, end)
}

/// Shrink a `function.inner` / `class.inner` node to the body between its
/// braces, dropping the `{`/`}` and the whitespace just inside them.
///
/// Only applies when `{` and `}` are *direct* children of the node — a
/// genuine brace block, or a `struct {…}` / `enum {…}` header whose node
/// starts at the keyword. A body that merely *contains* a brace expression
/// (Lua's `return { … }`, a Nix lambda returning an attrset) has no direct
/// brace child and is returned unchanged.
fn shrink_braces(
    src: &[u8],
    node: tree_sitter::Node,
) -> (std::ops::Range<usize>, (usize, usize), (usize, usize)) {
    let mut open = None;
    let mut close = None;
    let mut walk = node.walk();
    for child in node.children(&mut walk) {
        if child.is_named() {
            continue;
        }
        match child.kind() {
            "{" if open.is_none() => open = Some(child),
            "}" => close = Some(child),
            _ => {}
        }
    }
    let (Some(open), Some(close)) = (open, close) else {
        return raw_range(node);
    };
    let mut ns = open.end_byte();
    let mut ne = close.start_byte();
    while ns < ne && src[ns].is_ascii_whitespace() {
        ns += 1;
    }
    while ne > ns && src[ne - 1].is_ascii_whitespace() {
        ne -= 1;
    }
    (
        ns..ne,
        move_point(src, point(open.end_position()), open.end_byte(), ns),
        move_point(src, point(close.start_position()), close.start_byte(), ne),
    )
}

/// Recompute the `(row, byte-column)` Point at byte `to`, given the Point
/// `from_pt` known at byte `from`. Walks only the bytes between them (the
/// stripped braces and whitespace — a tiny span), so it stays cheap.
fn move_point(src: &[u8], from_pt: (usize, usize), from: usize, to: usize) -> (usize, usize) {
    if to >= from {
        let (mut row, mut col) = from_pt;
        for &b in &src[from..to] {
            if b == b'\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    } else {
        let mut row = from_pt.0;
        for &b in &src[to..from] {
            if b == b'\n' {
                row -= 1;
            }
        }
        let line_start = src[..to]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
        (row, to - line_start)
    }
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
