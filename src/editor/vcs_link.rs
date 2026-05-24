//! Buffer ↔ VCS bridge. The `vcs_base` (HEAD blob lines) and
//! `vcs_diff` (cached per-line status) fields live on `Buffer`; the
//! lookup logic that drives them lives here so [`super`] stays focused
//! on the document model.

use super::Buffer;
use crate::vcs::{self, LineStatus};

impl Buffer {
    /// Per-line VCS statuses, recomputed if the cached version is
    /// stale. Returns an empty slice when this buffer has no base
    /// (not in a git repo, or no path).
    pub fn vcs_statuses(&self) -> Vec<Option<LineStatus>> {
        let Some(base) = self.vcs_base.as_ref() else {
            return Vec::new();
        };
        {
            let cache = self.vcs_diff.borrow();
            if let Some((v, statuses)) = cache.as_ref()
                && *v == self.version
            {
                return statuses.clone();
            }
        }
        let statuses = vcs::diff_line_status(base, &self.lines);
        *self.vcs_diff.borrow_mut() = Some((self.version, statuses.clone()));
        statuses
    }

    /// True when this buffer differs from HEAD (any line marker is
    /// present). Cheap when the cache is hot; otherwise triggers a
    /// recompute. Returns false for buffers without a base.
    pub fn has_vcs_changes(&self) -> bool {
        if self.vcs_base.is_none() {
            return false;
        }
        self.vcs_statuses().iter().any(|s| s.is_some())
    }
}
