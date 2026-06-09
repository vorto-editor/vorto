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

/// True if any segment of `rel` (a `/`-separated relative path) matches
/// `patterns`. Used to post-filter `git ls-files` output, where we get
/// full paths rather than walked entries — a tracked file inside a
/// hidden-pattern directory still counts as hidden.
fn rel_path_has_hidden_segment(rel: &str, patterns: &[String]) -> bool {
    rel.split('/').any(|seg| matches_any_hidden(seg, patterns))
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
            .filter(|p| is_live_nonsymlink(&root.join(p)))
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

/// True if `path` exists on disk and is not a symlink (without
/// following it). `git ls-files --cached` keeps listing files that were
/// deleted from the work tree but whose deletion hasn't been staged, so
/// without the existence check the explorer would surface ghost entries
/// that survive a `refresh()` forever; dropping non-existent paths keeps
/// the tree in sync with the actual filesystem. Symlinks are filtered
/// out of the picker because opening one whose target is a directory or
/// broken propagates an `io::Error` from `Buffer::load` up to the main
/// loop and terminates the editor.
fn is_live_nonsymlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| !m.file_type().is_symlink())
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
}
