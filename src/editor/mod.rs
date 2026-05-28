//! Document model: `Buffer` (lines + cursor + undo + yank) and the
//! basic state lifecycle (load / save / version bump).
//!
//! Behaviour is split across siblings so this file stays focused on
//! state:
//!
//! - [`cursor`] — single-step cursor primitives (`h`/`j`/`k`/`l`,
//!   line/file edges, column clamp, `advance_one`).
//! - [`motion`] — word/paragraph/find/viewport motions and the shared
//!   [`Buffer::motion_target`] entry point.
//! - [`text_object`] — `iw`/`ip`/`i(` etc. resolution.
//! - [`ops`] — range/line/block delete + yank + paste, plus line-level
//!   edits (`J`, `D`, `S`, `~`, comment toggle).
//! - [`insert`] — typing, newline, opener/closer auto-pair, dedent,
//!   single-char delete primitives (`x`, backspace).
//! - [`search`] — `/`/`?` find-next state and lookup over the buffer.
//! - [`history`] — undo / redo snapshot stacks.
//! - [`vcs_link`] — HEAD-blob diff bridge driving the gutter VCS bars.

pub mod conflict;
mod cursor;
pub mod fold;
mod history;
mod inline_suggestion;
mod insert;
mod jumplist;
mod merge;
mod motion;
mod ops;
mod search;
mod substitute;
mod surround;
mod text_object;
mod vcs_link;

/// Per-buffer indent-guide animation state.
///
/// `started_at = Some(t)` means an animation is in flight from `t`;
/// `None` means the cursor's current scope has already played its
/// animation and is now static (kept so we can detect when the
/// cursor moves into a *different* scope and restart from zero).
/// `scope_key = (start_row, end_row, col)` is enough to detect a
/// scope change without holding a reference to the tree.
/// `anchor_row` is the cursor row at animation start.
pub type IndentAnimState = (Option<std::time::Instant>, (usize, usize, usize), usize);

pub use inline_suggestion::{RequestId, Suggestion, SuggestionState};
pub use jumplist::{JumpEntry, JumpList};
pub use ops::{flip_case_char_keep_width, to_lower_keep_width, to_upper_keep_width};
pub use search::SearchState;
pub use substitute::{SubsArgs, parse_substitute};

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

use crate::buffer_ref::BufferRef;
use crate::mode::Mode;
use crate::syntax::Engine;
use crate::vcs::{self, LineStatus};

