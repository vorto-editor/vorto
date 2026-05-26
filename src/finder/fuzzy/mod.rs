use std::path::Path;

mod score;
mod walk;

pub use score::fuzzy_match;
pub use walk::{IgnoreOpts, workspace_dirs, workspace_files};

use score::ascii_find_lower;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuzzyKind {
    /// Fuzzy file picker. See [`IgnoreOpts`] for the filter axes.
    Files {
        ignore: IgnoreOpts,
    },
    Lines,
    /// Cross-file location results (LSP references). The Finder carries
    /// a parallel `locations` Vec so the picker can jump on selection.
    Locations,
    /// `<space>/` — workspace-wide line search. Same data shape as
    /// [`Locations`] (display strings + parallel `Location`s on the
    /// prompt controller); split out only so the picker title and any
    /// kind-specific rendering can differ.
    WorkspaceSearch,
    /// Recently-opened files (MRU). Display strings are paths
    /// (typically relative to startup_cwd); the
    /// [`PromptController`](crate::prompt::PromptController) keeps a
    /// parallel `buffer_paths` Vec for the absolute path to actually
    /// open on selection.
    Buffers,
    /// LSP diagnostics picker. Same wiring as [`Locations`] — the
    /// caller supplies display strings and a parallel `Vec<Location>`
    /// on the prompt controller; submit fires `JumpToLocation`. The
    /// `workspace` flag toggles between "current buffer only" and
    /// "every URI the coordinator has diagnostics for" so the title
    /// and item formatting can differ without duplicating the picker
    /// plumbing.
    Diagnostics {
        workspace: bool,
    },
    /// `:jumps` / `<space>j` — fuzzy picker over the jump history. Same
    /// data shape as [`Locations`] (display strings + a parallel
    /// `Vec<Location>` on the prompt controller); submit fires
    /// `JumpToLocation`. Split out only so the picker title differs.
    Jumps,
}

#[derive(Debug, Clone)]
pub struct MatchItem {
    pub idx: usize,
    pub score: i32,
    /// Char indices into the item haystack that the fuzzy matcher hit —
    /// used by the picker list to paint hit highlights. Empty for
    /// [`FuzzyKind::WorkspaceSearch`], where matching is against line
    /// content rather than the displayed path.
    pub positions: Vec<usize>,
    /// 0-based line numbers in the item's file that matched the query,
    /// sorted by score (best first). Only populated for
    /// [`FuzzyKind::WorkspaceSearch`]; empty for every other kind.
    pub line_hits: Vec<usize>,
    /// 0-based char column where the matched substring starts in the
    /// hit line — used by `<space>/` so the cursor lands on the match
    /// itself when the user submits, not at column 0. Only meaningful
    /// for [`FuzzyKind::WorkspaceSearch`]; zero everywhere else.
    pub match_col: u32,
}

#[derive(Debug)]
pub struct Finder {
    pub kind: FuzzyKind,
    pub query: String,
    /// Char index of the insertion point into `query`, in `[0, char_count]`.
    pub cursor: usize,
    pub items: Vec<String>,
    /// Per-file line content, parallel to [`items`] when
    /// `kind == FuzzyKind::WorkspaceSearch`. Empty (and unused) for
    /// every other kind. Lives on the Finder so `refilter` can scan
    /// content on each keystroke without bouncing through a side
    /// channel.
    ///
    /// [`items`]: Self::items
    pub file_lines: Vec<Vec<String>>,
    pub matches: Vec<MatchItem>,
    pub selected: usize,
}

