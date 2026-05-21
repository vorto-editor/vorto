//! Bracket-pair matching via the tree-sitter syntax tree.
//!
//! Stateless: takes the parsed tree + source and returns the matching
//! bracket's position when the cursor sits on one. Brackets inside
//! strings and comments are naturally excluded because tree-sitter
//! resolves them as part of the enclosing literal node rather than as
//! standalone bracket tokens.

use tree_sitter::{Point, Tree};

use super::engine::{byte_to_char_col, char_to_byte_col};

/// Find the bracket-pair mate of the character at `(row, col_chars)`.
///
/// Returns `Some((row, char_col))` of the matching bracket when the
/// cursor sits on one of `()[]{}` *as a syntactic token*. Returns
/// `None` when the character is not a bracket token, or when the
/// parent node has no matching counterpart (broken syntax, parse
/// error recovery).
pub(super) fn matching(
    source: &str,
    tree: &Tree,
    row: usize,
    col_chars: usize,
) -> Option<(usize, usize)> {
    let line = source.lines().nth(row)?;
    let byte_col = char_to_byte_col(source, row, col_chars);
    let ch_byte_len = line
        .get(byte_col..)
        .and_then(|s| s.chars().next())
        .map(char::len_utf8)?;
    let start = Point {
        row,
        column: byte_col,
    };
    let end = Point {
        row,
        column: byte_col + ch_byte_len,
    };
    let node = tree.root_node().descendant_for_point_range(start, end)?;
    let (target, want_last) = match node.kind() {
        "(" => (")", true),
        "[" => ("]", true),
        "{" => ("}", true),
        ")" => ("(", false),
        "]" => ("[", false),
        "}" => ("{", false),
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
