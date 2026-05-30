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

use std::cell::RefCell;

use anyhow::{Context, Result};
use tree_sitter::{InputEdit, Language, Parser, Point, Tree};

use super::fold::FoldQuery;
use super::highlight::{Capture, HighlightQuery};
use super::indent::IndentQuery;
use super::injection::InjectionEngine;
use super::textobject::TextObjectQuery;
use super::{bracket, fold, highlight, indent, textobject};

/// Per-buffer tree-sitter state. Owns the parser, the last-parsed
/// tree, and the per-concern query objects. Refreshes the tree only
/// when [`Self::refresh`] is called with a version newer than the one
/// already cached, so callers can poke at it freely from a hot draw
/// loop.
pub struct Engine {
    parser: Parser,
    tree: Option<Tree>,
    source: String,
    /// Byte offset of the start of each line in `source`, plus a
    /// trailing sentinel of `source.len()`. Rebuilt once per
    /// [`Self::refresh`] so the per-capture byte→char column conversion
    /// is an O(1) row lookup instead of an O(row) `lines().nth(row)`
    /// scan — otherwise highlight cost scales with the cursor's
    /// absolute row, not the visible window.
    line_starts: Vec<usize>,
    parsed_version: Option<u64>,
    highlight: HighlightQuery,
    indent: Option<IndentQuery>,
    textobject: Option<TextObjectQuery>,
    fold: Option<FoldQuery>,
    injection: Option<InjectionEngine>,
    /// Memoized result of the last [`Self::captures_in_rows`] call.
    /// Keyed on `(parsed_version, start_row, end_row)`: the draw loop
    /// re-queries on every frame, but pure cursor moves, mode changes,
    /// and toast-TTL redraws leave all three unchanged — so we return
    /// the cached spans instead of re-walking the tree (and, with an
    /// injection engine, re-parsing every embedded region). Invalidated
    /// wholesale whenever [`Self::refresh`] reparses.
    capture_cache: RefCell<Option<CaptureCache>>,
    /// Memoized fold regions keyed on `parsed_version`. The draw loop
    /// asks for these every frame; they only change when the tree
    /// reparses, so the version gate keeps the full-tree fold walk off
    /// the hot path. Invalidated wholesale by [`Self::refresh`].
    fold_cache: RefCell<FoldRegionsCache>,
    /// Non-fatal warnings collected during construction (e.g. an
    /// `indents.scm` that failed to compile). The TUI drains these
    /// into toasts; writing to stderr from a worker thread would
    /// corrupt the alt-screen display.
    pub warnings: Vec<String>,
}

/// Memoized fold regions tagged with the `parsed_version` they were
/// computed for. `(version, regions)` — see [`Engine::fold_regions`].
type FoldRegionsCache = Option<(Option<u64>, Vec<(usize, usize)>)>;

/// Cached `captures_in_rows` output plus the key it was computed for.
struct CaptureCache {
    version: Option<u64>,
    start_row: usize,
    end_row: usize,
    captures: Vec<Capture>,
}

impl Engine {
    pub(super) fn new(
        language: Language,
        highlights_src: &str,
        textobjects_src: Option<&str>,
        indents_src: Option<&str>,
        folds_src: Option<&str>,
        injection: Option<InjectionEngine>,
    ) -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .context("setting parser language (ABI mismatch?)")?;
        let highlight = highlight::HighlightQuery::compile(&language, highlights_src)
            .context("compiling highlights query")?;

        // textobjects.scm / indents.scm failures are non-fatal: a bad
        // node name there just disables the corresponding feature
        // (text-object resolution / auto-indent) — we still want
        // highlighting to work.
        let mut warnings = Vec::new();
        let textobject = match textobjects_src {
            Some(src) => match textobject::TextObjectQuery::compile(&language, src) {
                Ok(q) => Some(q),
                Err(e) => {
                    warnings.push(format!(
                        "textobjects.scm compile failed, syntactic text objects disabled: {e}"
                    ));
                    None
                }
            },
            None => None,
        };

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

