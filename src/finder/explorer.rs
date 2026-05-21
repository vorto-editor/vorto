//! Tree-style file explorer state.
//!
//! Powers `<space>e`. The structure is built once at open time from
//! `workspace_files` (which honors `.gitignore` and the dotfile skip),
//! then the user navigates by toggling expand/collapse on directory
//! rows. A live fuzzy query box at the top filters the tree to files
//! whose path matches — and auto-expands every ancestor directory of a
//! matched file so the hit is reachable without manual chord work.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::IgnoreOpts;
use super::fuzzy::{fuzzy_match, workspace_files};

/// One row in the explorer's logical tree. The full node list is kept
/// in DFS pre-order so parents appear before children and every node
/// past index `i` whose `depth > nodes[i].depth` is a descendant of `i`
/// — that ordering is what lets the visible-row builder skip whole
/// subtrees by depth comparison rather than walking links.
#[derive(Debug, Clone)]
pub struct ExplorerNode {
    /// Basename shown in the row (`"fuzzy.rs"`, `"src"`).
    pub name: String,
    /// `true` for directories — controls the row glyph and the
    /// expand/collapse semantics. Submitting on a dir toggles
    /// expansion; submitting on a file opens it.
    pub is_dir: bool,
    /// Indent level. Top-level entries are depth 0.
    pub depth: usize,
    /// Path relative to the workspace root (`"src/finder/fuzzy.rs"`).
    /// Used as the expand-state key for dirs and the open target for
    /// files.
    pub rel_path: String,
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
}

