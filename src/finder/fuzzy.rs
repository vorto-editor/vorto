use std::fs;
use std::path::Path;

/// Filter toggles for the fuzzy file picker / tree explorer. Both axes
/// are independent: `vcs` decides whether to honor `.gitignore`, and
/// `hidden` decides whether to apply the configured
/// [`hidden_patterns`](crate::config::FilePickerConfig::hidden_patterns)
/// (defaults to dotfiles + heavy build dirs). The two filters compose,
/// so an entry that matches both rules requires both flags off to
/// surface — eg. `.cache/` (dotfile + gitignored) needs `.` and `h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IgnoreOpts {
    /// Honor VCS ignore rules. When true and we're inside a git repo
    /// the source is `git ls-files --cached --others
    /// --exclude-standard`; when false (and outside a repo) the walker
    /// falls back to a manual directory walk that only applies the
    /// `hidden_patterns` filter.
    pub vcs: bool,
    /// Apply `hidden_patterns` glob matching to entry basenames.
    /// Default `true`; flipped to `false` via the explorer's `.` key.
    pub hidden: bool,
}

impl IgnoreOpts {
    /// Standard `<space>f` behavior: filter both gitignored and hidden.
    pub const DEFAULT: Self = Self {
        vcs: true,
        hidden: true,
    };
    /// `<space>F` behavior: still respect `.gitignore`, but surface
    /// dotfiles.
    pub const SHOW_HIDDEN: Self = Self {
        vcs: true,
        hidden: false,
    };
}

/// Match a path basename against a glob pattern containing optional
/// `*` wildcards (each `*` matches zero or more characters). Anchored
/// on both ends — pattern `node_modules` matches only that exact name,
/// not `my_node_modules_old`. Pattern `.*` matches every dotfile.
///
/// Tiny ad-hoc matcher rather than a glob crate because the patterns
/// list is short and the call shape (one basename per `read_dir` entry)
/// doesn't benefit from a compiled matcher.
pub fn matches_glob(pattern: &str, name: &str) -> bool {
    fn rec(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (None, _) => false,
            (Some(&b'*'), _) => {
                if rec(&p[1..], n) {
                    return true;
                }
                if !n.is_empty() && rec(p, &n[1..]) {
                    return true;
                }
                false
            }
            (Some(&pc), Some(&nc)) if pc == nc => rec(&p[1..], &n[1..]),
            _ => false,
        }
    }
    rec(pattern.as_bytes(), name.as_bytes())
}

/// True if `name` (a single path component) matches any pattern in
/// `patterns`. Empty `patterns` is always false.
pub fn matches_any_hidden(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| matches_glob(p, name))
}

/// True if any segment of `rel` (a `/`-separated relative path) matches
/// `patterns`. Used to post-filter `git ls-files` output, where we get
/// full paths rather than walked entries — a tracked file inside a
/// hidden-pattern directory still counts as hidden.
pub fn rel_path_has_hidden_segment(rel: &str, patterns: &[String]) -> bool {
    rel.split('/').any(|seg| matches_any_hidden(seg, patterns))
}

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

/// Enumerate every directory under `root` (excluding `root` itself),
/// respecting the same hidden filter [`workspace_files`] applies.
/// Always walks the filesystem — `git ls-files` doesn't surface empty
/// directories, so even in a repo we need a manual pass for the
/// explorer to expose them as targets for new files.
pub fn workspace_dirs(
    root: &Path,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    collect_dirs(root, root, &mut out, 0, ignore, hidden_patterns, max_items);
    out.sort();
    out
}

/// Enumerate every file the file/workspace pickers should see, anchored
/// at `root` and respecting `ignore` plus `hidden_patterns`. Prefers
/// `git ls-files` when in a repo and `ignore.vcs` is on; otherwise
/// walks the directory tree manually. In both paths `hidden_patterns`
/// is applied when `ignore.hidden` is true, and the total result is
/// capped at `max_items`.
pub fn workspace_files(
    root: &Path,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) -> Vec<String> {
    let mut items = if ignore.vcs
        && let Some(paths) = crate::vcs::tracked_files(root)
    {
        paths
            .into_iter()
            .filter(|p| !ignore.hidden || !rel_path_has_hidden_segment(p, hidden_patterns))
            .filter(|p| !is_symlink(&root.join(p)))
            .take(max_items)
            .collect()
    } else {
        let mut v = Vec::new();
        collect_files(root, root, &mut v, 0, ignore, hidden_patterns, max_items);
        v
    };
    items.sort();
    items
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

/// Case-insensitive substring search for the ASCII fast path. Returns
/// the byte (= char, since haystack is ASCII) offset where the needle
/// first occurs. `needle_lower` must already be lower-cased; the
/// haystack is lower-cased inline byte-by-byte (no allocation).
fn ascii_find_lower(hay: &str, needle_lower: &str) -> Option<usize> {
    let hay = hay.as_bytes();
    let ndl = needle_lower.as_bytes();
    if ndl.is_empty() {
        return Some(0);
    }
    if hay.len() < ndl.len() {
        return None;
    }
    'outer: for start in 0..=hay.len() - ndl.len() {
        for (k, &n) in ndl.iter().enumerate() {
            if hay[start + k].to_ascii_lowercase() != n {
                continue 'outer;
            }
        }
        return Some(start);
    }
    None
}

