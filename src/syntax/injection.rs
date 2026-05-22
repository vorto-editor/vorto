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

use std::collections::HashMap;

use anyhow::{Context, Result};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator, Tree};

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
        tree: &Tree,
        start_row: usize,
        end_row: usize,
    ) -> Vec<Capture> {
        let Some(content_idx) = self.content_idx else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), host_source.as_bytes());
        let host_bytes = host_source.as_bytes();
        while let Some(m) = matches.next() {
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
                let mut parser = Parser::new();
                if parser.set_language(&sub.language).is_err() {
                    continue;
                }
                let Some(sub_tree) = parser.parse(slice, None) else {
                    continue;
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
                    host_start_pt.row,
                    host_start_pt.column,
                    start_row,
                    end_row,
                    &mut out,
                );
            }
        }
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
        let mut matches = cursor.matches(&self.query, tree.root_node(), host_source.as_bytes());
        let host_bytes = host_source.as_bytes();
        while let Some(m) = matches.next() {
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
                let mut parser = Parser::new();
                if parser.set_language(&sub.language).is_err() {
                    continue;
                }
                let Some(sub_tree) = parser.parse(slice, None) else {
                    continue;
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
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_sub_captures(
        &self,
        sub: &SubHighlighter,
        sub_tree: &Tree,
        sub_source: &str,
        host_source: &str,
        host_row_offset: usize,
        host_col_offset_first_row: usize,
        start_row: usize,
        end_row: usize,
        out: &mut Vec<Capture>,
    ) {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&sub.query, sub_tree.root_node(), sub_source.as_bytes());
        while let Some(m) = matches.next() {
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
                    start_col: super::engine::byte_to_char_col(
                        host_source,
                        host_start_row,
                        host_start_col,
                    ),
                    end_row: host_end_row,
                    end_col: super::engine::byte_to_char_col(
                        host_source,
                        host_end_row,
                        host_end_col,
                    ),
                    name,
                });
            }
        }
    }
}
