use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::types::{Types, TypesBuilder};
use ignore::{DirEntry, WalkBuilder};

/// Filter toggles for the fuzzy file picker / tree explorer. Both axes
/// are independent: `vcs` decides whether to honor ignore files, and
/// `hidden` decides whether to apply the configured
/// [`hidden_patterns`](crate::config::FinderConfig::hidden_patterns)
/// (defaults to dotfiles + heavy build dirs). The two filters compose,
/// so an entry that matches both rules requires both flags off to
/// surface — eg. `.cache/` (dotfile + gitignored) needs `.` and `h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IgnoreOpts {
    /// Honor ignore files the way Helix's file picker does:
    /// `.gitignore` (inside a git repo), `.git/info/exclude`, the
    /// global git excludesfile, `.ignore`, and the same rules found in
    /// parent directories of the root. When false only the
    /// `hidden_patterns` filter applies.
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

/// VCS metadata directories that are always pruned, even with both
/// filters off — the same list as Helix's `filter_picker_entry`.
/// Without this, `.git/objects/**` floods the picker the moment the
/// hidden filter is flipped off.
const VCS_DIRS: &[&str] = &[".git", ".pijul", ".jj", ".hg", ".svn"];

/// Archives the file picker skips, mirroring Helix's
/// `get_excluded_types`: the editor can't do anything useful with
/// them. The explorer still lists them so they can be moved/deleted.
const ARCHIVE_GLOB: &str = "*.{zip,gz,bz2,zst,lzo,sz,tgz,tbz2,lz,lz4,lzma,z,Z,xz,7z,rar,cab}";

/// Match a path basename against a glob pattern containing optional
/// `*` wildcards (each `*` matches zero or more characters). Anchored
/// on both ends — pattern `node_modules` matches only that exact name,
/// not `my_node_modules_old`. Pattern `.*` matches every dotfile.
///
/// Tiny ad-hoc matcher rather than a glob crate because the patterns
/// list is short and the call shape (one basename per walked entry)
/// doesn't benefit from a compiled matcher.
fn matches_glob(pattern: &str, name: &str) -> bool {
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
fn matches_any_hidden(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| matches_glob(p, name))
}

/// Per-entry prune rule shared by every walk. Returning false skips
/// the entry and, for a directory, everything under it.
///
/// - `hidden_patterns`: `Some` when the hidden filter is on.
/// - `dedup_root`: `Some(canonical root)` when symlinks are followed;
///   a link resolving back inside the root is dropped so its target
///   isn't listed twice (Helix's `deduplicate_links`).
fn keep_entry(
    entry: &DirEntry,
    hidden_patterns: Option<&[String]>,
    dedup_root: Option<&Path>,
) -> bool {
    // Never judge the root itself: a workspace opened at e.g.
    // `~/.dotfiles` would otherwise be pruned wholesale by `.*`.
    if entry.depth() == 0 {
        return true;
    }
    let Some(name) = entry.file_name().to_str() else {
        return false;
    };
    if VCS_DIRS.contains(&name) {
        return false;
    }
    if hidden_patterns.is_some_and(|p| matches_any_hidden(name, p)) {
        return false;
    }
    if let Some(root) = dedup_root
        && entry.path_is_symlink()
    {
        return entry
            .path()
            .canonicalize()
            .is_ok_and(|p| !p.starts_with(root));
    }
    true
}

/// Base walker configured like Helix's file picker: ignore files per
/// `vcs`, siblings sorted by file name (so the unfiltered list reads in
/// depth-first tree order), no depth limit. Dotfiles are left to
/// `hidden_patterns` (whose default includes `.*`) rather than the
/// walker's own hidden flag, so users can opt dotfiles back in via
/// config.
fn walker(
    root: &Path,
    vcs: bool,
    hidden_patterns: Option<&[String]>,
    dedup_root: Option<PathBuf>,
) -> WalkBuilder {
    let hidden_patterns: Option<Arc<[String]>> = hidden_patterns.map(Arc::from);
    let mut b = WalkBuilder::new(root);
    b.hidden(false)
        .parents(vcs)
        .ignore(vcs)
        .git_ignore(vcs)
        .git_global(vcs)
        .git_exclude(vcs)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |e| keep_entry(e, hidden_patterns.as_deref(), dedup_root.as_deref()));
    b
}

fn archive_types() -> Types {
    let mut t = TypesBuilder::new();
    t.add("archive", ARCHIVE_GLOB).expect("valid archive glob");
    t.negate("all");
    t.build().expect("valid archive types")
}

fn rel_string(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str())
        .map(str::to_owned)
}

/// Enumerate every file the file/workspace pickers should see, anchored
/// at `root`, in walk order (depth-first, siblings sorted by name —
/// the order Helix's picker shows before any query is typed).
///
/// Mirrors Helix's `file_picker` walk: ignore files per `ignore.vcs`,
/// symlinks followed (links back into the root deduplicated), archives
/// skipped, and only entries that resolve to a regular file kept — a
/// broken link or a link to a directory never reaches `Buffer::load`.
/// `hidden_patterns` applies when `ignore.hidden` is on; the result is
/// capped at `max_items`.
pub fn workspace_files(
    root: &Path,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) -> Vec<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    walker(
        root,
        ignore.vcs,
        ignore.hidden.then_some(hidden_patterns),
        Some(canonical_root),
    )
    .follow_links(true)
    .types(archive_types())
    .build()
    .filter_map(Result::ok)
    .filter(|e| e.path().is_file())
    .filter_map(|e| rel_string(root, e.path()))
    .take(max_items)
    .collect()
}