// Smith-Waterman-style scoring constants, matching nucleo/fzf v2 so
// the picker ranks results the way Helix users expect. Word-boundary
// hits dominate; long gaps are punished but not lethal.
const SCORE_MATCH: i32 = 16;
const SCORE_GAP_START: i32 = -3;
const SCORE_GAP_EXTEND: i32 = -1;
const BONUS_BOUNDARY: i32 = SCORE_MATCH / 2; // 8
const BONUS_CAMEL: i32 = BONUS_BOUNDARY - 1; // 7
const BONUS_CONSECUTIVE: i32 = -(SCORE_GAP_START + SCORE_GAP_EXTEND); // 4
const BONUS_FIRST_CHAR_MULT: i32 = 2;
const SCORE_NEG_INF: i32 = i32::MIN / 4;

#[derive(Copy, Clone, PartialEq)]
enum CharKind {
    NonWord,
    Lower,
    Upper,
    Number,
}

fn char_kind(c: char) -> CharKind {
    if c.is_ascii_lowercase() {
        CharKind::Lower
    } else if c.is_ascii_uppercase() {
        CharKind::Upper
    } else if c.is_ascii_digit() {
        CharKind::Number
    } else if c.is_alphanumeric() {
        // Treat non-ASCII letters (e.g. CJK) as word characters so a
        // transition from a separator into them still earns the
        // boundary bonus.
        CharKind::Lower
    } else {
        CharKind::NonWord
    }
}

fn boundary_bonus(prev: CharKind, curr: CharKind) -> i32 {
    use CharKind::*;
    match (prev, curr) {
        (NonWord, c) if c != NonWord => BONUS_BOUNDARY,
        (Lower, Upper) => BONUS_CAMEL,
        (Lower | Upper, Number) => BONUS_CAMEL,
        _ => 0,
    }
}

