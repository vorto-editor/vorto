//! Fold-region extraction from a `folds.scm` query.
//!
//! [`FoldQuery`] reports the line ranges of every `@fold`-captured node in
//! the tree. The editor turns those into collapsible regions: the
//! `header` row (where the node starts) stays visible and carries a fold
//! marker, while `header + 1 ..= end` are the rows hidden when the fold is
//! closed. Languages without a `folds.scm` fall back to indentation-based
//! regions (see [`crate::editor::fold::indent_fold_regions`]).

use anyhow::Result;
use tree_sitter::{Language, Query, QueryCursor, StreamingIterator, Tree};

/// Compiled `folds.scm` query for one language.
pub struct FoldQuery {
    query: Query,
    /// Indices of captures named `fold` — the only capture this query
    /// cares about. Almost always a single index, so a small `Vec` with a
    /// linear membership check is cheaper than a hash set and lets the
    /// match loop skip re-resolving capture names by string.
    fold_capture_idx: Vec<u32>,
}

impl FoldQuery {
    pub(super) fn compile(language: &Language, src: &str) -> Result<Self> {
        let query = Query::new(language, src)?;
        let fold_capture_idx = query
            .capture_names()
            .iter()
            .enumerate()
            .filter(|(_, name)| **name == "fold")
            .map(|(i, _)| i as u32)
            .collect();
        Ok(Self {
            query,
            fold_capture_idx,
        })
    }

    /// Raw foldable regions `(header_row, end_row)` for every `@fold`
    /// node spanning at least two rows. Not deduplicated or sorted —
    /// [`normalize_regions`] does that.
    pub(super) fn regions(&self, source: &str, tree: &Tree) -> Vec<(usize, usize)> {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        let mut out = Vec::new();
        while let Some(m) = matches.next() {
            for cap in m.captures {
                if !self.fold_capture_idx.contains(&cap.index) {
                    continue;
                }
                let start = cap.node.start_position().row;
                let end = cap.node.end_position().row;
                if end > start {
                    out.push((start, end));
                }
            }
        }
        out
    }
}

/// Collapse a raw region list into one fold per header row.
///
/// Tree-sitter folds are always properly nested (never partially
/// overlapping), so the only ambiguity is several `@fold` nodes that
/// *start* on the same line — e.g. an `if` whose body `block` opens on
/// the same row. We keep the one with the largest `end_row` so closing
/// that header hides the maximal span. Nested folds on *different*
/// header rows are preserved (vim supports nested folds). Output is
/// sorted by `start_row` ascending, `end_row` descending.
pub fn normalize_regions(mut regions: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if regions.is_empty() {
        return regions;
    }
    // Sort so that, for a given start, the largest end comes first.
    regions.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    regions.dedup_by_key(|(start, _)| *start);
    regions
}

#[cfg(test)]
mod tests {
    use super::normalize_regions;

    #[test]
    fn keeps_largest_end_for_shared_header() {
        let got = normalize_regions(vec![(0, 3), (0, 7), (0, 5), (10, 12)]);
        assert_eq!(got, vec![(0, 7), (10, 12)]);
    }

    #[test]
    fn sorts_by_start_then_keeps_nested() {
        // Nested on different headers are both kept.
        let got = normalize_regions(vec![(5, 9), (1, 20), (6, 8)]);
        assert_eq!(got, vec![(1, 20), (5, 9), (6, 8)]);
    }

    #[test]
    fn empty_stays_empty() {
        assert_eq!(normalize_regions(vec![]), Vec::<(usize, usize)>::new());
    }
}