#[derive(Default)]
pub struct Buffer {
    pub lines: Vec<String>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub yank: String,
    /// Monotonically increases on every content-modifying call. Used by
    /// the highlighter to decide whether its cached tree is stale.
    pub version: u64,
    /// Per-buffer tree-sitter state, attached at file-open time when a
    /// matching grammar + query are available. `None` means "no syntax
    /// highlighting for this buffer", which is the safe fallback.
    pub highlighter: Option<Engine>,
    /// Topmost line currently visible in the viewport. Sticky — only
    /// moved when the cursor would otherwise leave the viewport (the
    /// UI layer updates it during `draw_buffer`, so it's wrapped in
    /// `Cell` to stay reachable through a shared `&Buffer`).
    pub scroll: Cell<usize>,
    /// Leftmost visual column currently visible. Sticky like `scroll`:
    /// the UI shifts it during `draw_buffer` only when the cursor would
    /// otherwise leave the horizontal viewport.
    pub col_scroll: Cell<usize>,
    /// Height (in rows) of the buffer viewport at the last draw. The
    /// UI writes this during `compute_scroll`; motion code reads it
    /// for `H`/`M`/`L` and `<C-d>`/`<C-u>`/`<C-f>`/`<C-b>`. `0` until
    /// the first frame is drawn — motions guard against that.
    pub viewport_height: Cell<usize>,
    /// Set by `run_scroll(Center)` when the viewport height isn't known
    /// yet (e.g. right after switching to a sleeping buffer whose
    /// `viewport_height` thawed back to 0). The next `compute_scroll`
    /// in `draw_buffer` reads-and-clears this and centers the cursor
    /// instead of running the sticky scroll logic — so the user always
    /// lands mid-viewport on a picker-driven jump, even when the
    /// height-aware path couldn't fire yet.
    pub pending_center: Cell<bool>,
    /// Visual y (within the buffer viewport) of the row the cursor sits
    /// on at the last draw. Differs from `cursor.row - scroll` when
    /// inline diagnostics push subsequent rows down. The UI writes this
    /// in `draw_buffer`; `place_cursor` and cursor-anchored overlays
    /// read it.
    pub cursor_visual_y: Cell<u16>,
    /// Active indent-guide animation state. Reset whenever the
    /// cursor enters a different scope; cleared by the renderer
    /// once progress reaches 1.0 so a static frame doesn't keep
    /// waking the loop.
    pub indent_anim: Cell<Option<IndentAnimState>>,
    // `pub` so the sleeping-buffer freezer can take the stacks
    // by move (and reinstall them on thaw) without going through
    // accessor boilerplate. Editor-internal mutations still go
    // through the `snapshot` / `undo` / `redo` methods.
    pub undo_stack: Vec<Snapshot>,
    pub redo_stack: Vec<Snapshot>,
    /// HEAD blob lines captured at file-open time. `None` when the
    /// buffer isn't backed by a file inside a git repo. `Some(empty)`
    /// when the file is in a repo but not yet tracked at HEAD — every
    /// current line will diff as `Added`.
    pub vcs_base: Option<Vec<String>>,
    /// Cached `(version, per-line status)` produced by diffing
    /// `vcs_base` against `lines`. Recomputed lazily when `version`
    /// moves; wrapped in `RefCell` so the UI can refresh it through
    /// the shared `&Buffer` it gets at draw time.
    pub vcs_diff: RefCell<Option<(u64, Vec<Option<LineStatus>>)>>,
    /// Cached `(version, parsed git conflict hunks)`. Recomputed lazily
    /// when `version` moves — the renderer calls [`Self::conflict_hunks`]
    /// every frame (per visible pane), so the version gate keeps the
    /// per-line marker scan off the hot path. `RefCell` for the same
    /// reason as `vcs_diff`: the UI refreshes it through a shared
    /// `&Buffer`.
    pub conflict_cache: RefCell<Option<(u64, Vec<conflict::Hunk>)>>,
    /// Filesystem signature `(mtime, len)` captured the last time
    /// we touched the backing file — at load, after a successful
    /// save, and after `:reload`. `None` for scratch buffers and
    /// for new files that haven't been written yet. The runtime
    /// checks this before `:w` to refuse silently clobbering an
    /// external edit.
    pub disk_meta: Option<FileMeta>,
    /// The last content we saw on disk — captured at load, after a
    /// successful save, and after `:reload`/auto-merge. Serves as the
    /// common ancestor for the three-way merge that `autoreload = "merge"`
    /// runs against an external edit. `None` for scratch buffers and new
    /// files not yet written (mirrors `disk_meta`).
    pub disk_base: Option<Vec<String>>,
}

/// Outcome of [`Editor::merge_from_disk`] — how an external edit was
/// reconciled with the buffer's unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Disk already matched the buffer; only the baseline was refreshed.
    Unchanged,
    /// External edits merged cleanly into the buffer's unsaved changes.
    Clean,
    /// Overlapping edits left `<<<<<<<` markers in the buffer to resolve.
    Conflict,
}

/// Filesystem signature used to detect external edits between
/// load/save and the next save. `len` is what `Metadata::len()`
/// returns; `mtime` is `Metadata::modified()`. Both are cheap to
/// fetch and together catch the overwhelming majority of out-of-band
/// edits — a tool that rewrites a file with the same byte count *and*
/// preserves mtime to nanosecond precision will slip through, but
/// that combination is vanishingly rare in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub mtime: SystemTime,
    pub len: u64,
}

impl FileMeta {
    /// Fetch `(mtime, len)` for `path`. Returns `None` when the file
    /// doesn't exist, isn't a regular file the OS will stat, or the
    /// platform refuses to report modification time. Callers treat
    /// `None` as "no baseline to compare against" and skip the drift
    /// check rather than refusing to save.
    pub fn of(path: &Path) -> Option<Self> {
        let md = fs::metadata(path).ok()?;
        let mtime = md.modified().ok()?;
        Some(Self {
            mtime,
            len: md.len(),
        })
    }
}