/// Sub-sequence fuzzy match, modelled on Helix's nucleo / fzf v2.
/// Each needle character must appear in the haystack in order; gaps
/// between matches are permitted but penalised. Returns `(score, char
/// positions)` where positions are the indices of the matched needle
/// characters in the haystack (used for highlighting).
///
/// Scoring rewards: word-boundary anchored matches (after `/`, `_`,
/// `-`, `.`, ` ` or at start), camelCase boundaries
/// (`fooBar`-style), consecutive matches, and exact-case characters.
/// Scoring penalises every skipped haystack character (affine gap:
/// the first skip costs `SCORE_GAP_START`, each subsequent skip costs
/// `SCORE_GAP_EXTEND`).
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let hay: Vec<char> = haystack.chars().collect();
    let ndl: Vec<char> = needle.chars().collect();
    let n = ndl.len();
    let m = hay.len();
    if n > m {
        return None;
    }

    // Per-position transition bonus: `bonus[j]` is what you earn for
    // matching at `hay[j]` given the kind of `hay[j-1]`. Position 0
    // is treated as if preceded by a NonWord character, so a match
    // there is always a boundary hit.
    let mut bonus = vec![0i32; m];
    let mut prev_kind = CharKind::NonWord;
    for (j, &c) in hay.iter().enumerate() {
        let k = char_kind(c);
        bonus[j] = boundary_bonus(prev_kind, k);
        prev_kind = k;
    }

    // Two DP tables. `mscore[i][j]` is the best score for matching
    // needle[..=i] with `hay[j]` taken as the final match. `gscore[i][j]`
    // is the best score for matching needle[..=i] when `hay[j]` is *not*
    // a match — i.e. we're currently extending a gap that follows the
    // last needle char. Parent tables record where the predecessor
    // needle character landed, so we can rebuild the position list.
    let cell = |i: usize, j: usize| i * m + j;
    let mut mscore = vec![SCORE_NEG_INF; n * m];
    let mut gscore = vec![SCORE_NEG_INF; n * m];
    let mut mparent = vec![usize::MAX; n * m];
    let mut gmatch = vec![usize::MAX; n * m]; // gscore traceback: where needle[i] matched

    for i in 0..n {
        for j in i..m {
            let nc = ndl[i];
            let hc = hay[j];
            let is_match = nc.eq_ignore_ascii_case(&hc);

            if is_match {
                let case_bonus = if nc == hc { 1 } else { 0 };
                let ms = if i == 0 {
                    SCORE_MATCH + bonus[j] * BONUS_FIRST_CHAR_MULT + case_bonus
                } else if j == 0 {
                    SCORE_NEG_INF
                } else {
                    let from_m = mscore[cell(i - 1, j - 1)];
                    let from_g = gscore[cell(i - 1, j - 1)];
                    let consec_bonus = BONUS_CONSECUTIVE.max(bonus[j]);
                    let via_m = from_m.saturating_add(SCORE_MATCH + consec_bonus + case_bonus);
                    let via_g = from_g.saturating_add(SCORE_MATCH + bonus[j] + case_bonus);
                    if from_m == SCORE_NEG_INF && from_g == SCORE_NEG_INF {
                        SCORE_NEG_INF
                    } else if via_m >= via_g {
                        mparent[cell(i, j)] = j - 1;
                        via_m
                    } else {
                        // Predecessor's match position lives in the gap-trace table.
                        mparent[cell(i, j)] = gmatch[cell(i - 1, j - 1)];
                        via_g
                    }
                };
                mscore[cell(i, j)] = ms;
            }

            if j > 0 {
                let from_m = mscore[cell(i, j - 1)];
                let from_g = gscore[cell(i, j - 1)];
                let start = from_m.saturating_add(SCORE_GAP_START);
                let extend = from_g.saturating_add(SCORE_GAP_EXTEND);
                if from_m == SCORE_NEG_INF && from_g == SCORE_NEG_INF {
                    // no predecessor yet
                } else if extend >= start {
                    gscore[cell(i, j)] = extend;
                    gmatch[cell(i, j)] = gmatch[cell(i, j - 1)];
                } else {
                    gscore[cell(i, j)] = start;
                    gmatch[cell(i, j)] = j - 1;
                }
            }
        }
    }

    // The match must end on an actual needle character — trailing
    // characters are unmatched and don't contribute. Pick the column
    // where the last needle row scores highest.
    let mut best_score = SCORE_NEG_INF;
    let mut best_j = usize::MAX;
    for j in (n - 1)..m {
        let s = mscore[cell(n - 1, j)];
        if s > best_score {
            best_score = s;
            best_j = j;
        }
    }
    if best_j == usize::MAX || best_score == SCORE_NEG_INF {
        return None;
    }

    // Walk parent pointers back from (n-1, best_j) collecting match
    // positions for highlighting.
    let mut positions = Vec::with_capacity(n);
    let mut i = n - 1;
    let mut j = best_j;
    positions.push(j);
    while i > 0 {
        let pj = mparent[cell(i, j)];
        if pj == usize::MAX {
            return None;
        }
        j = pj;
        i -= 1;
        positions.push(j);
    }
    positions.reverse();
    Some((best_score, positions))
}

