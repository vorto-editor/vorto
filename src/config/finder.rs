//! `[finder]` settings — file picker and tree explorer behavior.
//!
//! ```toml
//! [finder]
//! hidden_patterns = [".*", "node_modules", "target", "dist", "build"]
//! max_items = 50_000
//! ```
//!
//! `hidden_patterns` controls what counts as "hidden" for both the
//! fuzzy file picker and the tree explorer. Each entry is a glob
//! pattern matched against the basename of each path component;
//! `.*` is the conventional shorthand for "all dotfiles." Patterns
//! only apply when the hidden filter is on (default), so the user can
//! still flip it off with the explorer's `.` key.
//!
//! `max_items` caps how many files the walker / `git ls-files` post-
//! filter will surface. Default 50_000 — enough to cover everything
//! short of very large monorepos, while keeping the per-keystroke
//! refilter under ~50ms in release builds. Lower it if interactive
//! filtering feels sluggish, raise it (up to ~100k comfortably) on
//! a fast machine.

use serde::Deserialize;

const DEFAULT_HIDDEN_PATTERNS: &[&str] = &[".*", "node_modules", "target", "dist", "build"];
const DEFAULT_MAX_ITEMS: usize = 50_000;

/// Resolved `[finder]` configuration. Constructed by overlaying
/// [`FinderToml`] (user input) over [`FinderConfig::default`].
#[derive(Debug, Clone)]
pub struct FinderConfig {
    /// Basename glob patterns that the file picker / explorer treat as
    /// hidden. See module docs.
    pub hidden_patterns: Vec<String>,
    /// Upper bound on how many entries the walker / `git ls-files`
    /// post-filter will surface. Shared between files and dirs so the
    /// explorer's tree doesn't out-grow the picker's flat list.
    pub max_items: usize,
}

impl Default for FinderConfig {
    fn default() -> Self {
        Self {
            hidden_patterns: DEFAULT_HIDDEN_PATTERNS
                .iter()
                .map(|s| (*s).into())
                .collect(),
            max_items: DEFAULT_MAX_ITEMS,
        }
    }
}

impl FinderConfig {
    /// Apply the user's `[finder]` overrides on top of the built-in
    /// defaults. Unset fields keep the default; explicitly set fields
    /// replace it wholesale (so the user can shrink or extend the
    /// pattern list without us trying to merge it).
    pub fn overlay(mut self, user: &FinderToml) -> Self {
        if let Some(p) = &user.hidden_patterns {
            self.hidden_patterns = p.clone();
        }
        if let Some(n) = user.max_items {
            self.max_items = n;
        }
        self
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct FinderToml {
    pub hidden_patterns: Option<Vec<String>>,
    pub max_items: Option<usize>,
}