/// Frozen buffer state for the undo/redo history. Exposed at the
/// crate boundary so the sleeping-buffer compressor can destructure
/// individual snapshots when it freezes a buffer; the editor module
/// itself still owns all the read/write logic.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub lines: Vec<String>,
    pub cursor: Cursor,
    /// Multi-cursor extras at snapshot time. Empty when there are no
    /// extras (the common case). Undo restores them along with the
    /// primary cursor so the multi-cursor state round-trips.
    pub extra_cursors: Vec<Cursor>,
    pub dirty: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// Knobs the buffer needs to produce an indent string for a freshly
/// inserted line. `width` is the spaces-per-level fallback when a level
/// is added in spaces. `use_tabs` is the tie-breaker when the reference
/// line carries no indent of its own (empty file, top-level statement);
/// when the reference line *does* have leading whitespace, that style is
/// preserved so we don't mix tabs and spaces within a file.
#[derive(Debug, Clone, Copy)]
pub struct IndentSettings {
    pub width: usize,
    pub use_tabs: bool,
}

impl Default for IndentSettings {
    fn default() -> Self {
        Self {
            width: 4,
            use_tabs: false,
        }
    }
}

/// Per-pane editing session. *References* its document — the [`Buffer`]
/// itself lives in the shared document pool (`App.documents`), named by
/// [`Self::doc`] — plus the cursor / multi-cursor / mode state that's
/// specific to *this* view of the document, and the in-flight command
/// token stream. Two panes can name the same document through equal
/// `doc` refs while keeping independent cursors.
#[derive(Default)]
pub struct Editor {
    /// The document this session is editing, named by its pool key.
    pub doc: BufferRef,
    /// Primary cursor.
    pub cursor: Cursor,
    /// Additional cursor positions for multi-cursor editing. The primary
    /// cursor lives in `cursor`; extras are *only* the non-primary ones,
    /// stored in insertion order so a pop semantic ("remove last added")
    /// is a simple `pop()`. Empty in the single-cursor common case.
    pub extra_cursors: Vec<Cursor>,
    /// Vim editing mode for this view. Defaults to `Normal`.
    pub mode: Mode,
    /// Accumulated command tokens since the last command fired. Cleared
    /// on Complete dispatch or Invalid parse.
    pub tokens: Vec<crate::action::Token>,
    /// This pane's jump history (vim's jumplist). Rides along with the
    /// session across buffer switches — the document swaps underneath but
    /// the session (and its history) persists — so `Ctrl-O` can step back
    /// into a buffer this pane has since switched away from. A `:split`
    /// copies it to the new pane, matching vim.
    pub jumps: JumpList,
    /// Last cursor position this session held in each buffer it has
    /// visited, keyed by document. On a buffer switch the outgoing
    /// cursor is stashed here and the incoming buffer's remembered
    /// cursor restored (origin on first visit); the entry is dropped
    /// when the buffer is `:bd`-deleted. Per-session, so two panes
    /// showing one document remember independent positions.
    pub cursor_memory: std::collections::HashMap<BufferRef, Cursor>,
    /// Per-buffer fold (collapse) state for this view, keyed by
    /// document. A view concern — two panes showing one document fold
    /// independently — so it lives here rather than on the shared
    /// `Buffer`. Survives buffer switches (keyed by ref, looked up
    /// fresh) and is copied to a new pane on `:split`, like
    /// `cursor_memory`; dropped on `:bd`.
    pub fold_memory: std::collections::HashMap<BufferRef, fold::FoldState>,
}

impl Editor {
    /// Fresh session referencing the original anonymous scratch
    /// document (cursor at the origin, no extras, Normal mode). The
    /// document itself lives in the pool; this only names it.
    pub fn new() -> Self {
        Self::for_doc(BufferRef::default())
    }

    /// Fresh session referencing `doc` (cursor at the origin, no extras,
    /// Normal mode, empty jumplist / cursor memory). The document with
    /// that ref must live in the pool.
    pub fn for_doc(doc: BufferRef) -> Self {
        Self {
            doc,
            ..Self::default()
        }
    }

