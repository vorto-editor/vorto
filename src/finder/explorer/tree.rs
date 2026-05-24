//! Tree model + navigation for the explorer.
//!
//! Holds the [`ExplorerNode`] row type, the node-list builder
//! (`build_nodes` / `push_children`), and the selection-movement
//! methods on [`ExplorerState`].

use std::collections::BTreeMap;

use super::ExplorerState;

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

impl ExplorerState {
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

    /// Best-effort: move the cursor to the row whose rel_path matches.
    /// Walks ancestors and auto-expands them so the target row is
    /// visible. No-op when the path isn't in the tree yet (caller
    /// should refresh first).
    pub fn select_by_path(&mut self, rel: &str) {
        // Expand every ancestor so the target row materializes.
        let mut prefix = rel;
        while let Some(slash) = prefix.rfind('/') {
            prefix = &prefix[..slash];
            self.expanded.insert(prefix.to_string());
        }
        self.refilter();
        if let Some(pos) = self
            .visible
            .iter()
            .position(|&i| self.nodes[i].rel_path == rel)
        {
            self.selected = pos;
        }
    }
}

/// Build the DFS-ordered node list from the workspace's relative path
/// list. We synthesize a node for every intermediate directory the
/// files imply, then merge in `dirs` (explicit directory paths, which
/// is how empty directories show up — `workspace_files` only sees
/// files). Within each level dirs come before files, matching the
/// fuzzy picker's sort.
pub(super) fn build_nodes(files: &[String], dirs: &[String]) -> Vec<ExplorerNode> {
    // Map every (parent_dir, basename, is_dir) the input implies.
    // BTreeMap keeps siblings sorted alphabetically.
    // Value semantics: `true` = file, `false` = dir.
    let mut children_by_parent: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    // Register explicit directories first so a dir without any files
    // underneath still has an entry. We mark every path segment as a
    // dir (false) — the file pass below can never flip a dir entry
    // back to a file because its `and_modify` only writes `false`.
    for d in dirs {
        let parts: Vec<&str> = d.split('/').collect();
        let mut acc = String::new();
        for part in &parts {
            children_by_parent
                .entry(acc.clone())
                .or_default()
                .insert((*part).to_string(), false);
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
        }
    }
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
    use super::super::tests::paths;
    use super::*;

    #[test]
    fn build_nodes_dirs_before_files() {
        let n = build_nodes(&paths(), &[]);
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
}