impl Finder {
    pub fn files(
        root: &Path,
        ignore: IgnoreOpts,
        hidden_patterns: &[String],
        max_items: usize,
    ) -> Self {
        // Prefer git when VCS filtering is on AND we're in a repo — it's
        // both faster and exact (matches `.gitignore`, global excludes,
        // etc.). The hidden filter is applied as a post-pass since git
        // doesn't know about our dotfile convention.
        let items = workspace_files(root, ignore, hidden_patterns, max_items);
        let mut f = Self {
            kind: FuzzyKind::Files { ignore },
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    pub fn lines(buffer_lines: &[String]) -> Self {
        let items: Vec<String> = buffer_lines.to_vec();
        let mut f = Self {
            kind: FuzzyKind::Lines,
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    /// Build a [`FuzzyKind::Buffers`] picker. `items` are the display
    /// strings (newest first); the caller stashes the absolute path
    /// for each one separately and uses `selection().idx` to look it
    /// up on submit.
    pub fn buffers(items: Vec<String>) -> Self {
        let mut f = Self {
            kind: FuzzyKind::Buffers,
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    /// Build a [`FuzzyKind::Jumps`] picker — identical plumbing to
    /// [`Self::locations`] (parallel `Vec<Location>` on the caller),
    /// only the `kind` differs so the title reads "jumps".
    pub fn jumps(items: Vec<String>) -> Self {
        let mut f = Self {
            kind: FuzzyKind::Jumps,
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    /// Build a [`FuzzyKind::Locations`] picker. Display strings are
    /// arbitrary; the caller keeps a parallel `Vec` (typically of
    /// `lsp::Location`) and looks up the selected index to decide what
    /// to do on submit.
    pub fn locations(items: Vec<String>) -> Self {
        let mut f = Self {
            kind: FuzzyKind::Locations,
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    /// Build a [`FuzzyKind::Diagnostics`] picker. Plumbing matches
    /// [`Self::locations`] — `items` are the display strings and the
    /// caller is responsible for stashing the parallel `Location`s on
    /// the prompt controller.
    pub fn diagnostics(items: Vec<String>, workspace: bool) -> Self {
        let mut f = Self {
            kind: FuzzyKind::Diagnostics { workspace },
            query: String::new(),
            items,
            file_lines: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    /// Build a [`FuzzyKind::WorkspaceSearch`] picker.
    ///
    /// `items` are the file path display strings (typically relative to
    /// `startup_cwd`); `file_lines[i]` is the full line content of
    /// `items[i]`. Each keystroke fuzzy-matches the query against every
    /// line of every file, then surfaces one [`MatchItem`] per file —
    /// with `line_hits` listing the rows that matched, best score
    /// first.
    ///
    /// The caller still keeps a parallel `Vec<Location>` side-channel
    /// (one per file) on [`PromptController`] so submit can build a
    /// jump target without recomputing paths/URIs.
    ///
    /// [`PromptController`]: crate::prompt::PromptController
    pub fn workspace_search(items: Vec<String>, file_lines: Vec<Vec<String>>) -> Self {
        debug_assert_eq!(items.len(), file_lines.len());
        let mut f = Self {
            kind: FuzzyKind::WorkspaceSearch,
            query: String::new(),
            items,
            file_lines,
            matches: Vec::new(),
            selected: 0,
            cursor: 0,
        };
        f.refilter();
        f
    }

    fn char_len(&self) -> usize {
        self.query.chars().count()
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.query
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len())
    }

    fn insert(&mut self, c: char) {
        let byte = self.byte_idx(self.cursor);
        self.query.insert(byte, c);
        self.cursor += 1;
        self.refilter();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_idx(self.cursor);
        let start = self.byte_idx(self.cursor - 1);
        self.query.replace_range(start..end, "");
        self.cursor -= 1;
        self.refilter();
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_idx(self.cursor);
        let end = self.byte_idx(self.cursor + 1);
        self.query.replace_range(start..end, "");
        self.refilter();
    }

    pub fn apply_line_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right if self.cursor < self.char_len() => self.cursor += 1,
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.char_len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Char('b') if ctrl => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl && self.cursor < self.char_len() => self.cursor += 1,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.char_len(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
    }

    pub fn next(&mut self) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1).min(self.matches.len() - 1);
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn selection(&self) -> Option<&MatchItem> {
        self.matches.get(self.selected)
    }

    fn refilter(&mut self) {
        self.matches.clear();
        if matches!(self.kind, FuzzyKind::WorkspaceSearch) {
            self.refilter_workspace();
            self.selected = 0;
            return;
        }
        if self.query.is_empty() {
            for (i, _) in self.items.iter().enumerate().take(500) {
                self.matches.push(MatchItem {
                    idx: i,
                    score: 0,
                    positions: Vec::new(),
                    line_hits: Vec::new(),
                    match_col: 0,
                });
            }
        } else {
            for (i, item) in self.items.iter().enumerate() {
                if let Some((score, positions)) = fuzzy_match(item, &self.query) {
                    self.matches.push(MatchItem {
                        idx: i,
                        score,
                        positions,
                        line_hits: Vec::new(),
                        match_col: 0,
                    });
                }
            }
            self.matches.sort_by_key(|m| -m.score);
            self.matches.truncate(500);
        }
        self.selected = 0;
    }

    /// `<space>/` refilter path. Substring match (not fuzzy) to match
    /// Helix's global-search behavior — predictable, fast, and aligned
    /// with how users already think about grepping a codebase.
    ///
    /// Case handling is smart-case: a lower-case query matches case-
    /// insensitively; any upper-case char in the query flips the match
    /// to case-sensitive. Same convention as ripgrep / vim's `smartcase`.
    ///
    /// Empty query → no candidates (nothing to match yet); otherwise:
    /// emit one match item per line containing the query, capped
    /// globally at [`WORKSPACE_SEARCH_MAX_MATCHES`] across the whole
    /// workspace.
    fn refilter_workspace(&mut self) {
        if self.query.is_empty() {
            return;
        }
        let case_sensitive = self.query.chars().any(|c| c.is_uppercase());
        // Build a lower-cased needle once per refilter when we're going
        // case-insensitive — the per-line allocation that would
        // otherwise happen inside the loop is exactly what we're trying
        // to avoid.
        let needle_ci: Option<String> = (!case_sensitive).then(|| self.query.to_lowercase());
        let mut scratch = String::new();
        'files: for (i, lines) in self.file_lines.iter().enumerate() {
            for (row, line) in lines.iter().enumerate() {
                // Long lines (minified bundles, generated data) blow up
                // `to_lowercase` for nothing useful — bail before
                // touching them.
                if line.len() > WORKSPACE_SEARCH_MAX_LINE_BYTES {
                    continue;
                }
                // Locate the substring's char column too (not just
                // whether it matches) so submit can land the cursor on
                // the hit, not at the line start.
                let col: Option<u32> = match &needle_ci {
                    None => line
                        .find(self.query.as_str())
                        .map(|byte| line[..byte].chars().count() as u32),
                    Some(n) => {
                        if line.is_ascii() {
                            // ASCII fast-path: byte offset == char
                            // offset, and no allocation.
                            ascii_find_lower(line, n).map(|c| c as u32)
                        } else {
                            // Unicode case-insensitive: lower-case via a
                            // reused scratch. The lowered byte offset
                            // can't be mapped back to the original
                            // line's char column precisely (case-folding
                            // is not length-preserving), so on a hit we
                            // settle for column 0 — rare in code search.
                            scratch.clear();
                            scratch.extend(line.chars().flat_map(|c| c.to_lowercase()));
                            scratch.contains(n.as_str()).then_some(0)
                        }
                    }
                };
                let Some(col) = col else {
                    continue;
                };
                // One row in the candidate list per match. `idx` still
                // names the file (used to look up the path / location
                // / line content); `line_hits` holds the matched row;
                // `match_col` is the cursor target column.
                self.matches.push(MatchItem {
                    idx: i,
                    // Substring match — no real score to sort by. Keep
                    // encounter order (workspace-walker alphabetical
                    // by file, then top-to-bottom within each file).
                    score: 0,
                    positions: Vec::new(),
                    line_hits: vec![row],
                    match_col: col,
                });
                if self.matches.len() >= WORKSPACE_SEARCH_MAX_MATCHES {
                    break 'files;
                }
            }
        }
    }
}

/// Hard cap on candidate rows in workspace search. Each row is one
/// match (one file × one line); a runaway query that hits everything
/// would otherwise blow up the list and per-frame render cost.
const WORKSPACE_SEARCH_MAX_MATCHES: usize = 2000;

/// Skip lines longer than this in workspace search. Lower-casing and
/// substring-scanning a 200KB minified line would dominate every
/// keystroke; cap it.
const WORKSPACE_SEARCH_MAX_LINE_BYTES: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    /// Perf sanity check — refilter at various item counts. Not part of
    /// the normal suite (annotated `#[ignore]`); run on demand with:
    /// `cargo test --release bench_refilter -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_refilter_scaling() {
        use std::time::Instant;
        // Synthesize realistic-looking paths so the fuzzy matcher
        // exercises typical char distributions.
        let make_items = |n: usize| -> Vec<String> {
            (0..n)
                .map(|i| {
                    format!(
                        "src/module_{}/sub_{}/file_{}.rs",
                        i % 137,
                        (i / 137) % 41,
                        i
                    )
                })
                .collect()
        };
        for &n in &[5_000usize, 20_000, 50_000, 100_000, 200_000] {
            let items = make_items(n);
            let mut f = Finder {
                kind: FuzzyKind::Files {
                    ignore: IgnoreOpts::DEFAULT,
                },
                query: "modfilers".into(),
                cursor: 9,
                items,
                file_lines: Vec::new(),
                matches: Vec::new(),
                selected: 0,
            };
            // Warmup.
            f.refilter();
            let t = Instant::now();
            for _ in 0..3 {
                f.refilter();
            }
            let avg = t.elapsed() / 3;
            eprintln!("  {n:>7} items: {avg:?} per refilter");
        }
    }
}