/// True if `path` is a symlink (without following it). Symlinks are
/// filtered out of the picker because opening one whose target is a
/// directory or broken propagates an `io::Error` from `Buffer::load`
/// up to the main loop and terminates the editor.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Walk variant that records directory paths instead of files.
/// Used by the explorer so empty directories show up as creatable
/// targets — the file walker would skip them entirely.
fn collect_dirs(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    depth: usize,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) {
    if depth > 12 || out.len() >= max_items {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if ignore.hidden && matches_any_hidden(&name, hidden_patterns) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        if let Some(s) = path.strip_prefix(root).ok().and_then(|p| p.to_str()) {
            out.push(s.to_string());
        }
        collect_dirs(
            root,
            &path,
            out,
            depth + 1,
            ignore,
            hidden_patterns,
            max_items,
        );
    }
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    depth: usize,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) {
    if depth > 12 || out.len() >= max_items {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if ignore.hidden && matches_any_hidden(&name, hidden_patterns) {
            continue;
        }
        // Use `file_type` (not `is_dir`/`is_file`) so symlinks are
        // detected without being followed: traversing through a
        // directory symlink risks cycles, and listing a file symlink
        // can crash the editor on open (broken target / target is a
        // directory bubbles an io::Error out of the prompt path).
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(
                root,
                &path,
                out,
                depth + 1,
                ignore,
                hidden_patterns,
                max_items,
            );
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let rel = path.strip_prefix(root).ok().and_then(|p| p.to_str());
        if let Some(s) = rel {
            out.push(s.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(haystack: &str, needle: &str) -> Option<String> {
        let (_score, positions) = fuzzy_match(haystack, needle)?;
        let hay: Vec<char> = haystack.chars().collect();
        Some(positions.iter().map(|&i| hay[i]).collect())
    }

    #[test]
    fn skips_dot_separator() {
        let (_score, positions) = fuzzy_match("xx.go", "xxgo").expect("should match");
        assert_eq!(positions, vec![0, 1, 3, 4]);
    }

    #[test]
    fn skips_underscore_and_slash() {
        assert_eq!(matched("foo_bar", "foobar").as_deref(), Some("foobar"));
        assert_eq!(matched("src/foo.rs", "foors").as_deref(), Some("foors"));
        assert_eq!(matched("foo bar", "foobar").as_deref(), Some("foobar"));
    }

    #[test]
    fn case_insensitive() {
        let (_s, pos) = fuzzy_match("README.md", "readme").unwrap();
        assert_eq!(pos, vec![0, 1, 2, 3, 4, 5]);
    }

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

    #[test]
    fn glob_matches_literal_and_wildcard() {
        assert!(matches_glob("node_modules", "node_modules"));
        assert!(!matches_glob("node_modules", "my_node_modules_old"));
        // `.*` is the dotfile convention.
        assert!(matches_glob(".*", ".gitignore"));
        assert!(matches_glob(".*", ".env"));
        assert!(!matches_glob(".*", "Cargo.toml"));
        // `*` mid-pattern.
        assert!(matches_glob("*.lock", "Cargo.lock"));
        assert!(matches_glob("*.lock", ".lock"));
        assert!(!matches_glob("*.lock", "Cargo.toml"));
        // Multiple `*`s.
        assert!(matches_glob("*foo*", "abcfoo123"));
        assert!(matches_glob("*foo*", "foo"));
        // Empty patterns.
        assert!(matches_glob("", ""));
        assert!(!matches_glob("", "x"));
        assert!(matches_glob("*", ""));
        assert!(matches_glob("*", "anything"));
    }

    #[test]
    fn rel_path_hidden_segment_walks_segments() {
        let pats = vec![".*".to_string(), "node_modules".to_string()];
        assert!(rel_path_has_hidden_segment(
            ".github/workflows/ci.yml",
            &pats
        ));
        assert!(rel_path_has_hidden_segment("a/node_modules/b/c", &pats));
        assert!(!rel_path_has_hidden_segment("src/main.rs", &pats));
        assert!(!rel_path_has_hidden_segment("Cargo.toml", &pats));
    }

    #[test]
    fn camel_case_boundary_preferred() {
        // Both haystacks contain "foobar" as a subsequence, but
        // `fooBar` has a camelCase boundary at 'B' which should rank
        // higher than the flat `foobar`.
        let (camel, _) = fuzzy_match("fooBar", "foob").unwrap();
        let (flat, _) = fuzzy_match("flooba", "foob").unwrap();
        assert!(camel > flat, "camelCase {camel} should outrank flat {flat}");
    }

    #[test]
    fn word_boundary_outranks_mid_word() {
        // `src/foo` should rank higher than `srcafoo` for needle "foo"
        // because the match starts on a path-segment boundary.
        let (boundary, _) = fuzzy_match("src/foo", "foo").unwrap();
        let (mid, _) = fuzzy_match("srcafoo", "foo").unwrap();
        assert!(
            boundary > mid,
            "boundary {boundary} should outrank mid-word {mid}"
        );
    }

    #[test]
    fn subsequence_with_long_gap() {
        // Needle chars need only appear in order anywhere in the
        // haystack — pure fzf-style subsequence matching.
        let (_score, positions) = fuzzy_match("alphabet_soup", "asp").unwrap();
        assert_eq!(positions.len(), 3);
        // Match must be in order and use distinct positions.
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn rejects_when_letters_missing() {
        assert!(fuzzy_match("xx.go", "xxrs").is_none());
        assert!(fuzzy_match("abc", "abcd").is_none());
        // Needle character not present anywhere.
        assert!(fuzzy_match("src/foo", "srcz").is_none());
    }
}
