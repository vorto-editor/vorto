//! Tree-style file explorer state.
//!
//! Powers `<space>e`. The structure is built once at open time from
//! `workspace_files` (which honors `.gitignore` and the dotfile skip),
//! then the user navigates by toggling expand/collapse on directory
//! rows. A live fuzzy query box at the top filters the tree to files
//! whose path matches — and auto-expands every ancestor directory of a
//! matched file so the hit is reachable without manual chord work.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::IgnoreOpts;
use super::fuzzy::{fuzzy_match, workspace_dirs, workspace_files};

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
    /// Last error message produced by a file op. Cleared on the next
    /// successful op or mode change.
    pub error: Option<String>,
}

impl ExplorerState {
    pub fn new(root: &Path, ignore: IgnoreOpts, _compact: bool) -> Self {
        let files = workspace_files(root, ignore);
        let dirs = workspace_dirs(root, ignore);
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
        let files = workspace_files(&self.root, self.ignore);
        let dirs = workspace_dirs(&self.root, self.ignore);
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
        match self.mode {
            ExplorerMode::Selection => self.apply_selection_key(key),
            ExplorerMode::Filter => self.apply_filter_key(key),
            ExplorerMode::PendingCreate
            | ExplorerMode::PendingRename
            | ExplorerMode::PendingMove => self.apply_action_input_key(key),
            ExplorerMode::PendingDelete => self.apply_delete_key(key),
        }
    }

