//! Language injection: pulling sub-language highlighters into a host
//! file's capture stream.
//!
//! Vue, Svelte, Markdown, and similar formats embed other languages
//! inside themselves — `<script lang="ts">…</script>`, fenced code
//! blocks, `style` attribute values, etc. Tree-sitter exposes those
//! regions through an `injections.scm` query that pins each region
//! with `@injection.content` and a language hint (statically via
//! `(#set! injection.language "X")` or dynamically via an
//! `@injection.language` capture).
//!
//! This module implements the static half — `(#set! injection.language
//! "X")` patterns only. Dynamic language captures are intentionally
//! out of scope for now; they require predicate-time language
//! resolution and a deferred grammar load that the loader doesn't have
//! plumbing for yet.
//!
//! ## Construction
//!
//! [`InjectionEngine::build`] compiles `injections.scm`, scans each
//! pattern for a `#set! injection.language` property, and asks the
//! loader to construct a [`SubHighlighter`] for every referenced
//! language. A missing grammar or query is non-fatal — the matching
//! pattern is dropped from the engine, the rest still works.
//!
//! ## Runtime
//!
//! [`InjectionEngine::captures_in_rows`] walks the host tree's
//! injection matches, parses each `@injection.content` region with
//! the relevant sub-language, runs that language's highlights query
//! against the sub-tree, and translates the sub-captures back into
//! host-source row/column coordinates.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use tree_sitter::{Language, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use super::highlight::Capture;
use super::loader::Loader;
use crate::vlog;

/// Pre-loaded child highlighter for one injected language. Owns the
/// grammar handle and the compiled `highlights.scm` query; the parser
/// is constructed fresh per match to keep [`InjectionEngine`] usable
/// behind a `&` borrow.
pub struct SubHighlighter {
    language: Language,
    query: Query,
    capture_names: Vec<String>,
    /// Optional `indents.scm` query for the sub-language. When present,
    /// the injection engine can answer indent-scope queries about the
    /// embedded region — so e.g. a `function` inside `<script
    /// lang="ts">` shows the same active scope bracket it would in a
    /// standalone `.ts` buffer. Missing is non-fatal; we just skip
    /// indent-scope contributions for that sub-language.
    indents: Option<Query>,
    indent_capture_names: Vec<String>,
}

impl SubHighlighter {
    pub(super) fn new(
        language: Language,
        query: Query,
        capture_names: Vec<String>,
        indents: Option<Query>,
        indent_capture_names: Vec<String>,
    ) -> Self {
        Self {
            language,
            query,
            capture_names,
            indents,
            indent_capture_names,
        }
    }
}

/// Static injection engine. Pre-resolves every `(#set!
/// injection.language "X")` pattern to a `SubHighlighter` at
/// construction; at runtime each match runs the resolved
/// sub-language's highlights query and re-bases the captures into
/// host coordinates.
pub struct InjectionEngine {
    query: Query,
    /// One slot per pattern in `query`, populated only for patterns
    /// that declare a static `#set! injection.language` value *and*
    /// whose sub-language could be resolved at build time. `None`
    /// means the pattern is inert.
    pattern_lang: Vec<Option<String>>,
    /// Capture index for `@injection.content` in the injections query.
    /// `None` means the query is missing the capture entirely (no
    /// usable injections), and runtime calls are early-returned.
    content_idx: Option<u32>,
    /// Pre-loaded sub-language highlighters keyed by the language
    /// name as it appears in `#set! injection.language`.
    subs: HashMap<String, SubHighlighter>,
    /// Parsed sub-trees keyed by the exact `@injection.content` text,
    /// with the language they were parsed as stored alongside so two
    /// regions sharing identical text but different languages (e.g. a
    /// markdown ` ```rust ` and ` ```python ` block both holding
    /// `x = 1`) can't return each other's tree.
    ///
    /// Re-parsing every embedded region from scratch on each frame was
    /// the injection path's dominant cost (a markdown code block or
    /// `<script>` body re-parsed per keystroke / redraw). Keying on the
    /// region *content* rather than its byte offset means edits in
    /// host prose that merely shift a region's position still hit the
    /// cache. Pruned each call down to the regions actually visited so
    /// it stays bounded by what's on screen.
    sub_tree_cache: RefCell<HashMap<String, (String, Tree)>>,
}

