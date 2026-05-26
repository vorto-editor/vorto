//! Pane layout + document-pool boundary.
//!
//! ## Design summary
//!
//! - **Documents** ([`crate::editor::Buffer`]) are the content model.
//!   They live in a shared pool, `App.documents`, keyed by
//!   [`crate::buffer_ref::BufferRef`]. A document is never owned by a
//!   pane. Documents shown in no pane are evicted to the compressed
//!   `App.sleeping` map.
//!
//! - **Sessions** ([`Editor`]) are per-pane: each carries a cursor /
//!   multi-cursor / mode and *names* its document via `Editor::doc`.
//!   The `editor_pane`'s session is `App.editor`; every other editor
//!   leaf's session lives in `App.pane_content` as
//!   [`PaneContent::Editor`], keyed by [`PaneId`]. One leaf may instead
//!   be the agent ([`PaneContent::Agent`]), backed by `App.agent`.
//!
//! - **Panes** are display regions (leaves of [`PaneLayout`]). Two
//!   panes can show the same document by holding two sessions whose
//!   `doc` refs are equal — same pooled `Buffer`, independent cursors.
//!   That's what makes `:split` over one buffer behave like vim's.
//!
//! - **Tabs** are not implemented yet. The design keeps them trivial to
//!   add later — a `Tab` would own a [`PaneLayout`], a `pane_content`
//!   map, and an `active_pane`; the document pool stays shared at the
//!   `App` level so a document can appear in any tab.

use std::collections::HashMap;

use crate::buffer_ref::BufferRef;
use crate::editor::Editor;

use super::{App, Toast};

/// Stable identifier for a pane. Minted once when the pane is opened
/// (initial buffer or new split) and stays attached to that on-screen
/// region until the pane is closed.
pub type PaneId = u32;

/// What an inactive (non-`editor_pane`) leaf shows. The active editor's
/// pane is *not* represented here — it's backed by `App.editor` /
/// `App.editor_pane` directly (hot-path field access). Every other leaf
/// of the layout has exactly one entry in `App.pane_content`.
pub enum PaneContent {
    /// An inactive editor session over a pooled document.
    Editor(Editor),
    /// The single in-app agent pane. A unit variant — the agent process
    /// itself lives in `App.agent`; this just marks the leaf that shows
    /// it (`App.agent_pane`).
    Agent,
}

/// Orientation of a split node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Side-by-side children (children laid out left → right).
    Vertical,
    /// Stacked children (children laid out top → bottom).
    Horizontal,
}

// `FocusDir` lives in `crate::action` so the action AST doesn't have
// to reach into `app::pane`. Re-exported here so the existing
// `pane::FocusDir` import path keeps working.
pub use crate::action::FocusDir;

/// Recursive tree describing how the buffer viewport is partitioned
/// into panes.
#[derive(Debug, Clone)]
pub enum PaneLayout {
    /// A single visible pane.
    Leaf(PaneId),
    /// A split with N (>= 2) children sharing the parent rect along
    /// `dir`. `ratios` is the same length as `children` and sums to
    /// approximately 1.0; renderers normalize before consuming.
    Split {
        dir: SplitDir,
        children: Vec<PaneLayout>,
        ratios: Vec<f32>,
    },
}

impl PaneLayout {
    /// Locate the leaf with the given id and return a mutable
    /// reference into the tree at that subtree.
    pub fn find_leaf_mut(&mut self, id: PaneId) -> Option<&mut PaneLayout> {
        match self {
            PaneLayout::Leaf(pid) if *pid == id => Some(self),
            PaneLayout::Leaf(_) => None,
            PaneLayout::Split { children, .. } => {
                for c in children {
                    if let Some(found) = c.find_leaf_mut(id) {
                        return Some(found);
                    }
                }
                None
            }
        }
    }

