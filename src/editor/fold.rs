//! View-side fold state and the indentation-based fold fallback.
//!
//! A *foldable region* is a `(header_row, end_row)` pair: the header
//! stays visible (and shows a fold marker) while `header + 1 ..= end`
//! are hidden when the fold is closed. Regions come from a language's
//! `folds.scm` when available; otherwise [`indent_fold_regions`] derives
//! them from indentation so folding works in any buffer.
//!
//! [`FoldState`] records which headers the user has collapsed. It lives
//! on the per-pane [`crate::editor::Editor`] (keyed by `BufferRef`), not
//! on the shared document, because folding is a view concern. State is
//! keyed by header row number, so an edit that inserts or removes lines
//! above a collapsed header can make it drift; the renderer always
//! intersects collapsed headers with freshly computed regions, so a
//! stale entry simply re-opens rather than corrupting the view.

use std::collections::HashSet;

/// The set of fold headers the user has collapsed in one view.
#[derive(Default, Clone)]
pub struct FoldState {
    collapsed: HashSet<usize>,
}

impl FoldState {
    /// Collapse the fold whose header is `row` if open, expand it if
    /// already collapsed.
    pub fn toggle(&mut self, header: usize) {
        if !self.collapsed.remove(&header) {
            self.collapsed.insert(header);
        }
    }

    /// Collapse the fold headed at `row` (idempotent).
    pub fn close(&mut self, header: usize) {
        self.collapsed.insert(header);
    }

    /// Expand the fold headed at `row` (idempotent).
    pub fn open(&mut self, header: usize) {
        self.collapsed.remove(&header);
    }

    /// True when the fold headed at `row` is collapsed.
    pub fn is_collapsed(&self, header: usize) -> bool {
        self.collapsed.contains(&header)
    }

    /// Expand every fold (`zR`).
    pub fn clear(&mut self) {
        self.collapsed.clear();
    }

    /// Collapse every region's header (`zM`).
    pub fn close_all(&mut self, regions: &[(usize, usize)]) {
        self.collapsed.extend(regions.iter().map(|&(h, _)| h));
    }

    /// True when no fold is collapsed — lets callers skip the
    /// hidden-row machinery entirely on the common (all-open) path.
    pub fn is_empty(&self) -> bool {
        self.collapsed.is_empty()
    }

    /// Drop collapsed headers that no longer point at a real line
    /// (e.g. after the buffer shrank on `:reload`).
    pub fn retain_below(&mut self, line_count: usize) {
        self.collapsed.retain(|&h| h < line_count);
    }
}

/// Visual indent width of `line`'s leading whitespace, expanding tabs to
/// the next `tab_width` stop. `None` for a blank (whitespace-only) line,
/// which neither opens nor closes an indent region.
fn indent_width(line: &str, tab_width: usize) -> Option<usize> {
    let mut width = 0;
    for ch in line.chars() {
        match ch {
            ' ' => width += 1,
            '\t' => width += tab_width - (width % tab_width),
            _ => return Some(width),
        }
    }
    None
}

/// Indentation-based foldable regions for buffers with no `folds.scm`.
///
/// A region's header is a line immediately before a deeper-indented run;
/// the region extends through the deeper (and any interleaved blank)
/// lines and closes when indentation returns to the header's level or
/// less. Trailing blank lines are trimmed from `end`. Output is one
/// region per header, sorted by header row (matching
/// [`crate::syntax::engine::Engine::fold_regions`]).
///
/// Single linear pass with an indent stack — O(n) in the number of
/// lines. (Called per frame and per vertical motion while folding is
/// active, so the earlier O(n²) double-scan mattered on large files.)
pub fn indent_fold_regions(lines: &[String], tab_width: usize) -> Vec<(usize, usize)> {
    let tab_width = tab_width.max(1);
    let mut regions = Vec::new();
    // Open headers awaiting a dedent, as `(header_row, header_indent)`,
    // kept in strictly-increasing indent order (a stack).
    let mut stack: Vec<(usize, usize)> = Vec::new();
    // Most recent non-blank line: its row becomes a region's `end` when a
    // dedent closes folds (so trailing blanks are excluded), and its
    // indent is compared against the next non-blank line.
    let mut prev: Option<(usize, usize)> = None;

    for (row, line) in lines.iter().enumerate() {
        let Some(indent) = indent_width(line, tab_width) else {
            continue; // blank line: doesn't open, close, or end a region
        };
        // Close every region whose header is at least as indented as this
        // line — they ended on the previous non-blank row.
        while let Some(&(header, header_indent)) = stack.last() {
            if indent <= header_indent {
                stack.pop();
                let end = prev.map(|(r, _)| r).unwrap_or(header);
                if end > header {
                    regions.push((header, end));
                }
            } else {
                break;
            }
        }
        // An indent increase from the previous non-blank line opens a
        // region headed by that previous line.
        if let Some((prev_row, prev_indent)) = prev
            && indent > prev_indent
        {
            stack.push((prev_row, prev_indent));
        }
        prev = Some((row, indent));
    }
    // Flush headers still open at EOF, ending at the last non-blank row.
    if let Some((last_row, _)) = prev {
        for (header, _) in stack {
            if last_row > header {
                regions.push((header, last_row));
            }
        }
    }

    regions.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    regions.dedup_by_key(|(start, _)| *start);
    regions
}

