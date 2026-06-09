//! Filesystem mutations + pending-mode transitions for the explorer.
//!
//! Each op resolves paths against [`ExplorerState::root`], then calls
//! [`ExplorerState::refresh`] on success so the tree mirrors disk state.
//! The `enter_*` helpers move the widget into the transient prompt
//! modes (`a/d/r/m`, `/`).

use std::fs;
use std::path::{Path, PathBuf};

use super::{ActionInput, ExplorerMode, ExplorerState};

/// Outcome of [`ExplorerState::perform_create`]. Directories are
/// materialised on disk immediately (there's no buffer to defer them
/// to), but files are *not*: creating an empty 0-byte stub used to
/// crash language servers like tsserver — its filesystem watcher picks
/// up the stub and then trips a `Bad line number` assertion on the
/// first `didChange`. Instead we hand the caller the target path and
/// let it open a fresh buffer; the file (and any missing parent dirs)
/// materialises on the first save.
pub enum CreateResult {
    /// A directory was created on disk and the tree refreshed.
    Directory,
    /// Open a new, unsaved buffer at this absolute path. Nothing has
    /// touched disk yet.
    NewFile(PathBuf),
}

impl ExplorerState {
    // ── mode transitions ─────────────────────────────────────────

    /// Switch to filter mode. The query is preserved across switches so
    /// the user can toggle back to selection (Esc) without losing what
    /// they typed.
    pub fn enter_filter_mode(&mut self) {
        self.mode = ExplorerMode::Filter;
        self.cursor = self.query.chars().count();
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

    /// Create an entry at `rel` (relative to the workspace root). A
    /// trailing `/` makes a directory — materialised on disk now, since
    /// there's no buffer to defer it to. Anything else is a file:
    /// [`CreateResult::NewFile`] is returned *without* touching disk
    /// (see [`CreateResult`] for why), and the caller opens a buffer
    /// that writes the file — and any missing parent dirs — on save.
    pub fn perform_create(&mut self, rel: &str) -> Result<CreateResult, String> {
        let trimmed = rel.trim();
        if trimmed.is_empty() {
            return Err("empty path".into());
        }
        let is_dir = trimmed.ends_with('/');
        let clean = trimmed.trim_end_matches('/').to_string();
        // Reject anything that would let `root.join(clean)` escape the
        // workspace: `..` traversal, and absolute inputs — `Path::is_
        // absolute` catches Unix `/foo`, Windows `C:\foo`, and UNC
        // `\\server\share` (a bare `starts_with('/')` would miss the
        // latter two, since `join` ignores `root` for an absolute RHS).
        if clean.is_empty() || Path::new(&clean).is_absolute() || clean.contains("..") {
            return Err(format!("invalid path: {trimmed}"));
        }
        let target = self.root.join(&clean);
        if target.exists() {
            return Err(format!("already exists: {clean}"));
        }
        if is_dir {
            fs::create_dir_all(&target).map_err(|e| format!("mkdir: {e}"))?;
            self.refresh();
            self.select_by_path(&clean);
            Ok(CreateResult::Directory)
        } else {
            // Don't touch disk — the file and any missing parent dirs
            // materialise on the first save (see [`CreateResult`]). The
            // caller opens a fresh buffer; the tree picks the file up on
            // its next refresh after that save.
            Ok(CreateResult::NewFile(target))
        }
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::default_patterns;
    use super::*;
    use crate::finder::IgnoreOpts;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn scratch_root(tag: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "vorto-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn perform_create_file_defers_to_disk() {
        // Creating a file must NOT touch disk — neither the file nor any
        // implied parent dir. A 0-byte stub here used to crash tsserver
        // (its watcher picks it up, then asserts on the first didChange).
        let tmp = scratch_root("create-file");
        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        let out = s.perform_create("sub/new.ts").unwrap();
        match out {
            CreateResult::NewFile(p) => assert_eq!(p, tmp.join("sub/new.ts")),
            CreateResult::Directory => panic!("expected NewFile, got Directory"),
        }
        assert!(!tmp.join("sub/new.ts").exists(), "file must not be on disk");
        assert!(!tmp.join("sub").exists(), "parent dir must not be on disk");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn perform_create_dir_materializes() {
        // Directories have no buffer to defer to, so they're created now.
        let tmp = scratch_root("create-dir");
        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        let out = s.perform_create("newdir/").unwrap();
        assert!(matches!(out, CreateResult::Directory));
        assert!(tmp.join("newdir").is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn perform_create_rejects_escaping_paths() {
        // Inputs that would let `root.join` escape the workspace must be
        // refused — `..` traversal and absolute paths. `is_absolute`
        // covers the Unix `/abs` form here; on Windows it also catches
        // `C:\abs` and `\\server\share` that a `starts_with('/')` check
        // would have let through.
        let tmp = scratch_root("create-escape");
        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        for bad in ["/etc/passwd", "../outside.ts", "a/../../b.ts"] {
            assert!(
                s.perform_create(bad).is_err(),
                "should reject escaping path: {bad}"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn toggle_hidden_surfaces_dotfile_at_root() {
        // Repro for an explorer bug report: pressing `.` in selection
        // mode should reveal a tracked-but-dotfile entry like
        // `.gitignore`. Uses a real tmp dir so the workspace_files
        // walker actually runs end-to-end.
        let tmp = std::env::temp_dir().join(format!(
            "vorto-explorer-dotfile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(".gitignore"), "/target\n").unwrap();
        std::fs::write(tmp.join("README.md"), "# test\n").unwrap();
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "fn main() {}\n").unwrap();

        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        let initial_visible: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        assert!(
            !initial_visible.iter().any(|p| p == ".gitignore"),
            "dotfile should be hidden by default, got {:?}",
            initial_visible
        );

        s.apply_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

        let after: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(
            after.iter().any(|p| p == ".gitignore"),
            "after pressing `.`, dotfile should appear, got {:?}",
            after
        );
    }

    #[test]
    fn toggle_vcs_after_expanding_empty_gitignored_dir() {
        // Realistic user flow: navigate into a gitignored dir (which
        // appears empty at default settings), press `h` to flip vcs,
        // expect the files to materialise without having to re-expand.
        let tmp = std::env::temp_dir().join(format!(
            "vorto-explorer-expand-then-h-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(".gitignore"), "/scratch/\n").unwrap();
        std::fs::write(tmp.join("README.md"), "# test\n").unwrap();
        std::fs::create_dir_all(tmp.join("scratch")).unwrap();
        std::fs::write(tmp.join("scratch/hello.txt"), "hi\n").unwrap();
        let ok = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        let _ = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["add", "-A"])
            .status();

        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        // Step 1: user expands `scratch` (empty at this point).
        s.expanded.insert("scratch".into());
        s.refilter();
        let after_expand: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        eprintln!("expanded but vcs=true: {:?}", after_expand);

        // Step 2: user presses `h`.
        s.apply_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        let after_h: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        eprintln!("after h: {:?}", after_h);

        let _ = std::fs::remove_dir_all(&tmp);
        assert!(
            after_h.iter().any(|p| p == "scratch/hello.txt"),
            "expected scratch/hello.txt to appear after `h`, got {:?}",
            after_h
        );
    }

    #[test]
    fn toggle_vcs_surfaces_gitignored_dir_in_git_repo() {
        // Repro: in a git repo with `scratch/` listed in `.gitignore`,
        // the default explorer view hides files inside `scratch/`.
        // Pressing `h` should reveal them. Mirrors the
        // `/assets/samples/` setup in this repo.
        let tmp = std::env::temp_dir().join(format!(
            "vorto-explorer-gitignored-dir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(".gitignore"), "/scratch/\n").unwrap();
        std::fs::write(tmp.join("README.md"), "# test\n").unwrap();
        std::fs::create_dir_all(tmp.join("scratch")).unwrap();
        std::fs::write(tmp.join("scratch/hello.txt"), "hi\n").unwrap();
        let ok = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        let _ = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["add", "-A"])
            .status();

        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        let initial: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        eprintln!("DEFAULT visible: {:?}", initial);

        s.apply_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));

        // After toggle, expand the `scratch` dir so its child is in
        // visible — the toggle just rebuilds nodes, it doesn't auto-
        // expand previously-empty dirs.
        s.expanded.insert("scratch".into());
        s.refilter();
        let after: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        eprintln!("AFTER h + expand visible: {:?}", after);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(
            after.iter().any(|p| p == "scratch/hello.txt"),
            "after pressing `h` and expanding scratch, gitignored file should appear, got {:?}",
            after
        );
    }

    #[test]
    fn toggle_hidden_surfaces_dotfile_in_git_repo() {
        // Same repro but inside an actual git repo, which routes
        // workspace_files through `git ls-files` instead of the manual
        // walker.
        let tmp = std::env::temp_dir().join(format!(
            "vorto-explorer-dotfile-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(".gitignore"), "/target\n").unwrap();
        std::fs::write(tmp.join("README.md"), "# test\n").unwrap();
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), "fn main() {}\n").unwrap();
        // init + stage so .gitignore is tracked by `git ls-files --cached`
        let ok = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return; // git not available — skip
        }
        let _ = std::process::Command::new("git")
            .args(["-C"])
            .arg(&tmp)
            .args(["add", "-A"])
            .status();

        let mut s = ExplorerState::new(&tmp, IgnoreOpts::DEFAULT, default_patterns(), 5000, false);
        s.apply_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));

        let after: Vec<String> = s
            .visible
            .iter()
            .map(|&i| s.nodes[i].rel_path.clone())
            .collect();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(
            after.iter().any(|p| p == ".gitignore"),
            "in git repo, after pressing `.`, .gitignore should appear, got {:?}",
            after
        );
    }
}