    /// Collect every leaf id in left-to-right / top-to-bottom traversal
    /// order. Used for `Ctrl-W w` cycle-window and for sanity checks.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneLayout::Leaf(id) => out.push(*id),
            PaneLayout::Split { children, .. } => {
                for c in children {
                    c.collect_leaves(out);
                }
            }
        }
    }

    /// Remove the leaf with the given id, collapsing any parent split
    /// that ends up with only one remaining child. Returns the id of a
    /// nearby surviving leaf — the caller uses it as the next "active"
    /// pane — or `None` when the removal would empty the tree (caller
    /// must handle that case before calling).
    pub fn remove_leaf(&mut self, target: PaneId) -> Option<PaneId> {
        enum RemoveResult {
            NotFound,
            RemoveSelf,
            Removed(Option<PaneId>),
        }
        fn rightmost_leaf(node: &PaneLayout) -> PaneId {
            match node {
                PaneLayout::Leaf(id) => *id,
                PaneLayout::Split { children, .. } => {
                    rightmost_leaf(children.last().expect("split has >= 1 child"))
                }
            }
        }
        fn walk(node: &mut PaneLayout, target: PaneId) -> RemoveResult {
            match node {
                PaneLayout::Leaf(id) if *id == target => RemoveResult::RemoveSelf,
                PaneLayout::Leaf(_) => RemoveResult::NotFound,
                PaneLayout::Split {
                    children, ratios, ..
                } => {
                    for i in 0..children.len() {
                        match walk(&mut children[i], target) {
                            RemoveResult::NotFound => continue,
                            RemoveResult::RemoveSelf => {
                                children.remove(i);
                                ratios.remove(i);
                                let sum: f32 = ratios.iter().sum();
                                if sum > 0.0 {
                                    for r in ratios.iter_mut() {
                                        *r /= sum;
                                    }
                                }
                                let neighbor = if children.is_empty() {
                                    None
                                } else {
                                    let pick = if i < children.len() { i } else { i - 1 };
                                    Some(rightmost_leaf(&children[pick]))
                                };
                                return RemoveResult::Removed(neighbor);
                            }
                            RemoveResult::Removed(n) => return RemoveResult::Removed(n),
                        }
                    }
                    RemoveResult::NotFound
                }
            }
        }
        let neighbor = match walk(self, target) {
            RemoveResult::Removed(n) => n,
            _ => return None,
        };
        collapse_singletons(self);
        neighbor
    }

    /// Replace this leaf with a 2-child Split. The existing leaf
    /// becomes one of the children; `new_id` is the new sibling.
    /// `place` chooses which side the existing pane ends up on.
    pub fn split_at(&mut self, dir: SplitDir, new_id: PaneId, place: SplitPlace) {
        let existing = std::mem::replace(self, PaneLayout::Leaf(0));
        let new = PaneLayout::Leaf(new_id);
        let (children, ratios) = match place {
            SplitPlace::After => (vec![existing, new], vec![0.5, 0.5]),
            SplitPlace::Before => (vec![new, existing], vec![0.5, 0.5]),
        };
        *self = PaneLayout::Split {
            dir,
            children,
            ratios,
        };
    }
}

/// Position of the existing pane relative to the new sibling when a
/// leaf is split into two.
#[derive(Debug, Clone, Copy)]
pub enum SplitPlace {
    /// Existing pane stays on the left/top, new pane on the right/bottom.
    After,
    /// Existing pane moves to the right/bottom, new pane on the left/top.
    #[allow(dead_code)]
    Before,
}

/// Fold any `Split` node that ended up with a single child into its
/// child. Runs after a `remove_leaf` so a tree like
/// `Split[Leaf(2)]` doesn't linger as a noop wrapper around `Leaf(2)`.
fn collapse_singletons(node: &mut PaneLayout) {
    loop {
        let collapsed = match node {
            PaneLayout::Leaf(_) => None,
            PaneLayout::Split { children, .. } if children.len() == 1 => Some(children.remove(0)),
            PaneLayout::Split { children, .. } => {
                for c in children.iter_mut() {
                    collapse_singletons(c);
                }
                None
            }
        };
        match collapsed {
            None => break,
            Some(replacement) => {
                *node = replacement;
            }
        }
    }
}

/// Per-frame pane rectangle, published by the UI after layout and read
/// by directional focus navigation. Standalone newtype so this module
/// stays free of any ratatui dependency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaneRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub type PaneRectMap = HashMap<PaneId, PaneRect>;

pub const INITIAL_PANE_ID: PaneId = 0;
pub const NEXT_PANE_ID_SEED: PaneId = 1;

// ────────────────────────────────────────────────────────────────────────
// App-side pane operations
// ────────────────────────────────────────────────────────────────────────