/// All foldable regions for `buf`: syntax folds when its language ships a
/// `folds.scm`, indentation folds otherwise (and for buffers with no
/// highlighter), with import-run folds merged on top. Normalized — one
/// region per header, sorted by header row.
///
/// Shared by the active path ([`crate::app::App::fold_regions`]) and the
/// inactive-pane renderer so a document folds identically regardless of
/// which pane has focus.
pub fn buffer_fold_regions(buf: &super::Buffer, tab_width: usize) -> Vec<(usize, usize)> {
    let mut regions = match buf.highlighter.as_ref().filter(|e| e.has_fold_query()) {
        Some(engine) => engine.fold_regions(),
        None => indent_fold_regions(&buf.lines, tab_width),
    };
    regions.extend(import_fold_regions(&buf.lines));
    regions.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    regions.dedup_by_key(|(start, _)| *start);
    regions
}

/// True when `trimmed` (leading whitespace already stripped) looks like
/// an import / include statement. A union of common keywords across
/// languages — each prefix ends in a delimiter so identifiers like
/// `use_count` or `importing` don't match.
fn is_import_line(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "import ",
        "from ",
        "use ",
        "pub use ",
        "using ",
        "require ",
        "require(",
        "require_relative ",
        "#include ",
        "#include<",
        "#include\"",
    ];
    trimmed == "import" || PREFIXES.iter().any(|p| trimmed.starts_with(p))
}

/// Net bracket nesting change on `line` — `{`, `[`, `(` open, their
/// mates close. Used to follow a multiline import (e.g. `use a::{ … }`
/// or `from x import ( … )`) across the lines it spans. Angle brackets
/// are intentionally ignored so `#include <h>` doesn't register depth.
fn bracket_delta(line: &str) -> i32 {
    line.chars().fold(0, |d, ch| match ch {
        '{' | '[' | '(' => d + 1,
        '}' | ']' | ')' => d - 1,
        _ => d,
    })
}