/// Explorer variant of [`workspace_files`]: same ignore rules, but
/// symlinks are neither followed nor listed (file ops on a link would
/// act on the link, not what the tree appears to show) and archives
/// stay visible.
pub fn explorer_files(
    root: &Path,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) -> Vec<String> {
    walker(
        root,
        ignore.vcs,
        ignore.hidden.then_some(hidden_patterns),
        None,
    )
    .build()
    .filter_map(Result::ok)
    .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
    .filter_map(|e| rel_string(root, e.path()))
    .take(max_items)
    .collect()
}

/// Enumerate every directory under `root` (excluding `root` itself) so
/// the explorer can expose empty directories as targets for new files.
/// Only the hidden filter applies — ignore files are deliberately not
/// consulted, so a gitignored dir stays visible (and expandable once
/// `h` flips the VCS filter). Symlinked directories are skipped.
pub fn workspace_dirs(
    root: &Path,
    ignore: IgnoreOpts,
    hidden_patterns: &[String],
    max_items: usize,
) -> Vec<String> {
    walker(root, false, ignore.hidden.then_some(hidden_patterns), None)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()))
        .filter_map(|e| rel_string(root, e.path()))
        .take(max_items)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn fresh_tmp(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "vorto-walk-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    fn patterns() -> Vec<String> {
        vec![".*".into(), "target".into()]
    }

    #[test]
    fn files_come_in_depth_first_name_order() {
        let root = fresh_tmp("order");
        for rel in ["b.txt", "a/z.rs", "a/b/c.rs", "a.txt", "src/main.rs"] {
            touch(&root, rel);
        }
        let files = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&root);
        // Siblings sorted by name, files and dirs interleaved — the
        // walk order, not a flat string sort.
        assert_eq!(
            files,
            vec!["a/b/c.rs", "a/z.rs", "a.txt", "b.txt", "src/main.rs"]
        );
    }

    #[test]
    fn hidden_patterns_and_vcs_dirs() {
        let root = fresh_tmp("hidden");
        for rel in [".env", "target/out", "src/lib.rs", ".git/HEAD", ".jj/repo"] {
            touch(&root, rel);
        }
        let on = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        assert_eq!(on, vec!["src/lib.rs"]);
        // Hidden filter off: dotfiles and `target` surface, VCS
        // metadata never does.
        let off = workspace_files(&root, IgnoreOpts::SHOW_HIDDEN, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(off, vec![".env", "src/lib.rs", "target/out"]);
    }

    #[test]
    fn hidden_root_is_not_pruned() {
        let parent = fresh_tmp("dotroot");
        let root = parent.join(".dotfiles");
        touch(&root, "init.lua");
        let files = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&parent);
        assert_eq!(files, vec!["init.lua"]);
    }

    #[test]
    fn ignore_file_and_archives() {
        let root = fresh_tmp("ignore");
        // `.ignore` applies without a git repo, unlike `.gitignore`.
        std::fs::write(root.join(".ignore"), "scratch/\n").unwrap();
        for rel in ["scratch/a.txt", "keep.rs", "dist.tar.gz", "pkg.zip"] {
            touch(&root, rel);
        }
        let picker = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        assert_eq!(picker, vec!["keep.rs"]);
        let no_vcs = IgnoreOpts {
            vcs: false,
            hidden: true,
        };
        let picker_all = workspace_files(&root, no_vcs, &patterns(), 100);
        assert_eq!(picker_all, vec!["keep.rs", "scratch/a.txt"]);
        // The explorer keeps archives.
        let explorer = explorer_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(explorer, vec!["dist.tar.gz", "keep.rs", "pkg.zip"]);
    }

    #[test]
    fn max_items_caps_result() {
        let root = fresh_tmp("cap");
        for rel in ["a", "b", "c", "d"] {
            touch(&root, rel);
        }
        let files = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 2);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(files, vec!["a", "b"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_followed_in_picker_only() {
        use std::os::unix::fs::symlink;
        let root = fresh_tmp("links");
        let outside = fresh_tmp("links-outside");
        touch(&root, "real.txt");
        touch(&outside, "ext.txt");
        // Link out of the root: followed, its files listed.
        symlink(&outside, root.join("ext")).unwrap();
        // Link back into the root: deduplicated.
        symlink(root.join("real.txt"), root.join("alias.txt")).unwrap();
        // Broken link and link to a directory: never listed as files.
        symlink(root.join("missing"), root.join("broken")).unwrap();

        let picker = workspace_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let explorer = explorer_files(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let dirs = workspace_dirs(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        assert_eq!(picker, vec!["ext/ext.txt", "real.txt"]);
        assert_eq!(explorer, vec!["real.txt"]);
        assert!(dirs.is_empty(), "symlinked dirs skipped, got {dirs:?}");
    }

    #[test]
    fn dirs_ignore_vcs_but_honor_hidden() {
        let root = fresh_tmp("dirs");
        std::fs::write(root.join(".ignore"), "scratch/\n").unwrap();
        for d in ["scratch", "empty", ".cache", "a/b"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let dirs = workspace_dirs(&root, IgnoreOpts::DEFAULT, &patterns(), 100);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(dirs, vec!["a", "a/b", "empty", "scratch"]);
    }
}