impl App {
    /// Open a new pane in direction `dir` alongside the currently-active
    /// pane. The new pane shares the same document as the current one
    /// (vim-style `:split`): no buffer clone, just a new `Leaf` in the
    /// tree and a new [`Editor`] session whose `doc` names the same
    /// pooled `BufferRef`. The shared document carries the content; each
    /// session carries its own cursor, so edits in either pane land on
    /// one underlying `Buffer` while the cursors stay independent.
    ///
    /// Focus moves to the new pane (matching vim's `:split` behaviour).
    /// The displaced session goes into `pane_editors` keyed by the old
    /// active pane id.
    pub fn split_window(&mut self, dir: SplitDir) {
        // Splitting clones the *editor* view, so it operates on the
        // active editor pane. The agent pane has no editor session to
        // displace — splitting it would corrupt the `editor_pane`
        // invariant — so jump focus back to the editor first.
        if self.active_pane == self.agent_pane.unwrap_or(u32::MAX) {
            self.active_pane = self.editor_pane;
        }
        self.split_window_quiet(dir);
        self.push_toast(Toast::info(format!(
            "split ({})",
            match dir {
                SplitDir::Vertical => "vertical",
                SplitDir::Horizontal => "horizontal",
            },
        )));
    }

    /// Split machinery without the user-facing toast, returning the new
    /// leaf's id. Used by `:split` (which adds the toast) and by the
    /// `:agent` pane opener (which provides its own messaging). Assumes
    /// the active pane is the editor pane (`split_window` enforces that).
    pub(super) fn split_window_quiet(&mut self, dir: SplitDir) -> PaneId {
        let new_pane_id = self.mint_pane_id();
        let active_pane_id = self.active_pane;
        let shared_ref = self.editor.doc.clone();
        // The new active session references the SAME pooled document
        // (no buffer clone) and starts at the current cursor, with no
        // extras / Normal mode. The displaced session — with its own
        // cursor — becomes the inactive pane, keyed by its pane id. The
        // two panes now edit one shared `Buffer` through independent
        // cursors.
        let mut new_active = Editor::for_doc(shared_ref);
        new_active.cursor = self.editor.cursor;
        let displaced = std::mem::replace(&mut self.editor, new_active);
        self.pane_content
            .insert(active_pane_id, PaneContent::Editor(displaced));
        let leaf = self
            .layout
            .find_leaf_mut(active_pane_id)
            .expect("active pane must be in the layout tree");
        leaf.split_at(dir, new_pane_id, SplitPlace::After);
        self.active_pane = new_pane_id;
        // The new leaf is the editor pane now; the displaced one moved
        // into `pane_content`.
        self.editor_pane = new_pane_id;
        new_pane_id
    }

    /// The session ([`Editor`]) driving pane `id` — the `editor_pane`'s
    /// is `App.editor`, every other *editor* leaf's lives in
    /// `pane_content`. Returns `None` when `id` isn't a known editor
    /// leaf (unknown id, or the agent leaf).
    pub fn editor_for_pane(&self, id: PaneId) -> Option<&Editor> {
        if id == self.editor_pane {
            return Some(&self.editor);
        }
        match self.pane_content.get(&id) {
            Some(PaneContent::Editor(ed)) => Some(ed),
            _ => None,
        }
    }

    /// The document a pane is showing, resolved through its session's
    /// `doc` ref into the pool. `None` when the pane is unknown or the
    /// pool invariant is violated (soft failure for the renderer
    /// rather than a panic).
    pub fn buffer_for_pane(&self, id: PaneId) -> Option<&crate::editor::Buffer> {
        let ed = self.editor_for_pane(id)?;
        self.documents.get(&ed.doc)
    }

    /// Does any *inactive* pane currently show `r`? Used when deciding
    /// whether a document leaving the active slot can move to
    /// `sleeping` (gone from every visible pane) or has to stay live in
    /// the pool because another pane still renders it.
    pub fn ref_used_by_inactive_pane(&self, r: &BufferRef) -> bool {
        self.pane_content.values().any(|c| match c {
            PaneContent::Editor(ed) => &ed.doc == r,
            PaneContent::Agent => false,
        })
    }