    /// Re-point this session at `next` (its document supplied as `doc`)
    /// with the fresh-view normalization a buffer switch applies: drop
    /// multi-cursors and pending command tokens, reset the mode to
    /// Normal, and restore `next`'s remembered cursor (origin on first
    /// visit) clamped to `doc`. The jumplist and cursor memory persist.
    ///
    /// Note this does *not* stash the outgoing buffer's cursor — callers
    /// that want the leaving position remembered must do so before
    /// calling (see `App::swap_active_doc`). The `:bd` inactive-pane
    /// fixup deliberately skips that stash since the outgoing buffer is
    /// gone for good.
    pub fn adopt_doc(&mut self, next: BufferRef, doc: &Buffer) {
        let restored = self.cursor_memory.get(&next).copied().unwrap_or_default();
        self.doc = next;
        self.extra_cursors.clear();
        self.tokens.clear();
        self.mode = Mode::default();
        let last = doc.lines.len().saturating_sub(1);
        self.cursor = Cursor {
            row: restored.row.min(last),
            col: restored.col,
        };
        // Clamp the column against the (possibly shorter) restored row.
        self.clamp_col(doc, false);
    }

    /// This view's fold state for the active document. Returns a shared
    /// empty default when nothing has been folded yet, so read paths
    /// don't have to allocate an entry.
    pub fn folds(&self) -> &fold::FoldState {
        static EMPTY: std::sync::OnceLock<fold::FoldState> = std::sync::OnceLock::new();
        self.fold_memory
            .get(&self.doc)
            .unwrap_or_else(|| EMPTY.get_or_init(fold::FoldState::default))
    }

    /// Mutable fold state for the active document, creating an empty
    /// entry on first fold.
    pub fn folds_mut(&mut self) -> &mut fold::FoldState {
        self.fold_memory.entry(self.doc.clone()).or_default()
    }

    /// Re-read `buf.path` from disk and replace the buffer contents in
    /// place. Caller is responsible for the dirty-vs-force decision —
    /// this method always reloads.
    ///
    /// Returns:
    /// - `Ok(true)` when the on-disk content differed and the buffer
    ///   was rewritten (undo snapshot taken, version bumped, cursor
    ///   clamped, highlighter refreshed).
    /// - `Ok(false)` when disk matched the buffer — only `disk_meta`
    ///   is refreshed (mtime alone may have moved), nothing else
    ///   moves so undo history stays intact.
    /// - `Err(_)` when the read failed or no path is attached.
    pub fn reload_from_disk(&mut self, buf: &mut Buffer) -> Result<bool> {
        let path = buf
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no file name"))?;
        let text = fs::read_to_string(&path)?;
        let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        if lines == buf.lines {
            buf.disk_meta = FileMeta::of(&path);
            buf.disk_base = Some(lines);
            return Ok(false);
        }
        self.snapshot(buf);
        buf.lines = lines;
        buf.dirty = false;
        buf.version = buf.version.wrapping_add(1);
        buf.vcs_base = vcs::head_blob_lines(&path);
        *buf.vcs_diff.borrow_mut() = None;
        buf.disk_meta = FileMeta::of(&path);
        buf.disk_base = Some(buf.lines.clone());

        self.clamp_cursors_into(&buf.lines);
        // Collapsed headers past the (possibly shorter) reloaded buffer
        // can never match a region again — prune them so the set doesn't
        // grow without bound across a long session. Only touch an
        // existing entry: don't conjure an empty `FoldState` for a buffer
        // that was never folded (autoreload would otherwise accrete them).
        if let Some(folds) = self.fold_memory.get_mut(&self.doc) {
            folds.retain_below(buf.lines.len());
        }
        buf.refresh_highlights();
        Ok(true)
    }