impl InjectionEngine {
    /// Build an engine from an `injections.scm` source. Returns
    /// `Ok(None)` when the query compiles but contains no usable
    /// static injections — callers should treat that the same as
    /// "no injections.scm shipped." Returns `Err` only on a hard
    /// query compile failure.
    pub(super) fn build(
        host_language: &Language,
        injections_src: &str,
        loader: &mut Loader,
    ) -> Result<Option<Self>> {
        let query =
            Query::new(host_language, injections_src).context("compiling injections query")?;
        let content_idx = query
            .capture_names()
            .iter()
            .position(|n| *n == "injection.content")
            .map(|i| i as u32);

        // Walk every pattern, pick up its `#set! injection.language`
        // setting if present. The tree-sitter API exposes settings
        // as a flat `[(key, Option<value>)]` array per pattern.
        let mut pattern_lang: Vec<Option<String>> = Vec::with_capacity(query.pattern_count());
        let mut needed: Vec<String> = Vec::new();
        for pat in 0..query.pattern_count() {
            let lang_name = query.property_settings(pat).iter().find_map(|setting| {
                if setting.key.as_ref() == "injection.language" {
                    setting.value.as_ref().map(|v| v.to_string())
                } else {
                    None
                }
            });
            let unseen = lang_name
                .as_deref()
                .filter(|n| !needed.iter().any(|x| x == n));
            if let Some(name) = unseen {
                needed.push(name.to_string());
            }
            pattern_lang.push(lang_name);
        }

        let mut subs = HashMap::new();
        for name in &needed {
            match loader.sub_highlighter_for(name) {
                Ok(sub) => {
                    subs.insert(name.clone(), sub);
                }
                Err(e) => {
                    // A missing grammar is a soft failure — the host
                    // language still highlights, just without that
                    // injection. Route through the debug log so the
                    // TUI (and the fuzzy-finder preview, in particular)
                    // never gets stderr scribbled over it.
                    vlog!("injection: sub-language `{}` unavailable: {:#}", name, e);
                }
            }
        }

        // Drop patterns whose sub-language never loaded so the runtime
        // loop doesn't have to repeat the check on every match.
        for slot in pattern_lang.iter_mut() {
            if let Some(name) = slot.as_deref()
                && !subs.contains_key(name)
            {
                *slot = None;
            }
        }

        if subs.is_empty() || content_idx.is_none() {
            return Ok(None);
        }
        Ok(Some(Self {
            query,
            pattern_lang,
            content_idx,
            subs,
            sub_tree_cache: RefCell::new(HashMap::new()),
        }))
    }