    /// Close the active pane. Removes the closing pane's session, makes
    /// a neighbour active by swapping its session into `App.editor`,
    /// and — if the closed pane's document is no longer shown by any
    /// remaining session — sleeps that document (moving it out of the
    /// pool). When another pane still shows it, the document stays live
    /// in the pool untouched.
    ///
    /// No-op (with a toast) when only one pane is left.
    pub fn close_window(&mut self) {
        if self.pane_count() <= 1 {
            self.push_toast(Toast::error("only one pane (use :q to quit)"));
            return;
        }
        let closing_id = self.active_pane;

        // Closing the agent pane: drop the leaf + its `Agent` marker and
        // focus a neighbour. The agent *process* (`App.agent`) is left
        // alive — reopening `:agent` re-attaches a pane to it.
        if Some(closing_id) == self.agent_pane {
            let neighbor = match self.layout.remove_leaf(closing_id) {
                Some(n) => n,
                None => {
                    self.push_toast(Toast::error("layout has no neighbour to close into"));
                    return;
                }
            };
            self.pane_content.remove(&closing_id);
            self.agent_pane = None;
            // Focus the neighbour. `focus_pane` no-ops when the neighbour
            // is already `editor_pane` (the common case); it does the
            // session swap when it's some other editor leaf.
            self.active_pane = self.editor_pane;
            self.focus_pane(neighbor);
            self.push_toast(Toast::info("agent pane closed"));
            return;
        }

        // Closing an editor pane needs another editor leaf to fall into.
        // Refuse before mutating the layout when this is the only editor
        // pane (e.g. the agent pane is the sole neighbour) — otherwise we
        // would remove the leaf, find no editor neighbour, and bail with
        // `editor_pane`/`active_pane` left pointing at the removed leaf.
        let has_other_editor_leaf = self
            .layout
            .leaves()
            .into_iter()
            .any(|id| id != closing_id && Some(id) != self.agent_pane);
        if !has_other_editor_leaf {
            self.push_toast(Toast::error("only editor pane (use :q to quit)"));
            return;
        }

        // The agent pane (if open and not the one closing) must not be
        // picked as the new editor pane — it's not an editor leaf.
        // `remove_leaf` returns a geometric neighbour; if that's the
        // agent, fall back to any other editor leaf.
        let neighbor = match self.layout.remove_leaf(closing_id) {
            Some(n) => n,
            None => {
                self.push_toast(Toast::error("layout has no neighbour to close into"));
                return;
            }
        };
        let editor_neighbor = if Some(neighbor) == self.agent_pane {
            self.layout
                .leaves()
                .into_iter()
                .find(|id| Some(*id) != self.agent_pane && *id != closing_id)
                .unwrap_or(neighbor)
        } else {
            neighbor
        };
        // In the editor-close path `active_pane == editor_pane ==
        // closing_id`, so the chosen neighbour is some *other* editor
        // leaf and lives in `pane_content`.
        let neighbour_ed = match self.pane_content.remove(&editor_neighbor) {
            Some(PaneContent::Editor(ed)) => ed,
            _ => {
                self.push_toast(Toast::error("no editor neighbour to close into"));
                return;
            }
        };
        let closing_ref = self.editor.doc.clone();
        // Make the neighbour active; the closing pane's session is
        // dropped (its cursor goes away — the document, if still shown,
        // survives in the pool).
        self.editor = neighbour_ed;
        self.editor_pane = editor_neighbor;
        self.active_pane = editor_neighbor;
        self.current_scratch_id = match &self.editor.doc {
            BufferRef::Scratch(id) => Some(*id),
            _ => None,
        };
        // Retire the closed document when nothing references it anymore.
        self.retire_doc_if_unreferenced(closing_ref);
        self.lsp.detach_current();
        self.lsp.set_last_synced_version(self.active_doc().version);
        // See `focus_pane` for why we skip the highlighter respawn in
        // the common case.
        if let Some(path) = self.active_doc().path.clone() {
            if self.active_doc().highlighter.is_none() {
                self.spawn_engine_worker(&path);
            }
            self.spawn_lsp_worker(&path);
        }
        self.push_toast(Toast::info("pane closed"));
    }