/// Foldable regions covering runs of consecutive import statements.
/// Header = the first import line (stays visible); the rest of the run
/// folds under it. Blank lines inside a run are tolerated (and trimmed
/// from the end), so a std/third-party block separated by a blank still
/// folds as one. A multiline import — `use a::{` opening a brace list
/// that closes lines later — keeps the run alive through its
/// continuation lines via bracket-depth tracking. Single-line imports
/// don't fold on their own (`end > start` required).
///
/// Language-agnostic by design: it works with no grammar and complements
/// [`indent_fold_regions`], since top-level imports have no deeper indent
/// to fold on. Go-style `import ( … )` blocks already fold via
/// indentation, so the heuristic just adds the flat case.
pub fn import_fold_regions(lines: &[String]) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut last_import = 0usize;
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate() {
        if depth > 0 {
            // Inside an unterminated multiline import: every line belongs
            // to the run until the brackets balance again.
            if !line.trim().is_empty() {
                last_import = i;
            }
            depth = (depth + bracket_delta(line)).max(0);
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Blank: keep an open run alive but don't extend its end.
            continue;
        }
        if is_import_line(line.trim_start()) {
            run_start.get_or_insert(i);
            last_import = i;
            depth = (depth + bracket_delta(line)).max(0);
        } else if let Some(start) = run_start.take()
            && last_import > start
        {
            regions.push((start, last_import));
        }
    }
    if let Some(start) = run_start
        && last_import > start
    {
        regions.push((start, last_import));
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &[&str]) -> Vec<String> {
        src.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn folds_a_simple_block() {
        let src = lines(&["fn main() {", "    let x = 1;", "    let y = 2;", "}"]);
        // Header row 0, body rows 1..=2 deeper; row 3 dedents to base.
        assert_eq!(indent_fold_regions(&src, 4), vec![(0, 2)]);
    }

    #[test]
    fn trims_trailing_blank_lines() {
        let src = lines(&["if a:", "    b", "", "c"]);
        // Blank row 2 isn't part of the fold end; row 3 is at base.
        assert_eq!(indent_fold_regions(&src, 4), vec![(0, 1)]);
    }

    #[test]
    fn nested_regions_both_reported() {
        let src = lines(&[
            "a:",    // 0
            "  b:",  // 1
            "    c", // 2
            "    d", // 3
            "  e",   // 4
        ]);
        let got = indent_fold_regions(&src, 2);
        assert_eq!(got, vec![(0, 4), (1, 3)]);
    }

    #[test]
    fn single_lines_dont_fold() {
        let src = lines(&["a", "b", "c"]);
        assert!(indent_fold_regions(&src, 4).is_empty());
    }

    #[test]
    fn staircase_indent_nests_correctly() {
        // Each line deeper than the last, then a full dedent — exercises
        // the stack flush and would have been the O(n^2) worst case.
        let src = lines(&["a", "  b", "    c", "      d", "e"]);
        // Headers 0,1,2 each fold down to row 3 (the deepest line); row 4
        // dedents to base and closes them.
        assert_eq!(indent_fold_regions(&src, 2), vec![(0, 3), (1, 3), (2, 3)]);
    }

    #[test]
    fn buffer_fold_regions_merges_indent_and_imports() {
        let buf = crate::editor::Buffer {
            lines: lines(&[
                "use a;",    // 0  import run header
                "use b;",    // 1
                "",          // 2
                "fn f() {",  // 3  indent header
                "    body;", // 4
                "}",         // 5
            ]),
            ..Default::default()
        };
        // No highlighter → indent fallback (3,4) plus import run (0,1).
        assert_eq!(buffer_fold_regions(&buf, 4), vec![(0, 1), (3, 4)]);
    }

    #[test]
    fn fold_state_toggle() {
        let mut s = FoldState::default();
        s.toggle(5);
        assert!(s.is_collapsed(5));
        s.toggle(5);
        assert!(!s.is_collapsed(5));
    }

    #[test]
    fn folds_a_run_of_imports() {
        let src = lines(&["use a;", "use b;", "use c;", "", "fn main() {}"]);
        // Rows 0..=2 are the import run; the blank and fn are excluded.
        assert_eq!(import_fold_regions(&src), vec![(0, 2)]);
    }

    #[test]
    fn import_run_spans_internal_blank_but_trims_trailing() {
        let src = lines(&[
            "import os",       // 0
            "import sys",      // 1
            "",                // 2 blank inside the block
            "from a import b", // 3
            "",                // 4 trailing blank — not folded
            "x = 1",           // 5
        ]);
        assert_eq!(import_fold_regions(&src), vec![(0, 3)]);
    }

    #[test]
    fn single_import_does_not_fold() {
        let src = lines(&["import os", "x = 1"]);
        assert!(import_fold_regions(&src).is_empty());
    }

    #[test]
    fn multiline_use_keeps_the_run_intact() {
        // The continuation lines of a `use a::{ … }` don't start with an
        // import keyword, but bracket depth keeps them inside the run so
        // the whole block folds as one (rows 0..=15).
        let src = lines(&[
            "use std::io::{self, Stdout, Write};",        // 0
            "use std::sync::mpsc;",                       // 1
            "use std::thread;",                           // 2
            "",                                           // 3
            "use anyhow::Result;",                        // 4
            "use crossterm::event::{",                    // 5  opens
            "    self as crossterm_event, Event,",        // 6
            "    PushKeyboardEnhancementFlags,",          // 7
            "};",                                         // 8  closes
            "use crossterm::execute;",                    // 9
            "use crossterm::terminal::{",                 // 10 opens
            "    EnterAlternateScreen, enable_raw_mode,", // 11
            "};",                                         // 12 closes
            "use ratatui::Terminal;",                     // 13
        ]);
        assert_eq!(import_fold_regions(&src), vec![(0, 13)]);
    }

    #[test]
    fn multiline_python_from_import_parens() {
        let src = lines(&[
            "from foo import (", // 0 opens with paren
            "    a,",            // 1
            "    b,",            // 2
            ")",                 // 3 closes
            "x = 1",             // 4
        ]);
        // The whole parenthesized import is one run (rows 0..=3).
        assert_eq!(import_fold_regions(&src), vec![(0, 3)]);
    }

    #[test]
    fn identifiers_starting_like_keywords_are_ignored() {
        // `use_count` / `importing` must not be treated as imports.
        let src = lines(&["use_count = 1", "importing = true", "from_date = x"]);
        assert!(import_fold_regions(&src).is_empty());
    }
}