    /// Run the injections query against `tree`, parse each
    /// `@injection.content` region with the matching sub-language,
    /// and return the sub-captures translated into host-source
    /// coordinates. Captures whose entire range falls outside
    /// `[start_row, end_row]` are filtered before parsing so we don't
    /// pay the cost on off-screen regions.
    pub(super) fn captures_in_rows(
        &self,
        host_source: &str,
        host_line_starts: &[usize],
        tree: &Tree,
        start_row: usize,
        end_row: usize,
    ) -> Vec<Capture> {
        let Some(content_idx) = self.content_idx else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut cursor = QueryCursor::new();
        // Only walk injection matches overlapping the visible window;
        // off-screen regions are skipped before we ever pay to parse
        // them (mirrors the range limiting on the host highlight query).
        cursor.set_point_range(
            Point {
                row: start_row,
                column: 0,
            }..Point {
                row: end_row.saturating_add(1),
                column: 0,
            },
        );
        let host_bytes = host_source.as_bytes();
        let mut matches = cursor.matches(&self.query, tree.root_node(), host_bytes);
        let mut cache = self.sub_tree_cache.borrow_mut();
        // Region contents touched this pass; everything else is evicted
        // afterwards so the cache tracks the on-screen working set.
        let mut seen: HashSet<&str> = HashSet::new();
        // Scratch state for `#eq?` / `#match?` gating below.
        let mut text_provider = host_bytes;
        let mut pred_buf1 = Vec::new();
        let mut pred_buf2 = Vec::new();
        while let Some(m) = matches.next() {
            // Honor text predicates that scope an injection to a subset of
            // matched nodes — e.g. `#eq? @_name "style"` restricts the CSS
            // injection to `<style>` elements. Without this every matching
            // element would inject every language, which is both wrong and
            // expensive. `#set! injection.language` is read at build time;
            // this gates *whether* a given match actually injects.
            if !m.satisfies_text_predicates(
                &self.query,
                &mut pred_buf1,
                &mut pred_buf2,
                &mut text_provider,
            ) {
                continue;
            }
            let Some(lang_name) = self
                .pattern_lang
                .get(m.pattern_index)
                .and_then(Option::as_deref)
            else {
                continue;
            };
            let Some(sub) = self.subs.get(lang_name) else {
                continue;
            };
            for cap in m.captures {
                if cap.index != content_idx {
                    continue;
                }
                let node = cap.node;
                let span_start_row = node.start_position().row;
                let span_end_row = node.end_position().row;
                if span_end_row < start_row || span_start_row > end_row {
                    continue;
                }
                let start_byte = node.start_byte();
                let end_byte = node.end_byte().min(host_bytes.len());
                if end_byte <= start_byte {
                    continue;
                }
                let Ok(slice) = std::str::from_utf8(&host_bytes[start_byte..end_byte]) else {
                    continue;
                };
                seen.insert(slice);
                // Reuse the sub-tree when this exact region text was
                // already parsed (cheap `Tree` clone — it's internally
                // reference-counted). `Tree` carries no borrow of the
                // source it was parsed from, so a cached tree stays
                // valid across frames.
                let sub_tree = match cache.get(slice) {
                    Some((cached_lang, t)) if cached_lang == lang_name => t.clone(),
                    _ => {
                        let mut parser = Parser::new();
                        if parser.set_language(&sub.language).is_err() {
                            continue;
                        }
                        let Some(t) = parser.parse(slice, None) else {
                            continue;
                        };
                        cache.insert(slice.to_string(), (lang_name.to_string(), t.clone()));
                        t
                    }
                };

                // Sub-tree positions are relative to `slice`. The
                // host's start position gives us the offset to add
                // back. Within a slice that doesn't begin at column 0,
                // tree-sitter still numbers rows from 0 — so row 0 of
                // the slice maps to the host's `span_start_row` and
                // its column is offset by the host's start column.
                let host_start_pt = node.start_position();
                self.collect_sub_captures(
                    sub,
                    &sub_tree,
                    slice,
                    host_source,
                    host_line_starts,
                    host_start_pt.row,
                    host_start_pt.column,
                    start_row,
                    end_row,
                    &mut out,
                );
            }
        }
        cache.retain(|k, _| seen.contains(k.as_str()));
        out
    }