    /// Close the agent pane and drop the agent process. Called when the
    /// process exits (reader thread saw EOF): the pane disappears and
    /// `App.agent` is cleared. Unlike `:close` on the agent pane — which
    /// detaches but keeps the process alive for re-attach — this drops the
    /// process too. No-op when no agent pane is open.
    pub(crate) fn close_agent_pane(&mut self) {
        let Some(agent_pid) = self.agent_pane else {
            return;
        };
        let was_active = self.active_pane == agent_pid;
        let neighbor = self.layout.remove_leaf(agent_pid);
        self.pane_content.remove(&agent_pid);
        self.agent_pane = None;
        self.agent = None;
        if was_active {
            // `editor_pane` always backs `App.editor` and is a live leaf,
            // so it's a safe landing spot; focus the geometric neighbour
            // when there is one (mirrors the manual close path).
            self.active_pane = self.editor_pane;
            if let Some(n) = neighbor {
                self.focus_pane(n);
            }
        }
    }

    /// Move focus to the pane lying in the requested cardinal direction.
    /// Resolves against the rectangles computed by the UI on the last
    /// frame. No-op when no pane sits in that direction.
    pub fn focus_window(&mut self, dir: FocusDir) {
        let Some(target) = self.pane_in_direction(dir) else {
            return;
        };
        self.focus_pane(target);
    }

    /// Cycle to the next pane in tree-traversal order. Bound to
    /// `Ctrl-W w`.
    pub fn cycle_window(&mut self) {
        let leaves = self.layout.leaves();
        if leaves.len() <= 1 {
            return;
        }
        let idx = leaves
            .iter()
            .position(|id| *id == self.active_pane)
            .unwrap_or(0);
        let next = leaves[(idx + 1) % leaves.len()];
        self.focus_pane(next);
    }

    /// Number of leaves in the current layout. `1` means "no splits";
    /// the value drives the `:close` guard so we don't try to close the
    /// last visible pane (vim's `:q` is the right tool there).
    pub fn pane_count(&self) -> usize {
        self.layout.leaves().len()
    }

    /// Swap focus to `target`.
    ///
    /// Focusing the *agent* pane is just an `active_pane` change — the
    /// agent isn't an editor session, so `App.editor`/`editor_pane` stay
    /// put (the editor keeps rendering and the user can switch back).
    ///
    /// Focusing an *editor* pane swaps sessions: `App.editor` (backing
    /// the old `editor_pane`) is stashed into `pane_content`, the
    /// target's session moves into `App.editor`, and both `editor_pane`
    /// and `active_pane` become `target`. Documents stay put in the
    /// pool — two panes sharing a doc keep naming the same ref, each
    /// with its own cursor.
    pub(super) fn focus_pane(&mut self, target: PaneId) {
        if target == self.active_pane {
            return;
        }
        // Focusing the agent pane: just retarget `active_pane`. Leave
        // the editor session and `editor_pane` exactly where they are.
        if Some(target) == self.agent_pane {
            self.active_pane = target;
            return;
        }
        // Otherwise `target` must be an editor leaf. When it's the
        // current `editor_pane` (e.g. switching back from the agent),
        // the session is already `App.editor` — no swap, just retarget.
        if target == self.editor_pane {
            self.active_pane = target;
            return;
        }
        let Some(PaneContent::Editor(target_ed)) = self.pane_content.remove(&target) else {
            return;
        };
        let prev_editor_pane = self.editor_pane;
        let target_ref = target_ed.doc.clone();
        let prev_ed = std::mem::replace(&mut self.editor, target_ed);
        self.pane_content
            .insert(prev_editor_pane, PaneContent::Editor(prev_ed));
        self.editor_pane = target;
        self.active_pane = target;
        self.current_scratch_id = match &target_ref {
            BufferRef::Scratch(id) => Some(*id),
            _ => None,
        };
        self.lsp.detach_current();
        self.lsp.set_last_synced_version(self.active_doc().version);
        // The pooled document carries its existing highlighter, so the
        // common-case focus swap keeps syntax painted continuously.
        // Only respawn when it's missing one (rare — either the
        // open-time worker hadn't completed by the swap, or the
        // document's grammar wasn't available at open). Respawning
        // unconditionally would null the highlighter for a few frames
        // (see `spawn_engine_worker`) and flicker through plain text.
        if let Some(path) = self.active_doc().path.clone() {
            if self.active_doc().highlighter.is_none() {
                self.spawn_engine_worker(&path);
            }
            self.spawn_lsp_worker(&path);
        }
        self.record_opened(target_ref);
    }