impl ExplorerState {
    pub fn new(root: &Path, ignore: IgnoreOpts, _compact: bool) -> Self {
        let files = workspace_files(root, ignore);
        let nodes = build_nodes(&files);
        let mut s = Self {
            nodes,
            expanded: HashSet::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            visible: Vec::new(),
        };
        s.refilter();
        s
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

    pub fn selection(&self) -> Option<&ExplorerNode> {
        self.visible
            .get(self.selected)
            .and_then(|&i| self.nodes.get(i))
    }

    /// Toggle expand on the currently selected dir. No-op for files
    /// (and for empty visible lists). Returns true if anything changed
    /// so the caller can refilter.
    pub fn toggle_selected(&mut self) -> bool {
        let Some(&node_idx) = self.visible.get(self.selected) else {
            return false;
        };
        let n = &self.nodes[node_idx];
        if !n.is_dir {
            return false;
        }
        if self.expanded.contains(&n.rel_path) {
            self.expanded.remove(&n.rel_path);
        } else {
            self.expanded.insert(n.rel_path.clone());
        }
        true
    }

    pub fn move_down(&mut self) {
        if !self.visible.is_empty() {
            self.selected = (self.selected + 1).min(self.visible.len() - 1);
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// `h` / `Left` — collapse the current dir, or if we're already
    /// sitting on a closed dir / a file, jump to the parent row.
    pub fn collapse_or_parent(&mut self) {
        let Some(&node_idx) = self.visible.get(self.selected) else {
            return;
        };
        let n = &self.nodes[node_idx];
        if n.is_dir && self.expanded.contains(&n.rel_path) {
            self.expanded.remove(&n.rel_path);
            self.refilter();
            return;
        }
        // Find parent visible row: walk back until a node with smaller
        // depth.
        if n.depth == 0 {
            return;
        }
        for i in (0..self.selected).rev() {
            let node = &self.nodes[self.visible[i]];
            if node.depth < n.depth {
                self.selected = i;
                return;
            }
        }
    }

    /// `l` / `Right` — expand the current dir, then drop into its
    /// first child (if any). On a file or already-expanded dir, just
    /// step into the next row.
    pub fn expand_or_descend(&mut self) {
        let Some(&node_idx) = self.visible.get(self.selected) else {
            return;
        };
        // Copy fields out before the &mut self calls below; we no
        // longer need to hold a borrow on `nodes`.
        let (is_dir, rel_path, depth) = {
            let n = &self.nodes[node_idx];
            (n.is_dir, n.rel_path.clone(), n.depth)
        };
        if is_dir && !self.expanded.contains(&rel_path) {
            self.expanded.insert(rel_path);
            self.refilter();
            // Hop to the first child if there is one (next row with
            // greater depth).
            if let Some(&next) = self.visible.get(self.selected + 1)
                && self.nodes[next].depth > depth
            {
                self.selected += 1;
            }
            return;
        }
        self.move_down();
    }

    // ── query input ───────────────────────────────────────────────
    //
    // Mirrors the readline-ish subset used by the fuzzy picker so
    // typing behaviour is consistent across both prompts.

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

    pub fn apply_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => {
                // Inside the query field Left moves the cursor; on
                // empty-query we treat Left as the tree's
                // collapse-or-parent so the user can navigate without
                // first clicking out of the query.
                if self.query.is_empty() {
                    self.collapse_or_parent();
                } else {
                    self.cursor = self.cursor.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if self.query.is_empty() {
                    self.expand_or_descend();
                } else if self.cursor < self.char_len() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.char_len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Char('p') if ctrl => self.move_up(),
            KeyCode::Char('n') if ctrl => self.move_down(),
            KeyCode::Char('b') if ctrl => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl && self.cursor < self.char_len() => self.cursor += 1,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.char_len(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
    }
}

/// Build the DFS-ordered node list from the workspace's relative path
/// list. We synthesize a node for every intermediate directory the
/// paths imply (the path list is files-only), then emit dirs-first
/// then files within each directory level — the same order
/// `workspace_files` already sorts paths by, so the build is a single
/// linear pass over a BTreeMap-grouped view of the inputs.
fn build_nodes(files: &[String]) -> Vec<ExplorerNode> {
    // Map every (parent_dir, basename, is_dir) the input implies.
    // BTreeMap keeps siblings sorted alphabetically.
    let mut children_by_parent: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    for f in files {
        let parts: Vec<&str> = f.split('/').collect();
        let mut acc = String::new();
        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            children_by_parent
                .entry(acc.clone())
                .or_default()
                .entry((*part).to_string())
                .and_modify(|v| {
                    if !is_last {
                        *v = false; // mark as dir
                    }
                })
                .or_insert(is_last); // true if file, false if dir
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
        }
    }
    // Note above: the `or_insert` value is `is_last` (true = file),
    // but the `and_modify` flips a previously-file entry to dir when
    // we later see a deeper path under it. This shouldn't actually
    // happen with a real file list (a name can't be both), but the
    // defensive flip is cheap.
    let mut out = Vec::new();
    let mut stack: Vec<(String, usize)> = Vec::new();
    push_children(&mut out, &children_by_parent, "", 0, &mut stack);
    out
}

fn push_children(
    out: &mut Vec<ExplorerNode>,
    map: &BTreeMap<String, BTreeMap<String, bool>>,
    parent: &str,
    depth: usize,
    _stack: &mut Vec<(String, usize)>,
) {
    let Some(children) = map.get(parent) else {
        return;
    };
    // Dirs before files within the same level — the common file
    // explorer convention. `is_file` here is the BTreeMap value
    // (`true` = file, `false` = dir).
    let mut entries: Vec<(&String, &bool)> = children.iter().collect();
    entries.sort_by(|a, b| match (a.1, b.1) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(b.0),
    });
    for (name, is_file) in entries {
        let is_dir = !*is_file;
        let rel_path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", parent, name)
        };
        out.push(ExplorerNode {
            name: name.clone(),
            is_dir,
            depth,
            rel_path: rel_path.clone(),
        });
        if is_dir {
            push_children(out, map, &rel_path, depth + 1, _stack);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<String> {
        vec![
            "Cargo.toml".into(),
            "src/finder/fuzzy.rs".into(),
            "src/finder/mod.rs".into(),
            "src/ui/fuzzy/list.rs".into(),
        ]
    }

    #[test]
    fn build_nodes_dirs_before_files() {
        let n = build_nodes(&paths());
        let rels: Vec<&str> = n.iter().map(|x| x.rel_path.as_str()).collect();
        assert_eq!(
            rels,
            vec![
                "src",
                "src/finder",
                "src/finder/fuzzy.rs",
                "src/finder/mod.rs",
                "src/ui",
                "src/ui/fuzzy",
                "src/ui/fuzzy/list.rs",
                "Cargo.toml",
            ]
        );
        // Depths follow the slashes.
        let depths: Vec<usize> = n.iter().map(|x| x.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 2, 1, 2, 3, 0]);
    }

    #[test]
    fn empty_query_collapses_to_top_level() {
        let nodes = build_nodes(&paths());
        let mut s = ExplorerState {
            nodes,
            expanded: HashSet::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            visible: Vec::new(),
        };
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
        let nodes = build_nodes(&paths());
        let mut s = ExplorerState {
            nodes,
            expanded: HashSet::new(),
            query: "list".into(),
            cursor: 4,
            selected: 0,
            visible: Vec::new(),
        };
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
