//! Tree-style file explorer state.
//!
//! Powers `<space>e`. The structure is built once at open time from
//! `workspace_files` (which honors `.gitignore` and the dotfile skip),
//! then the user navigates by toggling expand/collapse on directory
//! rows. A live fuzzy query box at the top filters the tree to files
//! whose path matches — and auto-expands every ancestor directory of a
//! matched file so the hit is reachable without manual chord work.

mod fs_ops;
mod input;
mod tree;

pub use tree::ExplorerNode;

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use std::path::PathBuf;

use super::IgnoreOpts;
use super::fuzzy::{fuzzy_match, workspace_dirs, workspace_files};

use tree::build_nodes;

/// Top-level input mode for the explorer.
///
/// The widget opens in [`Selection`] — keys like `j/k/a/d/r/m` operate
/// on the highlighted row. `/` switches to [`Filter`] for fuzzy-querying
/// the tree; the pending modes are transient prompts driven by `a/d/r/m`.
///
/// [`Selection`]: Self::Selection
/// [`Filter`]: Self::Filter
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerMode {
    /// Default. Single-key navigation and file ops.
    Selection,
    /// `/` — query field is live; characters flow into the fuzzy filter.
    Filter,
    /// `a` — input field for a new file/dir path. Trailing `/` means dir.
    PendingCreate,
    /// `r` — input field for a renamed basename.
    PendingRename,
    /// `m` — input field for the destination rel path.
    PendingMove,
    /// `d` — y/N confirmation overlay on the highlighted entry.
    PendingDelete,
}

/// Tiny insertion-point text buffer used by the pending action prompts.
/// We don't reuse `prompt::LineInput` to avoid a finder→prompt module
/// dependency; the editing surface here is intentionally small and
/// mirrors the existing query input behavior.
#[derive(Default, Debug)]
pub struct ActionInput {
    pub text: String,
    pub cursor: usize,
}

impl ActionInput {
    fn new(initial: &str) -> Self {
        Self {
            text: initial.to_string(),
            cursor: initial.chars().count(),
        }
    }

    fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn insert(&mut self, c: char) {
        let byte = self.byte_idx(self.cursor);
        self.text.insert(byte, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_idx(self.cursor);
        let start = self.byte_idx(self.cursor - 1);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_idx(self.cursor);
        let end = self.byte_idx(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }
}

/// Live state for the explorer prompt. Owns the full node list, the
/// expanded-dir set, the fuzzy query, and the cached visible-row
/// projection that the renderer iterates.
pub struct ExplorerState {
    pub nodes: Vec<ExplorerNode>,
    /// Rel-paths of currently expanded directories. The explorer opens
    /// with this empty — every dir starts collapsed; the user expands
    /// as they explore, or typing a query auto-expands ancestors of
    /// matched files.
    pub expanded: HashSet<String>,
    /// Live query. Empty means "show the tree as-is".
    pub query: String,
    /// Char index of the insertion point in `query`.
    pub cursor: usize,
    /// Selected index into [`visible`]. Always in `[0, visible.len())`
    /// after `refilter` — clamped on shrink.
    pub selected: usize,
    /// Indexes into [`nodes`] for rows actually drawn — collapsed
    /// subtrees and (when filtering) non-matching files are omitted.
    /// Recomputed by [`Self::refilter`].
    ///
    /// [`nodes`]: Self::nodes
    /// [`visible`]: Self::visible
    pub visible: Vec<usize>,
    /// Top row of the visible window inside `visible`. The renderer pulls
    /// it down/up only when `selected` falls outside the viewport, so the
    /// list stays put while the cursor moves around inside the visible
    /// page (instead of pinning the cursor to the bottom row once it
    /// scrolls past). Wrapped in a `Cell` because the renderer holds the
    /// state by shared reference but still needs to nudge scroll based on
    /// the live viewport height it discovers each frame.
    pub scroll: Cell<usize>,
    /// Current input mode. Defaults to [`ExplorerMode::Selection`].
    pub mode: ExplorerMode,
    /// In-flight action input (path for create/move, basename for
    /// rename). `None` outside the pending-input modes.
    pub action: Option<ActionInput>,
    /// Workspace root captured at open time so file ops can resolve
    /// rel paths and the tree can be rebuilt after a mutation.
    pub root: PathBuf,
    /// Ignore options captured at open time, mirrored when the tree is
    /// rebuilt after a create/delete/rename/move.
    pub ignore: IgnoreOpts,
    /// Glob patterns marking entries as hidden. Captured at open time;
    /// `refresh` forwards them to `workspace_files` / `workspace_dirs`
    /// so toggling `ignore.hidden` produces a consistent view.
    pub hidden_patterns: Vec<String>,
    /// Walker / `git ls-files` cap. Surfaced here so `refresh` after a
    /// file op rebuilds the tree with the same limit the explorer was
    /// opened under.
    pub max_items: usize,
    /// Last error message produced by a file op. Cleared on the next
    /// successful op or mode change.
    pub error: Option<String>,
}

impl ExplorerState {
    pub fn new(
        root: &Path,
        ignore: IgnoreOpts,
        hidden_patterns: Vec<String>,
        max_items: usize,
        _compact: bool,
    ) -> Self {
        let files = workspace_files(root, ignore, &hidden_patterns, max_items);
        let dirs = workspace_dirs(root, ignore, &hidden_patterns, max_items);
        let nodes = build_nodes(&files, &dirs);
        let mut s = Self {
            nodes,
            expanded: HashSet::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            visible: Vec::new(),
            scroll: Cell::new(0),
            mode: ExplorerMode::Selection,
            action: None,
            root: root.to_path_buf(),
            ignore,
            hidden_patterns,
            max_items,
            error: None,
        };
        s.refilter();
        s
    }

    /// Re-scan the workspace and rebuild the node list — used after
    /// create/delete/rename/move so the tree reflects what's on disk.
    /// Tries to keep the selected node on the same rel_path when it
    /// still exists; otherwise clamps to the new last visible row.
    pub fn refresh(&mut self) {
        let prev_path = self.selection().map(|n| n.rel_path.clone());
        let files = workspace_files(
            &self.root,
            self.ignore,
            &self.hidden_patterns,
            self.max_items,
        );
        let dirs = workspace_dirs(
            &self.root,
            self.ignore,
            &self.hidden_patterns,
            self.max_items,
        );
        self.nodes = build_nodes(&files, &dirs);
        // Drop any expanded entry that no longer maps to a dir.
        let alive: HashSet<String> = self
            .nodes
            .iter()
            .filter(|n| n.is_dir)
            .map(|n| n.rel_path.clone())
            .collect();
        self.expanded.retain(|p| alive.contains(p));
        self.refilter();
        if let Some(path) = prev_path
            && let Some(pos) = self
                .visible
                .iter()
                .position(|&i| self.nodes[i].rel_path == path)
        {
            self.selected = pos;
        }
    }

    /// Flip the dotfile filter and rebuild the tree. Bound to `.` in
    /// selection mode so the user can surface `.env`/`.github/...` without
    /// reopening the explorer with `<space>F`.
    pub fn toggle_hidden(&mut self) {
        self.ignore.hidden = !self.ignore.hidden;
        self.refresh();
    }

    /// Flip the VCS-ignore filter and rebuild the tree. Bound to `h` in
    /// selection mode — useful for peeking at `target/`, `node_modules/`,
    /// or anything else `.gitignore` normally hides.
    pub fn toggle_vcs(&mut self) {
        self.ignore.vcs = !self.ignore.vcs;
        self.refresh();
    }

    /// Recompute [`visible`] from the current expand state and query.
    ///
    /// With no query: walk `nodes` in order and skip any node whose
    /// nearest enclosing collapsed dir hasn't been opened. With a
    /// query: fuzzy-match every file, then surface every matching
    /// file plus the chain of dir ancestors that leads to it (the
    /// ancestor dirs are also auto-expanded so successive non-empty
    /// queries don't lose state when the user backspaces and the
    /// matches change).
    ///
    /// [`visible`]: Self::visible
    pub fn refilter(&mut self) {
        let prev_selected_node = self.visible.get(self.selected).copied();
        self.visible.clear();

        if self.query.is_empty() {
            // Tree-walk with collapse-aware skipping. `cut_depth` is
            // the depth at which we're currently hiding everything
            // deeper (e.g. when we hit a collapsed dir at depth 2, we
            // skip all subsequent rows with depth > 2 until we leave
            // the subtree).
            let mut cut_depth: Option<usize> = None;
            for (i, n) in self.nodes.iter().enumerate() {
                if let Some(cd) = cut_depth {
                    if n.depth > cd {
                        continue;
                    }
                    cut_depth = None;
                }
                self.visible.push(i);
                if n.is_dir && !self.expanded.contains(&n.rel_path) {
                    cut_depth = Some(n.depth);
                }
            }
        } else {
            // File-fuzzy filter. Each file whose rel_path matches gets
            // its ancestor dirs added to `keep` and to `expanded`; we
            // then re-walk the node list and push every node whose
            // index is in `keep`.
            let mut keep: HashSet<usize> = HashSet::new();
            // Map rel_path -> node index for ancestor lookup. Building
            // this on every keystroke is fine: the tree is bounded by
            // the workspace_files cap (5000).
            let path_to_idx: BTreeMap<&str, usize> = self
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.rel_path.as_str(), i))
                .collect();
            for (i, n) in self.nodes.iter().enumerate() {
                if n.is_dir {
                    continue;
                }
                if fuzzy_match(&n.rel_path, &self.query).is_none() {
                    continue;
                }
                keep.insert(i);
                // Walk ancestors: for each `/`-separated prefix of the
                // file's rel_path, look up the dir node and mark it
                // kept + expanded. The empty prefix is the workspace
                // root, which has no node entry, so we skip it.
                let mut prefix = n.rel_path.as_str();
                while let Some(slash) = prefix.rfind('/') {
                    prefix = &prefix[..slash];
                    if let Some(&ix) = path_to_idx.get(prefix) {
                        keep.insert(ix);
                        self.expanded.insert(prefix.to_string());
                    }
                }
            }
            for (i, _) in self.nodes.iter().enumerate() {
                if keep.contains(&i) {
                    self.visible.push(i);
                }
            }
        }

        // Preserve the cursor across refilters: if the previously
        // selected node is still visible, follow it; otherwise clamp
        // to the new last row. This keeps backspace from snapping the
        // cursor to row 0 every keystroke.
        if let Some(node_idx) = prev_selected_node {
            if let Some(pos) = self.visible.iter().position(|&i| i == node_idx) {
                self.selected = pos;
            } else if self.selected >= self.visible.len() {
                self.selected = self.visible.len().saturating_sub(1);
            }
        } else if self.selected >= self.visible.len() {
            self.selected = self.visible.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(super) fn paths() -> Vec<String> {
        vec![
            "Cargo.toml".into(),
            "src/finder/fuzzy.rs".into(),
            "src/finder/mod.rs".into(),
            "src/ui/fuzzy/list.rs".into(),
        ]
    }

    pub(super) fn default_patterns() -> Vec<String> {
        vec![
            ".*".into(),
            "node_modules".into(),
            "target".into(),
            "dist".into(),
            "build".into(),
        ]
    }

    pub(super) fn make_state(nodes: Vec<ExplorerNode>, query: &str) -> ExplorerState {
        ExplorerState {
            nodes,
            expanded: HashSet::new(),
            query: query.to_string(),
            cursor: query.chars().count(),
            selected: 0,
            visible: Vec::new(),
            scroll: Cell::new(0),
            mode: if query.is_empty() {
                ExplorerMode::Selection
            } else {
                ExplorerMode::Filter
            },
            action: None,
            root: PathBuf::from("/tmp/vorto-test"),
            ignore: IgnoreOpts::DEFAULT,
            hidden_patterns: default_patterns(),
            max_items: 5000,
            error: None,
        }
    }

    #[test]
    fn empty_query_collapses_to_top_level() {
        let nodes = build_nodes(&paths(), &[]);
        let mut s = make_state(nodes, "");
        s.refilter();
        let visible_paths: Vec<&str> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.as_str())
            .collect();
        assert_eq!(visible_paths, vec!["src", "Cargo.toml"]);
    }

    #[test]
    fn query_filters_and_expands_ancestors() {
        let nodes = build_nodes(&paths(), &[]);
        let mut s = make_state(nodes, "list");
        s.refilter();
        let visible_paths: Vec<&str> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.as_str())
            .collect();
        assert_eq!(
            visible_paths,
            vec!["src", "src/ui", "src/ui/fuzzy", "src/ui/fuzzy/list.rs"]
        );
        // Ancestor dirs got auto-expanded so a follow-up empty query
        // wouldn't snap them shut.
        assert!(s.expanded.contains("src"));
        assert!(s.expanded.contains("src/ui"));
        assert!(s.expanded.contains("src/ui/fuzzy"));
    }
}