    /// Three-way merge the backing file's current on-disk content into the
    /// buffer's unsaved edits via [`merge::three_way`] (line-level, with a
    /// character-level retry on conflicting runs). The ancestor is
    /// `buf.disk_base` (the content we last read from / wrote to disk),
    /// "ours" is `buf.lines`, "theirs" is the freshly-read file. A clean
    /// merge rewrites the buffer in place; genuinely overlapping edits carry
    /// `<<<<<<<`/`>>>>>>>` markers (the caller toasts either outcome — this
    /// method itself shows no UI). Either way an undo snapshot is taken and
    /// `disk_base`/`disk_meta` advance to the just-seen disk state, so the
    /// watcher won't re-fire until the file changes again.
    ///
    /// Falls back to a wholesale [`Self::reload_from_disk`] when there's no
    /// `disk_base` to anchor the merge (returns [`MergeOutcome::Clean`]).
    pub fn merge_from_disk(&mut self, buf: &mut Buffer) -> Result<MergeOutcome> {
        let path = buf
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no file name"))?;
        let theirs = fs::read_to_string(&path)?;
        let mut disk_lines: Vec<String> = theirs.split('\n').map(|s| s.to_string()).collect();
        if disk_lines.is_empty() {
            disk_lines.push(String::new());
        }

        // Buffer already matches disk (e.g. an editor rewrote identical
        // bytes, or our own save raced the poll): just re-baseline.
        if disk_lines == buf.lines {
            buf.disk_meta = FileMeta::of(&path);
            buf.disk_base = Some(disk_lines);
            buf.dirty = false;
            return Ok(MergeOutcome::Unchanged);
        }

        // No ancestor to diff against — can't do a meaningful three-way,
        // so fall back to replacing wholesale.
        let Some(base) = buf.disk_base.clone() else {
            self.reload_from_disk(buf)?;
            return Ok(MergeOutcome::Clean);
        };

        let (merged_lines, conflict) = merge::three_way(&base, &buf.lines, &disk_lines);

        self.snapshot(buf);
        buf.lines = merged_lines;
        buf.version = buf.version.wrapping_add(1);
        buf.vcs_base = vcs::head_blob_lines(&path);
        *buf.vcs_diff.borrow_mut() = None;
        // Re-baseline to the disk state we just merged against. The buffer
        // now holds the merge result, which may carry local edits not yet
        // on disk — so it stays dirty unless it happens to equal disk.
        buf.disk_meta = FileMeta::of(&path);
        buf.dirty = buf.lines != disk_lines;
        buf.disk_base = Some(disk_lines);

        self.clamp_cursors_into(&buf.lines);
        buf.refresh_highlights();
        Ok(if conflict {
            MergeOutcome::Conflict
        } else {
            MergeOutcome::Clean
        })
    }

    /// Clamp every cursor (primary + extras) into `lines`, fixing `row`
    /// before `col`. Done inline rather than via `clamp_col` so the row is
    /// valid first — `clamp_col` reads `current_line` off the primary
    /// cursor's row, which would panic if `row` were still past the new
    /// end. Shared by `reload_from_disk` and `merge_from_disk`, both of
    /// which can shrink the buffer underneath the cursors.
    fn clamp_cursors_into(&mut self, lines: &[String]) {
        let last_row = lines.len().saturating_sub(1);
        let clamp_one = |c: &mut Cursor, lines: &[String]| {
            if c.row > last_row {
                c.row = last_row;
            }
            let row_len = lines.get(c.row).map(|s| s.chars().count()).unwrap_or(0);
            if c.col > row_len {
                c.col = row_len;
            }
        };
        clamp_one(&mut self.cursor, lines);
        for c in &mut self.extra_cursors {
            clamp_one(c, lines);
        }
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            ..Default::default()
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let disk_meta = FileMeta::of(path);
        let disk_base = Some(lines.clone());
        Ok(Self {
            lines,
            path: Some(path.to_path_buf()),
            dirty: false,
            yank: String::new(),
            version: 0,
            highlighter: None,
            scroll: Cell::new(0),
            col_scroll: Cell::new(0),
            viewport_height: Cell::new(0),
            pending_center: Cell::new(false),
            cursor_visual_y: Cell::new(0),
            indent_anim: Cell::new(None),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            // Filled in by a worker thread after open so git doesn't
            // block the first paint — see `App::spawn_vcs_worker`.
            vcs_base: None,
            vcs_diff: RefCell::new(None),
            conflict_cache: RefCell::new(None),
            disk_meta,
            disk_base,
        })
    }

    pub fn save(&mut self) -> Result<()> {
        if let Some(p) = &self.path {
            fs::write(p, self.lines.join("\n"))?;
            self.dirty = false;
            self.disk_meta = FileMeta::of(p);
            self.disk_base = Some(self.lines.clone());
        }
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        fs::write(path, self.lines.join("\n"))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.disk_meta = FileMeta::of(path);
        self.disk_base = Some(self.lines.clone());
        Ok(())
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.version = self.version.wrapping_add(1);
    }