    /// Pick the leaf-pane id that sits in `dir` relative to the active
    /// pane. Resolves against `last_pane_rects`, populated by the UI on
    /// the most recent draw.
    fn pane_in_direction(&self, dir: FocusDir) -> Option<PaneId> {
        let rects = self.last_pane_rects.borrow();
        let active = rects.get(&self.active_pane).copied()?;
        let active_cx = active.x + active.width / 2;
        let active_cy = active.y + active.height / 2;
        let mut best: Option<(PaneId, i32)> = None;
        for (&id, &rect) in rects.iter() {
            if id == self.active_pane {
                continue;
            }
            let matches_dir = match dir {
                FocusDir::Left => rect.x + rect.width <= active.x,
                FocusDir::Right => rect.x >= active.x + active.width,
                FocusDir::Up => rect.y + rect.height <= active.y,
                FocusDir::Down => rect.y >= active.y + active.height,
            };
            if !matches_dir {
                continue;
            }
            let cx = rect.x + rect.width / 2;
            let cy = rect.y + rect.height / 2;
            let dist: i32 = match dir {
                FocusDir::Left | FocusDir::Right => {
                    (cx as i32 - active_cx as i32).abs() * 2 + (cy as i32 - active_cy as i32).abs()
                }
                FocusDir::Up | FocusDir::Down => {
                    (cy as i32 - active_cy as i32).abs() * 2 + (cx as i32 - active_cx as i32).abs()
                }
            };
            match best {
                Some((_, b)) if dist >= b => {}
                _ => best = Some((id, dist)),
            }
        }
        best.map(|(id, _)| id)
    }

    pub(super) fn mint_pane_id(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        id
    }
}

#[cfg(test)]
mod tests {
    //! The new capability Phase B unlocks: one pooled document shown in
    //! two panes through two sessions with independent cursors. A full
    //! `App` is heavy to build in a unit test (config + LSP + worker
    //! channels), so this exercises the storage model directly — a
    //! `documents` pool plus two `Editor`s naming the same `BufferRef`,
    //! mirroring exactly what `split_window` sets up.

    use crate::buffer_ref::BufferRef;
    use crate::editor::{Buffer, Cursor, Editor};
    use std::collections::HashMap;

    use super::{PaneContent, PaneLayout};
    use crate::app::App;

    /// Build a minimal `App` for pane-juggling tests: default config,
    /// an empty grammar loader, a throwaway event channel, and the
    /// process cwd as the startup anchor. The initial layout is a single
    /// scratch editor pane.
    fn test_app() -> App {
        let config = crate::config::Config::load(None).expect("default config loads");
        let loader =
            crate::syntax::Loader::new(std::path::PathBuf::new(), std::path::PathBuf::new());
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        App::new(config, loader, tx, cwd)
    }

    /// Attach a fake agent pane: split a new leaf, convert it to the
    /// `Agent` marker, and restore the editor pane — mirroring
    /// `App::open_agent_pane` but without spawning a real process
    /// (`App.agent` stays `None`; the focus/close logic never touches
    /// it). Returns the agent leaf id, leaving it focused.
    fn attach_agent_pane(app: &mut App) -> super::PaneId {
        let prev_editor_pane = app.editor_pane;
        let new_leaf = app.split_window_quiet(super::SplitDir::Vertical);
        if let Some(PaneContent::Editor(prev_ed)) = app.pane_content.remove(&prev_editor_pane) {
            app.editor = prev_ed;
            app.editor_pane = prev_editor_pane;
        }
        app.pane_content.insert(new_leaf, PaneContent::Agent);
        app.agent_pane = Some(new_leaf);
        app.active_pane = new_leaf;
        new_leaf
    }