    /// Selection mode — single-key navigation and op triggers. The
    /// arrow / Ctrl-N/P bindings remain too so users who reach for them
    /// out of habit still get the expected motion.
    fn apply_selection_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') if !ctrl => self.collapse_or_parent(),
            KeyCode::Right | KeyCode::Char('l') if !ctrl => self.expand_or_descend(),
            KeyCode::Up | KeyCode::Char('k') if !ctrl => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') if !ctrl => self.move_down(),
            KeyCode::Char('p') if ctrl => self.move_up(),
            KeyCode::Char('n') if ctrl => self.move_down(),
            KeyCode::Char('/') => self.enter_filter_mode(),
            KeyCode::Char('a') => self.enter_create_mode(),
            KeyCode::Char('d') => self.enter_delete_mode(),
            KeyCode::Char('r') => self.enter_rename_mode(),
            KeyCode::Char('m') => self.enter_move_mode(),
            _ => {}
        }
    }

    /// Filter mode — the legacy readline-style query input. Esc returns
    /// to selection mode (handled by the prompt controller, not here).
    fn apply_filter_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right if self.cursor < self.char_len() => self.cursor += 1,
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

    /// Shared key handler for the create/rename/move input prompts.
    /// Enter and Esc are intercepted upstream (the prompt controller
    /// needs filesystem access to submit, and Esc has to know which
    /// mode to fall back to).
    fn apply_action_input_key(&mut self, key: KeyEvent) {
        // Editing the input invalidates any sticky error from the
        // previous submission attempt — clear it so the user isn't
        // staring at a stale complaint while they fix the path.
        self.error = None;
        let Some(input) = self.action.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => input.cursor = input.cursor.saturating_sub(1),
            KeyCode::Right if input.cursor < input.char_len() => input.cursor += 1,
            KeyCode::Home => input.cursor = 0,
            KeyCode::End => input.cursor = input.char_len(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Char('b') if ctrl => input.cursor = input.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl && input.cursor < input.char_len() => input.cursor += 1,
            KeyCode::Char('a') if ctrl => input.cursor = 0,
            KeyCode::Char('e') if ctrl => input.cursor = input.char_len(),
            KeyCode::Char(c) if !ctrl => input.insert(c),
            _ => {}
        }
    }

    /// Delete confirmation: `y`/`Y` deletes via the controller, anything
    /// else (besides Enter/Esc, which are handled upstream) cancels.
    fn apply_delete_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Mark intent — the controller drains it after each key
                // event and runs the filesystem mutation.
            }
            _ => {
                self.cancel_pending();
            }
        }
    }

    // ── mode transitions ─────────────────────────────────────────

    /// Switch to filter mode. The query is preserved across switches so
    /// the user can toggle back to selection (Esc) without losing what
    /// they typed.
    pub fn enter_filter_mode(&mut self) {
        self.mode = ExplorerMode::Filter;
        self.cursor = self.char_len();
        self.error = None;
    }

    /// Return to selection mode from anywhere. Drops any in-flight
    /// action input but keeps the query string (filtering remains in
    /// effect; the user can re-enter filter mode with `/`).
    pub fn cancel_pending(&mut self) {
        self.mode = ExplorerMode::Selection;
        self.action = None;
        self.error = None;
    }

    /// `a` — open the create-path prompt. The seed text is the
    /// directory the new entry will land in (the selected dir, or the
    /// selected file's parent) plus a trailing `/` so the user can type
    /// the basename directly. A submitted path ending in `/` creates a
    /// directory; otherwise a file.
    pub fn enter_create_mode(&mut self) {
        let seed = self.create_seed();
        self.mode = ExplorerMode::PendingCreate;
        self.action = Some(ActionInput::new(&seed));
        self.error = None;
    }

    /// `r` — open the rename prompt, seeded with the selected entry's
    /// basename. No-op when nothing is selected.
    pub fn enter_rename_mode(&mut self) {
        let Some(name) = self.selection().map(|n| n.name.clone()) else {
            return;
        };
        self.mode = ExplorerMode::PendingRename;
        self.action = Some(ActionInput::new(&name));
        self.error = None;
    }

    /// `m` — open the move prompt, seeded with the selected entry's
    /// full rel path. The user edits to the destination rel path.
    pub fn enter_move_mode(&mut self) {
        let Some(rel) = self.selection().map(|n| n.rel_path.clone()) else {
            return;
        };
        self.mode = ExplorerMode::PendingMove;
        self.action = Some(ActionInput::new(&rel));
        self.error = None;
    }

    /// `d` — open the delete confirmation. No-op when nothing is
    /// selected.
    pub fn enter_delete_mode(&mut self) {
        if self.selection().is_none() {
            return;
        }
        self.mode = ExplorerMode::PendingDelete;
        self.action = None;
        self.error = None;
    }

    /// Compute the parent directory string the create prompt should
    /// seed into the input. Returns `""` when the user should type a
    /// top-level entry (selection is at depth 0 or nothing is
    /// selected); otherwise returns the path with a trailing `/`.
    fn create_seed(&self) -> String {
        let Some(node) = self.selection() else {
            return String::new();
        };
        if node.is_dir {
            // Inside the dir.
            format!("{}/", node.rel_path)
        } else if let Some(slash) = node.rel_path.rfind('/') {
            // Sibling of a file → parent dir + `/`.
            format!("{}/", &node.rel_path[..slash])
        } else {
            // File at the root → no prefix.
            String::new()
        }
    }

    // ── file operations ──────────────────────────────────────────
    //
    // All ops resolve paths against [`Self::root`], then call
    // [`Self::refresh`] on success so the tree mirrors disk state.

    /// Create a file or directory at `rel` (relative to the workspace
    /// root). A trailing `/` selects the directory variant; anything
    /// else creates an empty file. Missing parent directories are
    /// created either way.
    ///
    /// Returns the rel path of the created entry on success (for the
    /// caller to surface as the next selection / open intent).
    pub fn perform_create(&mut self, rel: &str) -> Result<String, String> {
        let trimmed = rel.trim();
        if trimmed.is_empty() {
            return Err("empty path".into());
        }
        let is_dir = trimmed.ends_with('/');
        let clean = trimmed.trim_end_matches('/').to_string();
        if clean.is_empty() || clean.starts_with('/') || clean.contains("..") {
            return Err(format!("invalid path: {trimmed}"));
        }
        let target = self.root.join(&clean);
        if target.exists() {
            return Err(format!("already exists: {clean}"));
        }
        if is_dir {
            fs::create_dir_all(&target).map_err(|e| format!("mkdir: {e}"))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
            }
            fs::File::create(&target).map_err(|e| format!("create: {e}"))?;
        }
        self.refresh();
        self.select_by_path(&clean);
        Ok(clean)
    }

    /// Delete the entry at `rel`. Directories are removed recursively
    /// (the confirmation prompt is the user-facing safety net).
    pub fn perform_delete(&mut self, rel: &str) -> Result<(), String> {
        if rel.is_empty() {
            return Err("nothing to delete".into());
        }
        let target = self.root.join(rel);
        let meta = fs::symlink_metadata(&target).map_err(|e| format!("stat: {e}"))?;
        if meta.file_type().is_dir() {
            fs::remove_dir_all(&target).map_err(|e| format!("rmdir: {e}"))?;
        } else {
            fs::remove_file(&target).map_err(|e| format!("rm: {e}"))?;
        }
        self.refresh();
        Ok(())
    }

    /// Rename the selected entry's basename to `new_name`. The parent
    /// directory stays the same — for cross-directory moves the user
    /// reaches for `m` instead.
    pub fn perform_rename(&mut self, old_rel: &str, new_name: &str) -> Result<String, String> {
        let new_name = new_name.trim();
        if new_name.is_empty() || new_name.contains('/') {
            return Err(format!("invalid name: {new_name}"));
        }
        let parent_rel = match old_rel.rfind('/') {
            Some(i) => &old_rel[..i],
            None => "",
        };
        let new_rel = if parent_rel.is_empty() {
            new_name.to_string()
        } else {
            format!("{parent_rel}/{new_name}")
        };
        self.fs_rename(old_rel, &new_rel)
    }

    /// Move the entry at `old_rel` to `new_rel`. Missing destination
    /// parents are created.
    pub fn perform_move(&mut self, old_rel: &str, new_rel: &str) -> Result<String, String> {
        let new_rel = new_rel.trim().trim_end_matches('/');
        if new_rel.is_empty() || new_rel.starts_with('/') || new_rel.contains("..") {
            return Err(format!("invalid path: {new_rel}"));
        }
        self.fs_rename(old_rel, new_rel)
    }

    fn fs_rename(&mut self, old_rel: &str, new_rel: &str) -> Result<String, String> {
        if old_rel == new_rel {
            return Err("source and destination are the same".into());
        }
        let src = self.root.join(old_rel);
        let dst = self.root.join(new_rel);
        if dst.exists() {
            return Err(format!("already exists: {new_rel}"));
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        fs::rename(&src, &dst).map_err(|e| format!("rename: {e}"))?;
        self.refresh();
        self.select_by_path(new_rel);
        Ok(new_rel.to_string())
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
fn build_nodes(files: &[String], dirs: &[String]) -> Vec<ExplorerNode> {
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

    fn make_state(nodes: Vec<ExplorerNode>, query: &str) -> ExplorerState {
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
    fn selection_mode_swallows_chars_no_query_change() {
        // Confirms `j` / `a` / random text in Selection mode never
        // leaks into the query input — the failure mode the user hit
        // when this initially shipped.
        let nodes = build_nodes(&paths(), &[]);
        let mut s = make_state(nodes, "");
        assert_eq!(s.mode, ExplorerMode::Selection);
        s.apply_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        s.apply_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(s.query, "");
        assert_eq!(s.mode, ExplorerMode::Selection);
    }

    #[test]
    fn slash_enters_filter_mode() {
        let nodes = build_nodes(&paths(), &[]);
        let mut s = make_state(nodes, "");
        s.apply_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(s.mode, ExplorerMode::Filter);
        s.apply_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(s.query, "l");
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