    /// Bump the version counter without touching `dirty`. Used when an
    /// external rewriter (e.g. LSP workspace edit application) wants to
    /// invalidate cached highlights without otherwise altering state.
    pub fn bump_version(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Parsed git conflict hunks, recomputed only when the buffer
    /// `version` moves (mirrors [`Self::vcs_statuses`]). Cheap on a hot
    /// cache, so the renderer can call it every frame; the underlying
    /// parse is a per-line marker scan, kept off the hot path by the
    /// version gate. Returns an owned `Vec` (hunks are small `Copy`
    /// structs) so callers don't hold a `RefCell` borrow.
    pub fn conflict_hunks(&self) -> Vec<conflict::Hunk> {
        {
            let cache = self.conflict_cache.borrow();
            if let Some((v, hunks)) = cache.as_ref()
                && *v == self.version
            {
                return hunks.clone();
            }
        }
        let hunks = conflict::hunks(&self.lines);
        *self.conflict_cache.borrow_mut() = Some((self.version, hunks.clone()));
        hunks
    }

    pub fn refresh_highlights(&mut self) {
        let Some(h) = self.highlighter.as_mut() else {
            return;
        };
        // Called every frame from the draw loop. Skip the
        // full-document `lines.join` allocation when the tree already
        // reflects the current version — the common case once the
        // buffer is idle and during pure navigation.
        if h.is_current(self.version) {
            return;
        }
        let source = self.lines.join("\n");
        h.refresh(&source, self.version);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Shared helpers, available to all editor submodules.
// ────────────────────────────────────────────────────────────────────────

/// Convert a 0-based character index into the corresponding byte offset
/// in `s`. Past-the-end indices clamp to `s.len()` so callers can use
/// the result as an exclusive end without bounds checking.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Inverse of [`char_to_byte`]. Counts the chars up to (but not
/// including) `byte_idx`. The caller is responsible for ensuring
/// `byte_idx` falls on a char boundary.
fn byte_to_char(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx].chars().count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Punct,
    Space,
}

fn classify(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn is_blank_line(line: &str) -> bool {
    line.chars().all(|c| c.is_whitespace())
}

// ────────────────────────────────────────────────────────────────────────
// Test harness.
// ────────────────────────────────────────────────────────────────────────

/// Pairs an [`Editor`] session with the [`Buffer`] it edits so the
/// submodule unit tests can keep their old `Editor::new()` + `b.buffer`
/// + `b.<op>(...)` shape after `Editor` stopped owning its buffer.
///
/// `Deref`/`DerefMut` forward to the inner `Editor` (so `b.cursor`,
/// `b.extra_cursors`, `b.mode` keep working), while the editing ops
/// used by tests are re-exposed as inherent methods that thread
/// `&mut self.buffer` — inherent methods shadow the `Deref` target, so
/// `b.insert_char(...)` resolves here, not to `Editor::insert_char`.
#[cfg(test)]
pub(crate) struct Ed {
    pub editor: Editor,
    pub buffer: Buffer,
}

#[cfg(test)]
#[allow(dead_code)]
impl Ed {
    pub fn new() -> Self {
        Self {
            editor: Editor::new(),
            buffer: Buffer::new(),
        }
    }

    // Insert-side ops.
    pub fn insert_char(&mut self, c: char) {
        self.editor.insert_char(&mut self.buffer, c)
    }
    pub fn insert_char_smart(&mut self, c: char, indent: IndentSettings) {
        self.editor.insert_char_smart(&mut self.buffer, c, indent)
    }
    pub fn insert_newline(&mut self, indent: IndentSettings) {
        self.editor.insert_newline(&mut self.buffer, indent)
    }
    pub fn insert_text_raw(&mut self, s: &str) {
        self.editor.insert_text_raw(&mut self.buffer, s)
    }
    pub fn insert_line_below(&mut self, indent: IndentSettings) {
        self.editor.insert_line_below(&mut self.buffer, indent)
    }
    pub fn insert_line_above(&mut self, indent: IndentSettings) {
        self.editor.insert_line_above(&mut self.buffer, indent)
    }
    pub fn indent_line(&mut self, row: usize, indent: IndentSettings) {
        self.editor.indent_line(&mut self.buffer, row, indent)
    }
    pub fn dedent_line(&mut self, row: usize, indent: IndentSettings) {
        self.editor.dedent_line(&mut self.buffer, row, indent)
    }
    pub fn delete_char_before_smart(&mut self, indent: IndentSettings) {
        self.editor
            .delete_char_before_smart(&mut self.buffer, indent)
    }

    // Surround ops.
    pub fn surround_wrap(&mut self, open: &str, close: &str, from: Cursor, to: Cursor) {
        self.editor
            .surround_wrap(&mut self.buffer, open, close, from, to)
    }
    pub fn surround_strip(&mut self, lo: Cursor, hi: Cursor) {
        self.editor.surround_strip(&mut self.buffer, lo, hi)
    }
    pub fn surround_replace(&mut self, lo: Cursor, hi: Cursor, new_open: &str, new_close: &str) {
        self.editor
            .surround_replace(&mut self.buffer, lo, hi, new_open, new_close)
    }

    // Substitute.
    pub fn substitute(&mut self, args: &SubsArgs<'_>) -> substitute::SubsOutcome {
        self.editor.substitute(&mut self.buffer, args)
    }
}

#[cfg(test)]
impl std::ops::Deref for Ed {
    type Target = Editor;
    fn deref(&self) -> &Editor {
        &self.editor
    }
}

#[cfg(test)]
impl std::ops::DerefMut for Ed {
    fn deref_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A throwaway file under the temp dir, removed on drop.
    struct TempFile(PathBuf);
    impl TempFile {
        fn with(contents: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("vorto-merge-{}-{n}.txt", std::process::id()));
            fs::write(&path, contents).unwrap();
            Self(path)
        }
        fn write(&self, contents: &str) {
            fs::write(&self.0, contents).unwrap();
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn merge_clean_combines_disjoint_edits() {
        let tf = TempFile::with("a\nb\nc");
        let mut buf = Buffer::load(&tf.0).unwrap();
        let mut ed = Editor::new();

        // Local edit on the first line (unsaved).
        buf.lines[0] = "A".to_string();
        buf.dirty = true;
        // External edit on the last line.
        tf.write("a\nb\nC");

        let outcome = ed.merge_from_disk(&mut buf).unwrap();
        assert_eq!(outcome, MergeOutcome::Clean);
        assert_eq!(buf.lines, vec!["A", "b", "C"]);
        // Local edit not yet on disk → still dirty.
        assert!(buf.dirty);
        // Re-baselined to the disk we merged against.
        assert_eq!(
            buf.disk_base.as_ref().map(|v| v.join("\n")),
            Some("a\nb\nC".to_string())
        );
    }

    #[test]
    fn merge_conflict_inserts_markers() {
        let tf = TempFile::with("a\nb\nc");
        let mut buf = Buffer::load(&tf.0).unwrap();
        let mut ed = Editor::new();

        buf.lines[1] = "local-b".to_string();
        buf.dirty = true;
        tf.write("a\ndisk-b\nc");

        let outcome = ed.merge_from_disk(&mut buf).unwrap();
        assert_eq!(outcome, MergeOutcome::Conflict);
        let text = buf.lines.join("\n");
        assert!(text.contains("<<<<<<< local (your edits)"), "{text}");
        assert!(text.contains("local-b"), "{text}");
        assert!(text.contains("disk-b"), "{text}");
        assert!(text.contains(">>>>>>> disk"), "{text}");
        assert!(buf.dirty);
    }

    #[test]
    fn conflict_hunks_are_cached_until_version_moves() {
        let mut b = Buffer::new();
        b.lines = vec![
            "<<<<<<<".into(),
            "a".into(),
            "=======".into(),
            "b".into(),
            ">>>>>>>".into(),
        ];
        assert_eq!(b.conflict_hunks().len(), 1);
        // Edit the lines without bumping the version: the cache is still
        // hot, so the stale (cached) parse is served on purpose.
        b.lines = vec![String::new()];
        assert_eq!(
            b.conflict_hunks().len(),
            1,
            "cache is served while the version is unchanged"
        );
        // Bumping the version invalidates it and forces a re-parse.
        b.bump_version();
        assert_eq!(
            b.conflict_hunks().len(),
            0,
            "re-parsed after a version bump"
        );
    }

    #[test]
    fn merge_clean_buffer_just_rebaselines() {
        let tf = TempFile::with("a\nb\nc");
        let mut buf = Buffer::load(&tf.0).unwrap();
        let mut ed = Editor::new();

        // No local edits; disk changed.
        tf.write("a\nb\nc");
        let outcome = ed.merge_from_disk(&mut buf).unwrap();
        assert_eq!(outcome, MergeOutcome::Unchanged);
        assert!(!buf.dirty);
    }
}