    #[test]
    fn focusing_agent_pane_leaves_editor_intact() {
        let mut app = test_app();
        let editor_pane = app.editor_pane;
        let editor_doc = app.editor.doc.clone();
        let agent_leaf = attach_agent_pane(&mut app);

        // After attaching, the agent pane is active but the editor pane
        // / session are unchanged.
        assert_eq!(app.active_pane, agent_leaf);
        assert_eq!(app.editor_pane, editor_pane);
        assert_eq!(app.editor.doc, editor_doc);
        assert!(matches!(
            app.pane_content.get(&agent_leaf),
            Some(PaneContent::Agent)
        ));
        // `editor_pane` must not be in `pane_content`.
        assert!(!app.pane_content.contains_key(&editor_pane));

        // Re-focusing the agent pane is a no-op for the editor.
        app.focus_pane(agent_leaf);
        assert_eq!(app.editor_pane, editor_pane);
        assert_eq!(app.editor.doc, editor_doc);
    }

    #[test]
    fn switching_back_from_agent_restores_editor_focus() {
        let mut app = test_app();
        let editor_pane = app.editor_pane;
        attach_agent_pane(&mut app);
        // Focus the editor pane again.
        app.focus_pane(editor_pane);
        assert_eq!(app.active_pane, editor_pane);
        assert_eq!(app.editor_pane, editor_pane);
    }

    #[test]
    fn closing_agent_pane_keeps_agent_and_clears_pane() {
        let mut app = test_app();
        let editor_pane = app.editor_pane;
        let editor_doc = app.editor.doc.clone();
        attach_agent_pane(&mut app);
        // Closing the focused agent pane removes the leaf + marker,
        // clears `agent_pane`, and focuses the editor — without touching
        // `App.agent` (the process, here `None`, would survive).
        assert_eq!(app.pane_count(), 2);
        app.close_window();
        assert_eq!(app.agent_pane, None);
        assert_eq!(app.pane_count(), 1);
        assert_eq!(app.active_pane, editor_pane);
        assert_eq!(app.editor_pane, editor_pane);
        assert_eq!(app.editor.doc, editor_doc);
        assert!(matches!(app.layout, PaneLayout::Leaf(id) if id == editor_pane));
        // The editor pane is the only leaf and is never in pane_content.
        assert!(app.pane_content.is_empty());
    }

    #[test]
    fn split_is_redirected_away_from_the_agent_pane() {
        let mut app = test_app();
        let editor_pane = app.editor_pane;
        let agent_leaf = attach_agent_pane(&mut app);
        // Splitting while the agent pane is focused must not consume the
        // agent leaf as an editor session; it splits the editor pane.
        app.split_window(super::SplitDir::Horizontal);
        // The new active pane is a fresh editor leaf, the agent pane is
        // untouched, and the editor invariant holds.
        assert_eq!(app.active_pane, app.editor_pane);
        assert_ne!(app.editor_pane, agent_leaf);
        assert!(matches!(
            app.pane_content.get(&agent_leaf),
            Some(PaneContent::Agent)
        ));
        // The previously-active editor session was stashed under its id.
        assert!(matches!(
            app.pane_content.get(&editor_pane),
            Some(PaneContent::Editor(_))
        ));
    }

    #[test]
    fn split_shares_document_with_independent_cursors() {
        let mut documents: HashMap<BufferRef, Buffer> = HashMap::new();
        let doc_ref = BufferRef::Scratch(0);
        let mut buf = Buffer::new();
        buf.lines = vec!["alpha".into(), "beta".into(), "gamma".into()];
        documents.insert(doc_ref.clone(), buf);

        // Two sessions over the SAME pooled document (the `split_window`
        // shape: same `doc` ref, independent cursors).
        let mut a = Editor::for_doc(doc_ref.clone());
        let mut b = Editor::for_doc(doc_ref.clone());
        a.cursor = Cursor { row: 0, col: 0 };
        b.cursor = Cursor { row: 2, col: 0 };

        // Pane A edits the shared document.
        {
            let doc = documents.get_mut(&a.doc).unwrap();
            a.insert_char(doc, 'X');
        }

        // The edit is visible through pane B's view of the SAME document…
        let shared = documents.get(&b.doc).unwrap();
        assert_eq!(shared.lines[0], "Xalpha");
        // …yet the two panes keep separate cursors: A advanced past the
        // inserted char, B is untouched on its own row.
        assert_eq!(a.cursor, Cursor { row: 0, col: 1 });
        assert_eq!(b.cursor, Cursor { row: 2, col: 0 });
    }
}