        let fold = match folds_src {
            Some(src) => match fold::FoldQuery::compile(&language, src) {
                Ok(q) => Some(q),
                Err(e) => {
                    warnings.push(format!(
                        "folds.scm compile failed, syntax folding disabled: {e}"
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
            line_starts: vec![0],
            parsed_version: None,
            highlight,
            indent,
            textobject,
            fold,
            injection,
            capture_cache: RefCell::new(None),
            fold_cache: RefCell::new(None),
            warnings,
        })
    }

    /// True when the cached tree already reflects `version`, so a
    /// caller can skip rebuilding the source snapshot and calling
    /// [`Self::refresh`]. Lets the per-frame `refresh_highlights` avoid
    /// a full-document `lines.join` when nothing changed.
    pub fn is_current(&self, version: u64) -> bool {
        self.parsed_version == Some(version)
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
        self.line_starts = line_start_offsets(&self.source);
        self.parsed_version = Some(version);
        // The tree changed; any memoized capture window is now stale.
        self.capture_cache.borrow_mut().take();
        self.fold_cache.borrow_mut().take();
    }

    /// All highlight captures intersecting rows `[start_row..=end_row]`.
    /// Includes captures contributed by embedded sub-languages when an
    /// injection engine is configured. Columns are returned in
    /// characters (not bytes).
    pub fn captures_in_rows(&self, start_row: usize, end_row: usize) -> Vec<Capture> {
        if let Some(c) = self.capture_cache.borrow().as_ref()
            && c.version == self.parsed_version
            && c.start_row == start_row
            && c.end_row == end_row
        {
            return c.captures.clone();
        }
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let mut out = self.highlight.captures_in_rows(
            &self.source,
            &self.line_starts,
            tree,
            start_row,
            end_row,
        );
        // Layer in sub-language captures *after* the host's so the
        // renderer's patch-merge stacks sub-language fg/bold on top of
        // any host capture covering the same region. The subsequent
        // stable sort keeps everything in document order while
        // preserving intra-row priority.
        if let Some(inj) = self.injection.as_ref() {
            out.extend(inj.captures_in_rows(
                &self.source,
                &self.line_starts,
                tree,
                start_row,
                end_row,
            ));
        }
        out.sort_by_key(|c| (c.start_row, c.start_col));
        *self.capture_cache.borrow_mut() = Some(CaptureCache {
            version: self.parsed_version,
            start_row,
            end_row,
            captures: out.clone(),
        });
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

    /// True when this engine has a compiled `folds.scm`. The editor
    /// uses syntax folds when this holds and an indentation-based
    /// fallback otherwise.
    pub fn has_fold_query(&self) -> bool {
        self.fold.is_some()
    }

    /// Normalized foldable regions `(header_row, end_row)` for the whole
    /// document — one per header row, sorted by `header_row` ascending.
    /// Returns the full set (not windowed): the caller needs it all to
    /// compute hidden rows and to find the region under the cursor.
    /// Empty when there's no fold query or no parsed tree. Memoized on
    /// `parsed_version`.
    pub fn fold_regions(&self) -> Vec<(usize, usize)> {
        if let Some((v, r)) = self.fold_cache.borrow().as_ref()
            && *v == self.parsed_version
        {
            return r.clone();
        }
        let regions = match (&self.tree, &self.fold) {
            (Some(tree), Some(q)) => fold::normalize_regions(q.regions(&self.source, tree)),
            _ => Vec::new(),
        };
        *self.fold_cache.borrow_mut() = Some((self.parsed_version, regions.clone()));
        regions
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

    /// Every range matching `target` in the buffer (not cursor-relative).
    /// Empty when no tree is parsed or the language has no
    /// `textobjects.scm`. Used by the grammar golden tests to snapshot
    /// all text objects of a kind.
    #[cfg(test)]
    pub(crate) fn all_text_objects(&self, target: &str) -> Vec<(usize, usize, usize, usize)> {
        let (Some(tree), Some(q)) = (self.tree.as_ref(), self.textobject.as_ref()) else {
            return Vec::new();
        };
        q.all(&self.source, tree, target)
    }

    /// Pair mate of the character at `(row, col_chars)` when the cursor
    /// sits on a syntactic bracket (`()`, `[]`, `{}`, `<>`) or quote
    /// (`"`, `'`, `` ` ``). Tree-sitter resolves brackets inside
    /// strings/comments to the enclosing literal so they don't match,
    /// and disambiguates `<`/`>` between generics and comparison
    /// operators by parent kind.
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

/// Byte offset where each line of `source` begins, terminated by a
/// `source.len()` sentinel so the end of the last line is always
/// `line_starts[row + 1]`. A single linear pass; cached on the
/// [`Engine`] and rebuilt only when the source changes.
fn line_start_offsets(source: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(source.len() / 32 + 2);
    starts.push(0);
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts.push(source.len());
    starts
}

/// [`byte_to_char_col`] using a precomputed [`line_start_offsets`]
/// table: an O(1) row lookup plus an O(column) char count, instead of
/// the O(row) `lines().nth(row)` scan. Called once per highlight
/// capture, so the difference is what keeps per-frame cost tied to the
/// visible window rather than the cursor's absolute row.
pub(super) fn byte_to_char_col_indexed(
    source: &str,
    line_starts: &[usize],
    row: usize,
    byte_col: usize,
) -> usize {
    let Some(&start) = line_starts.get(row) else {
        return 0;
    };
    let end = line_starts.get(row + 1).copied().unwrap_or(source.len());
    let line = &source[start..end.min(source.len())];
    let take = byte_col.min(line.len());
    match line.get(..take) {
        Some(prefix) => prefix.chars().count(),
        // `take` landed mid-codepoint (shouldn't happen for tree-sitter
        // node columns, but stay total): fall back to the whole line.
        None => line.chars().count(),
    }
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

    // ───────────────────────────────────────────────────────────────
    // Ad-hoc performance probes. Ignored by default (they depend on
    // grammars installed under ~/.config/vorto and are timing-based,
    // not assertions). Run with:
    //   cargo test --release perf_ -- --ignored --nocapture
    // ───────────────────────────────────────────────────────────────

    use crate::config::Config;
    use crate::syntax::Loader;
    use std::path::Path;
    use std::time::Instant;

    // Returns the `Loader` alongside the `Engine` so the caller keeps
    // it alive: the engine's `Language` is a pointer into a dylib the
    // loader owns, so dropping the loader early dangles it (SIGSEGV).
    fn engine_for_path(sample: &str, source: &str) -> Option<(Loader, Engine)> {
        let cfg = Config::load(None).ok()?;
        let spec = cfg.languages.by_path(Path::new(sample))?.clone();
        let mut loader = Loader::new(cfg.grammar_dir.clone(), cfg.query_dir.clone());
        let mut engine = loader.engine_for(&spec).ok()?;
        engine.refresh(source, 1);
        Some((loader, engine))
    }

    fn median_us(mut samples: Vec<f64>) -> f64 {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        samples[samples.len() / 2]
    }

    /// Per-frame highlight cost for a 50-row viewport should be roughly
    /// independent of total file size (the query is range-limited), and
    /// a repeated identical window should be nearly free (memoized).
    #[test]
    #[ignore]
    fn perf_highlight_scaling() {
        let unit = "fn compute(x: i32) -> i32 {\n    let y = x * 2 + 1;\n    y - 3\n}\n\n";
        const WINDOW: usize = 50;

        for lines_target in [2_000usize, 20_000, 100_000] {
            let reps = lines_target / 5 + 1;
            let source = unit.repeat(reps);
            let total_rows = source.lines().count();
            let Some((_loader, engine)) = engine_for_path("bench.rs", &source) else {
                eprintln!("skip: rust grammar not installed");
                return;
            };

            // Cache MISS each frame: move the window so (start,end) changes,
            // simulating scrolling / editing through a large file.
            let mut miss = Vec::new();
            for i in 0..400 {
                let scroll = (i * 37) % total_rows.saturating_sub(WINDOW).max(1);
                let t = Instant::now();
                let caps = engine.captures_in_rows(scroll, scroll + WINDOW);
                miss.push(t.elapsed().as_secs_f64() * 1e6);
                std::hint::black_box(caps);
            }

            // Cache HIT: same window repeatedly (cursor moving in place,
            // toast redraws, mode changes).
            let mut hit = Vec::new();
            let scroll = total_rows / 2;
            let _ = engine.captures_in_rows(scroll, scroll + WINDOW); // prime
            for _ in 0..2_000 {
                let t = Instant::now();
                let caps = engine.captures_in_rows(scroll, scroll + WINDOW);
                hit.push(t.elapsed().as_secs_f64() * 1e6);
                std::hint::black_box(caps);
            }

            eprintln!(
                "rust {:>7} rows | miss(scroll/edit) median {:>7.1} us | hit(repaint) median {:>6.2} us",
                total_rows,
                median_us(miss),
                median_us(hit),
            );
        }
    }

    /// Injection path: a markdown file full of fenced code blocks. With
    /// the sub-tree cache, re-querying a fixed window across version
    /// bumps (typing in prose) reuses each visible block's parse.
    #[test]
    #[ignore]
    fn perf_injection_markdown() {
        const WINDOW: usize = 50;
        let block = "```rust\nfn demo(n: usize) -> usize {\n    let mut acc = 0;\n    for i in 0..n { acc += i * 2; }\n    acc\n}\n```\n\nSome prose paragraph between code blocks to mimic a real doc.\n\n";
        let source = block.repeat(400);
        let total_rows = source.lines().count();
        let Some((_loader, mut engine)) = engine_for_path("bench.md", &source) else {
            eprintln!("skip: markdown grammar not installed");
            return;
        };

        let scroll = total_rows / 2;
        // First touch parses the visible blocks (cold injection cache).
        let t = Instant::now();
        let _ = engine.captures_in_rows(scroll, scroll + WINDOW);
        let cold = t.elapsed().as_secs_f64() * 1e6;

        // Simulate typing: bump the version each frame (clears the
        // capture memo, forcing the injection walk) but leave the
        // visible code blocks' text unchanged so the sub-tree cache
        // hits. Median is the steady-state per-keystroke cost.
        let mut warm = Vec::new();
        for v in 2..402u64 {
            engine.refresh(&source, v);
            let t = Instant::now();
            let caps = engine.captures_in_rows(scroll, scroll + WINDOW);
            warm.push(t.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(caps);
        }

        eprintln!(
            "markdown {} rows | cold(first paint) {:.1} us | warm(typing, sub-tree cache) median {:.1} us",
            total_rows,
            cold,
            median_us(warm),
        );
    }
}
