//! Pair matching via the tree-sitter syntax tree.
//!
//! Stateless: takes the parsed tree + source and returns the matching
//! position when the cursor sits on one half of a pair. Two flavours:
//!
//! - Asymmetric brackets `()`, `[]`, `{}`, `<>` resolve through the
//!   parent node's children — opener seeks the last same-kind sibling,
//!   closer seeks the first. Tree-sitter naturally excludes brackets
//!   inside strings/comments (they collapse into the containing literal
//!   node) and disambiguates `<`/`>` between generics and comparison
//!   operators (only the former produces sibling tokens of kind `<`/`>`
//!   under the same parent).
//!
//! - Symmetric quotes `"`, `'`, `` ` `` can't disambiguate opener from
//!   closer by kind, so they walk up from the leaf until they find the
//!   smallest ancestor whose text starts and ends with the same quote
//!   character — that ancestor is the string literal, and the cursor
//!   must be sitting on one of its end quotes for the match to apply.
//!   Works across grammars that use same-kind sibling tokens (Rust,
//!   JavaScript) and across grammars that split openers/closers into
//!   distinct kinds (Python's `string_start` / `string_end`).
//!
//! Returns `None` for any character that isn't a recognized pair token
//! at the cursor, or when the parent has no matching counterpart
//! (broken syntax, parse error recovery, prefix-quoted strings like
//! Python f-strings where the leading prefix character breaks the
//! "starts and ends with quote" test).

use tree_sitter::{Point, Tree};

use super::engine::{byte_to_char_col, char_to_byte_col};

/// Find the pair mate of the character at `(row, col_chars)`.
///
/// Returns `Some((row, char_col))` of the matching character when the
/// cursor sits on one of `()`, `[]`, `{}`, `<>`, `"`, `'`, or `` ` ``
/// *as a syntactic token*.
pub(super) fn matching(
    source: &str,
    tree: &Tree,
    row: usize,
    col_chars: usize,
) -> Option<(usize, usize)> {
    let line = source.lines().nth(row)?;
    let byte_col = char_to_byte_col(source, row, col_chars);
    let ch = line.get(byte_col..).and_then(|s| s.chars().next())?;
    let ch_byte_len = ch.len_utf8();
    let start = Point {
        row,
        column: byte_col,
    };
    let end = Point {
        row,
        column: byte_col + ch_byte_len,
    };
    let node = tree.root_node().descendant_for_point_range(start, end)?;

    let cursor = Point {
        row,
        column: byte_col,
    };
    match ch {
        '"' | '\'' | '`' => match_quote(source, node, ch, cursor),
        _ => match_bracket(source, node),
    }
}

fn match_bracket(source: &str, node: tree_sitter::Node) -> Option<(usize, usize)> {
    // Matches on the leaf's `kind`, not on the source character, so
    // comparison `<` / `>` (whose parent is a `binary_expression` with
    // no matching-kind sibling) and `<` / `>` inside generics (whose
    // parent is `type_arguments` / `type_parameters` with a matching
    // sibling) are disambiguated for free.
    let (target, want_last) = match node.kind() {
        "(" => (")", true),
        "[" => ("]", true),
        "{" => ("}", true),
        "<" => (">", true),
        ")" => ("(", false),
        "]" => ("[", false),
        "}" => ("{", false),
        ">" => ("<", false),
        _ => return None,
    };
    let parent = node.parent()?;
    // Opener → matching close is the *last* matching-kind child of the
    // parent (in case of nested same-kind tokens within one parent,
    // which is unusual but cheap to guard against). Closer → first
    // matching-kind child (the opener).
    let mut found: Option<tree_sitter::Node> = None;
    let mut walk = parent.walk();
    for child in parent.children(&mut walk) {
        if child.kind() == target {
            found = Some(child);
            if !want_last {
                break;
            }
        }
    }
    let m = found?;
    let pos = m.start_position();
    Some((pos.row, byte_to_char_col(source, pos.row, pos.column)))
}

fn match_quote(
    source: &str,
    leaf: tree_sitter::Node,
    ch: char,
    cursor: Point,
) -> Option<(usize, usize)> {
    let ch_len = ch.len_utf8();
    let mut node = leaf;
    loop {
        let ns = node.start_byte();
        let ne = node.end_byte();
        // Need at least an opener and a closer to call it a pair.
        if ne >= ns + 2 * ch_len {
            let first = source.get(ns..ne).and_then(|s| s.chars().next());
            let last = source.get(ns..ne).and_then(|s| s.chars().next_back());
            if first == Some(ch) && last == Some(ch) {
                // Compare via tree-sitter `Point`s (row + per-row byte
                // column) rather than absolute byte offsets — the
                // cursor arrives in per-row coordinates and the node's
                // boundary positions do too, so this works regardless
                // of which row in the buffer the string sits on.
                let opener = node.start_position();
                let end = node.end_position();
                let closer = Point {
                    row: end.row,
                    column: end.column.saturating_sub(ch_len),
                };
                let target = if cursor == opener {
                    closer
                } else if cursor == closer {
                    opener
                } else {
                    // Cursor sits on a quote that isn't one of this
                    // node's end quotes — likely an inner quote inside
                    // an escape sequence or an interpolation marker.
                    return None;
                };
                return Some((
                    target.row,
                    byte_to_char_col(source, target.row, target.column),
                ));
            }
        }
        node = node.parent()?;
    }
}