    /// Indent scopes contributed by sub-languages embedded in the host
    /// tree. Mirrors [`super::highlight::Highlighter::indent_scopes_in_rows`]
    /// but defers per-region work to the sub-language's `indents.scm`.
    /// Each returned `(start_row, end_row)` is already translated into
    /// host-source row coordinates and includes only scopes whose body
    /// spans more than one row — the renderer drops same-row scopes.
    pub(super) fn indent_scopes_in_rows(
        &self,
        host_source: &str,
        tree: &Tree,
        start_row: usize,
        end_row: usize,
    ) -> Vec<(usize, usize)> {
        let Some(content_idx) = self.content_idx else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_point_range(
            Point {
                row: start_row,
                column: 0,
            }..Point {
                row: end_row.saturating_add(1),
                column: 0,
            },
        );
        let host_bytes = host_source.as_bytes();
        let mut matches = cursor.matches(&self.query, tree.root_node(), host_bytes);
        let mut cache = self.sub_tree_cache.borrow_mut();
        let mut seen: HashSet<&str> = HashSet::new();
        // Mirror the predicate gating in `captures_in_rows` so indent
        // scopes are only collected for regions that actually inject.
        let mut text_provider = host_bytes;
        let mut pred_buf1 = Vec::new();
        let mut pred_buf2 = Vec::new();
        while let Some(m) = matches.next() {
            if !m.satisfies_text_predicates(
                &self.query,
                &mut pred_buf1,
                &mut pred_buf2,
                &mut text_provider,
            ) {
                continue;
            }
            let Some(lang_name) = self
                .pattern_lang
                .get(m.pattern_index)
                .and_then(Option::as_deref)
            else {
                continue;
            };
            let Some(sub) = self.subs.get(lang_name) else {
                continue;
            };
            let Some(indents_query) = sub.indents.as_ref() else {
                continue;
            };
            for cap in m.captures {
                if cap.index != content_idx {
                    continue;
                }
                let node = cap.node;
                let span_start_row = node.start_position().row;
                let span_end_row = node.end_position().row;
                if span_end_row < start_row || span_start_row > end_row {
                    continue;
                }
                let start_byte = node.start_byte();
                let end_byte = node.end_byte().min(host_bytes.len());
                if end_byte <= start_byte {
                    continue;
                }
                let Ok(slice) = std::str::from_utf8(&host_bytes[start_byte..end_byte]) else {
                    continue;
                };
                seen.insert(slice);
                let sub_tree = match cache.get(slice) {
                    Some((cached_lang, t)) if cached_lang == lang_name => t.clone(),
                    _ => {
                        let mut parser = Parser::new();
                        if parser.set_language(&sub.language).is_err() {
                            continue;
                        }
                        let Some(t) = parser.parse(slice, None) else {
                            continue;
                        };
                        cache.insert(slice.to_string(), (lang_name.to_string(), t.clone()));
                        t
                    }
                };
                let host_row_offset = node.start_position().row;
                let mut sub_cursor = QueryCursor::new();
                let mut sub_matches =
                    sub_cursor.matches(indents_query, sub_tree.root_node(), slice.as_bytes());
                while let Some(sm) = sub_matches.next() {
                    for sub_cap in sm.captures {
                        let name = sub
                            .indent_capture_names
                            .get(sub_cap.index as usize)
                            .map(String::as_str)
                            .unwrap_or("");
                        if name != "indent.begin" {
                            continue;
                        }
                        let s = sub_cap.node.start_position().row;
                        let e = sub_cap.node.end_position().row;
                        if e <= s {
                            continue;
                        }
                        let host_s = host_row_offset + s;
                        let host_e = host_row_offset + e;
                        if host_e < start_row || host_s > end_row {
                            continue;
                        }
                        out.push((host_s, host_e));
                    }
                }
            }
        }
        cache.retain(|k, _| seen.contains(k.as_str()));
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_sub_captures(
        &self,
        sub: &SubHighlighter,
        sub_tree: &Tree,
        sub_source: &str,
        host_source: &str,
        host_line_starts: &[usize],
        host_row_offset: usize,
        host_col_offset_first_row: usize,
        start_row: usize,
        end_row: usize,
        out: &mut Vec<Capture>,
    ) {
        let mut cursor = QueryCursor::new();
        let sub_bytes = sub_source.as_bytes();
        let mut matches = cursor.matches(&sub.query, sub_tree.root_node(), sub_bytes);
        let mut text_provider = sub_bytes;
        let mut pred_buf1 = Vec::new();
        let mut pred_buf2 = Vec::new();
        while let Some(m) = matches.next() {
            // Apply `#eq?` / `#match?` / `#any-of?` text predicates, same
            // as the host highlight path — see `highlight::captures_in_rows`.
            if !m.satisfies_text_predicates(
                &sub.query,
                &mut pred_buf1,
                &mut pred_buf2,
                &mut text_provider,
            ) {
                continue;
            }
            for cap in m.captures {
                let s = cap.node.start_position();
                let e = cap.node.end_position();
                let host_start_row = host_row_offset + s.row;
                let host_end_row = host_row_offset + e.row;
                if host_end_row < start_row || host_start_row > end_row {
                    continue;
                }
                // Only the slice's first row carries the host's
                // column offset; subsequent rows are flush left in
                // the host because the embedded region's newlines
                // are real newlines in the host source.
                let host_start_col = if s.row == 0 {
                    host_col_offset_first_row + s.column
                } else {
                    s.column
                };
                let host_end_col = if e.row == 0 {
                    host_col_offset_first_row + e.column
                } else {
                    e.column
                };
                let name = sub
                    .capture_names
                    .get(cap.index as usize)
                    .cloned()
                    .unwrap_or_default();
                out.push(Capture {
                    start_row: host_start_row,
                    start_col: super::engine::byte_to_char_col_indexed(
                        host_source,
                        host_line_starts,
                        host_start_row,
                        host_start_col,
                    ),
                    end_row: host_end_row,
                    end_col: super::engine::byte_to_char_col_indexed(
                        host_source,
                        host_line_starts,
                        host_end_row,
                        host_end_col,
                    ),
                    name,
                });
            }
        }
    }
}
