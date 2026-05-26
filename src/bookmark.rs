//! Harpoon-style bookmarks: a small, manually-curated, ordered list of
//! places (buffer + line) the user can jump back to via the `<space>mm`
//! picker.
//!
//! Persistence mirrors harpoon's model rather than vorto's config: a
//! single global JSON file under the XDG **state** dir
//! (`$XDG_STATE_HOME/vorto/bookmarks.json`, else
//! `$HOME/.local/state/vorto/bookmarks.json`), namespaced by project
//! root — so bookmarks travel with the project without littering the
//! repo, and never end up in git. Only file-backed marks survive a
//! restart; scratch-buffer marks ([`BufferRef::Scratch`]) are
//! session-only (their ids aren't stable across runs) and are skipped
//! when writing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::buffer_ref::BufferRef;

/// One bookmark: which buffer, and the line to land on (0-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub target: BufferRef,
    /// 0-based row, captured at mark time. Harpoon-style: not tracked
    /// through edits, so it can drift if the file changes — accepted.
    pub line: usize,
}

/// On-disk shape for a single mark. Only [`BufferRef::File`] marks are
/// ever serialized, so the buffer is just a path.
#[derive(Debug, Serialize, Deserialize)]
struct StoredMark {
    path: String,
    line: usize,
}

/// The current project's bookmark list plus the persistence key.
pub struct BookmarkStore {
    /// Marks in user order. Drives the picker; mutated by add/remove.
    pub marks: Vec<Bookmark>,
    /// Absolute project root, used as the key in the global JSON file.
    root: PathBuf,
}

impl BookmarkStore {
    /// Load the marks recorded for the project rooted at (or above)
    /// `startup_cwd`. Missing/corrupt state yields an empty list — a
    /// bookmark file is never load-bearing enough to fail startup.
    pub fn load(startup_cwd: &Path) -> Self {
        let root = project_root(startup_cwd);
        let marks = read_all()
            .remove(&root_key(&root))
            .unwrap_or_default()
            .into_iter()
            .map(|m| Bookmark {
                target: BufferRef::File(PathBuf::from(m.path)),
                line: m.line,
            })
            .collect();
        Self { marks, root }
    }

    /// Add a mark for `target` at `line`. Harpoon-style dedup: a buffer
    /// already in the list is a no-op (returns `false`) so re-marking
    /// the same file doesn't pile up duplicates or move its slot.
    pub fn add(&mut self, target: BufferRef, line: usize) -> bool {
        if self.marks.iter().any(|m| m.target == target) {
            return false;
        }
        self.marks.push(Bookmark { target, line });
        self.persist();
        true
    }

    /// Drop the mark for `target`, if present, and persist.
    pub fn remove_target(&mut self, target: &BufferRef) {
        let before = self.marks.len();
        self.marks.retain(|m| &m.target != target);
        if self.marks.len() != before {
            self.persist();
        }
    }

    /// Rewrite this project's entry in the global file, keeping every
    /// other project's marks verbatim. Only file-backed marks are
    /// written; scratch marks are session-only. Best-effort: a write
    /// failure is logged but never surfaced to the caller.
    fn persist(&self) {
        let Some(path) = bookmarks_path() else {
            return;
        };
        let mut all = read_all();
        let stored: Vec<StoredMark> = self
            .marks
            .iter()
            .filter_map(|m| match &m.target {
                BufferRef::File(p) => Some(StoredMark {
                    path: p.to_string_lossy().into_owned(),
                    line: m.line,
                }),
                BufferRef::Scratch(_) => None,
            })
            .collect();
        if stored.is_empty() {
            all.remove(&root_key(&self.root));
        } else {
            all.insert(root_key(&self.root), stored);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&all) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    crate::vlog!("bookmark persist: write {}: {e}", path.display());
                }
            }
            Err(e) => crate::vlog!("bookmark persist: serialize: {e}"),
        }
    }
}

/// Read the whole project→marks map. Absent or unparseable file → empty.
fn read_all() -> BTreeMap<String, Vec<StoredMark>> {
    let Some(path) = bookmarks_path() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Stable string key for a project root.
fn root_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// The project root used to namespace bookmarks: the nearest ancestor
/// of `startup_cwd` containing a `.git` entry, else `startup_cwd`
/// itself. Canonicalized so the key doesn't depend on how the path was
/// spelled. Matches harpoon's "per-project list" intent while being
/// launch-dir-independent (marking from a subdir keys the same list).
fn project_root(startup_cwd: &Path) -> PathBuf {
    let start = startup_cwd
        .canonicalize()
        .unwrap_or_else(|_| startup_cwd.to_path_buf());
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
    }
    start
}

/// `$XDG_STATE_HOME/vorto/bookmarks.json`, else
/// `$HOME/.local/state/vorto/bookmarks.json`. Mirrors
/// [`crate::log::default_path`]'s state-dir resolution.
fn bookmarks_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return Some(PathBuf::from(xdg).join("vorto").join("bookmarks.json"));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("vorto")
            .join("bookmarks.json"),
    )
}
