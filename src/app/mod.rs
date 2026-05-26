//! Top-level application state.
//!
//! `App` owns the buffer, mode, prompt, configuration, LSP coordinator,
//! highlighter loader, and fuzzy-preview cache + worker channel. The
//! behavioral surface (input handling, LSP orchestration, file-open
//! orchestration, Normal-mode evaluation) is split across sibling
//! `impl App { ... }` blocks in the submodules below.

/// Call a mutating [`crate::editor::Editor`] op on the active session,
/// threading the active document out of the pool. Expands to the
/// resolve-ref-then-`get_mut` dance so the `&mut Buffer` borrow and the
/// `&mut Editor` borrow stay disjoint field projections of the same
/// `App`. Use at any `app.editor.<op>(...)` site that mutates the
/// document. `$app` is the `App` place (e.g. `self` or `app`).
macro_rules! ed_op {
    ($app:expr, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let __doc_ref = $app.editor.doc.clone();
        let __doc = $app
            .documents
            .get_mut(&__doc_ref)
            .expect("active doc present in pool");
        $app.editor.$method(__doc, $($arg),*)
    }};
}

/// Read-only counterpart to [`ed_op!`] for [`crate::editor::Editor`]
/// methods that take `&Buffer`. The borrowed document is released at
/// the end of the expression, so the call's return value must be owned
/// (e.g. an `Option<(Cursor, Cursor)>`), not a borrow into the buffer.
macro_rules! ed_op_ref {
    ($app:expr, $method:ident ( $($arg:expr),* $(,)? )) => {{
        let __doc_ref = $app.editor.doc.clone();
        let __doc = $app
            .documents
            .get(&__doc_ref)
            .expect("active doc present in pool");
        $app.editor.$method(__doc, $($arg),*)
    }};
}

mod agent;
mod buffer_list;
mod comment;
mod completion;
mod copilot;
mod eval;
mod grammar;
mod input;
mod jump;
mod lsp_apply;
mod lsp_coordinator;
mod lsp_request;
mod open;
mod pane;
mod runtime;
mod signature;
mod sleeping;
mod toast;
mod types;
mod workers;

pub use completion::CompletionState;
pub use copilot::{CopilotAuthState, CopilotPending};

pub use jump::JumpState;
pub use lsp_coordinator::{LspCoordinator, LspEventOutcome};
pub use pane::{PaneId, PaneLayout, PaneRect, PaneRectMap, SplitDir};
pub use signature::{SignatureState, SignatureTrigger};
pub use sleeping::SleepingBuffer;
pub use toast::{Level, Toast, ToastQueue};
pub use types::Selection;

use crate::buffer_ref::BufferRef;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Sender;

use crate::action::{InsertKey, LastChange, LastFind};

/// Quiet period after the last input event before a debounced
/// inline-completion request actually fires. Tuned to feel snappy
/// while still folding a bursty typing run into a single request.
/// Copilot's server-side latency dominates total perceived delay
/// (~hundreds of ms), so keeping the client-side wait short is what
/// makes the ghost feel responsive.
const INLINE_REQUEST_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(75);

/// Active insert-session recording. Lives on `App` so `handle_insert_key`
/// can append the keystrokes the user types, and finalize on Esc.
#[derive(Debug)]
pub struct InsertRecording {
    pub trigger: crate::action::Expr,
    pub keys: Vec<InsertKey>,
}
use crate::config::{Config, EditorConfig};
use crate::editor::SearchState;
use crate::editor::{Cursor, Editor, SuggestionState};
use crate::event::AppEvent;
use crate::finder::{self, PreviewLru};
use crate::prompt::PromptController;
use crate::syntax::Loader;

pub use crate::prompt::Prompt;

/// Cap on the recently-opened-files MRU. 64 is plenty for normal use
/// and bounds memory without needing a fancy eviction policy.
const MRU_CAP: usize = 64;

pub struct App {
    /// Active editing session. *References* its document via
    /// [`Editor::doc`]; the document itself lives in [`Self::documents`].
    /// Carries the cursor / multi-cursor / mode for this view and the
    /// in-flight command token stream. Kept as a dedicated field (rather
    /// than another entry in `pane_editors`) so the hot path
    /// `self.editor.cursor` / `.mode` / `.tokens` stays a single field
    /// access.
    pub editor: Editor,
    /// The shared document pool. Always contains the document named by
    /// `self.editor.doc` and by every entry in [`Self::pane_editors`].
    /// Two panes showing the same buffer name one entry here through
    /// equal [`BufferRef`]s while keeping independent cursors.
    pub documents: std::collections::HashMap<BufferRef, crate::editor::Buffer>,
    pub prompt: PromptController,
    pub search: SearchState,
    pub toasts: ToastQueue,
    /// Anchor cursor for visual modes — the position the selection was
    /// started from. `None` outside of any visual mode.
    pub visual_anchor: Option<Cursor>,
    /// Resolved user configuration (keymap, cursor shapes, language
    /// registry, grammar/query dirs). Frozen at startup.
    pub config: Config,
    /// Tree-sitter grammar loader. Lives for the whole program so the
    /// loaded `Language` pointers stay valid. Wrapped in `Arc<Mutex>` so
    /// the file-open worker thread can build a fresh highlighter off the
    /// main thread, and the fuzzy-finder preview can still lazily build
    /// a separate highlighter for the file under the cursor during the
    /// (otherwise `&App`) draw pass.
    pub loader: Arc<Mutex<Loader>>,
    /// Bounded LRU of fuzzy-finder source previews. The worker thread
    /// fills this asynchronously through `AppEvent::PreviewReady`; the
    /// draw path looks here first and falls back to plain text on miss
    /// (while enqueueing a worker request). Living on `App` so back-
    /// to-back navigation to the same file is instant.
    pub preview_lru: RefCell<PreviewLru>,
    /// Request channel feeding the preview worker. Cloned on draw to
    /// dispatch "build preview for path X" jobs.
    pub preview_tx: std::sync::mpsc::Sender<PathBuf>,
    /// Last path we asked the worker about. Prevents the draw loop from
    /// flooding the channel with duplicate requests for the same
    /// selection while the worker is still busy.
    pub last_preview_request: RefCell<Option<PathBuf>>,
    /// Working directory captured once at process startup. All workspace
    /// root discovery anchors here — `:e` opened mid-session still uses
    /// the same anchor as the file passed on the command line.
    pub startup_cwd: PathBuf,
    /// All LSP state — clients, current document, diagnostics, pending
    /// requests, sync version, root anchor. See [`LspCoordinator`].
    pub lsp: LspCoordinator,
    /// Copilot LSP client. `None` while not yet spawned or after a
    /// reader-thread error tore it down; future requests trigger a
    /// re-spawn attempt. See [`crate::copilot`].
    pub copilot: Option<crate::copilot::CopilotClient>,
    /// True while a Copilot spawn is in flight on a worker thread.
    /// Prevents [`Self::spawn_copilot_if_needed`] from launching a
    /// second handshake before the first finishes.
    pub(super) copilot_spawning: bool,
    /// Pending Copilot requests, keyed by request id. Lets the reader
    /// thread surface generic [`crate::copilot::CopilotEvent::Response`]
    /// events without leaking response shapes into the codec layer.
    pub copilot_pending: CopilotPending,
    /// Most recent Copilot auth state, learned from `checkStatus`
    /// replies. `Unknown` until the first reply lands; inline
    /// completion is suppressed until `SignedIn`.
    pub copilot_auth: CopilotAuthState,
    /// Device-flow user code + verification URL from an outstanding
    /// `signInInitiate`. Held so `:copilot code` can re-display the
    /// modal and re-copy to clipboard if the user dismissed it or
    /// the clipboard was overwritten by something else mid-signin.
    /// `None` when no signin is in flight (either never started, or
    /// the confirm reply has already settled the auth state).
    pub copilot_pending_code: Option<(String, String)>,
    /// Shared event channel — kept on `App` so `open_path` can spawn
    /// worker threads that report `EngineReady` / `LspReady` back
    /// to the main loop without going through the LSP coordinator.
    pub event_tx: Sender<AppEvent>,
    /// Monotonic counter bumped on every `open_path`. Worker threads
    /// stamp their result with the generation they were spawned for; a
    /// stale result (user opened another file in the meantime) gets
    /// dropped instead of clobbering the current buffer.
    pub open_gen: u64,
    /// MRU of recently-touched buffers (newest at the end). Drives the
    /// `<space>b` buffer picker. Capped at [`MRU_CAP`] entries. Scratch
    /// buffers are represented by `BufferRef::Scratch(id)`; the
    /// initial unnamed buffer is `Scratch(0)` and each `:new` mints a
    /// fresh id.
    pub opened_paths: Vec<BufferRef>,
    /// Identifier of the active buffer when it is unnamed (a scratch
    /// buffer). `None` when the active buffer is backed by a file.
    /// Kept on `App` rather than on `Buffer` so the buffer struct
    /// stays oblivious to MRU bookkeeping.
    pub current_scratch_id: Option<u32>,
    /// Next id `:new` will hand out. Incremented after each mint;
    /// never reused even after a scratch buffer is deleted, so a
    /// stashed sleeping scratch can't be confused with a fresh one.
    pub next_scratch_id: u32,
    /// Sleeping (non-active) buffers, keyed by [`BufferRef`]. When the
    /// user switches away from a buffer we move its state in here so
    /// the unsaved edits, undo history, and cursor position are still
    /// around the next time they pick it up. The highlighter isn't
    /// preserved — it's rebuilt by the worker on restore. Lines and
    /// undo/redo content are deflate-compressed when the buffer's
    /// total raw byte count is large enough to be worth it (see
    /// `sleeping::SleepingBuffer::freeze`).
    pub sleeping: HashMap<BufferRef, SleepingBuffer>,
    /// Last `f`/`F`/`t`/`T` so `;` and `,` know what to repeat.
    pub last_find: Option<LastFind>,
    /// Last buffer-modifying change — what `.` replays. Updated when a
    /// change finishes (immediately for one-shot Exprs, on Esc for
    /// Insert-mode sessions).
    pub last_change: Option<LastChange>,
    /// Active Insert-session recording. `Some` while the user is in an
    /// Insert mode entered through a recordable trigger; finalized into
    /// `last_change` when Esc returns us to Normal.
    pub recording: Option<InsertRecording>,
    /// True when a `g` prefix is pending in Visual mode. Normal mode
    /// uses its token stream for this; Visual mode bypasses the token
    /// pipeline so it tracks the one prefix it cares about here.
    pub visual_g_pending: bool,
    /// Active `gw` jump-label overlay, if any. `Some` between the user
    /// pressing `gw` and either picking a label or cancelling. While
    /// it's `Some`, the input dispatcher routes every key to
    /// [`App::handle_jump_key`] and the UI renders the label overlay.
    pub jump_state: Option<JumpState>,
    /// Active LSP completion popup, if any. `Some` between a successful
    /// `textDocument/completion` response and the user accepting,
    /// dismissing, or invalidating it (cursor row change / backspace
    /// past the prefix start).
    pub completion: Option<CompletionState>,
    /// Active LSP signature-help popup, if any. `Some` between a
    /// non-empty `textDocument/signatureHelp` response and either the
    /// server returning `null` (no longer inside a call), the cursor
    /// crossing rows, or Esc.
    pub signature: Option<SignatureState>,
    /// In-flight or showing inline (ghost-text) completion. Driven by
    /// the Copilot LSP client when one is attached. The request id
    /// inside the `Pending` variant is the Copilot JSON-RPC id, so
    /// supersession races are decided by the same id space.
    pub inline_suggestion: SuggestionState,
    /// Earliest moment a debounced inline-suggestion request may fire.
    /// Set by [`Self::schedule_inline_suggestion`] on every input event
    /// and cleared by [`Self::tick_inline_suggestion`] once the
    /// deadline elapses and the request actually goes out. `None`
    /// means no fire is queued — the main loop won't add a wake-up.
    pub(super) inline_request_deadline: Option<std::time::Instant>,
    /// System clipboard handle, initialized lazily on first yank.
    /// `None` means we haven't tried yet *or* the platform refused to
    /// give us one (Wayland without a compositor, headless CI, …); the
    /// internal `Buffer.yank` register keeps working either way, so a
    /// failed init silently degrades to vorto-local yank.
    pub clipboard: Option<arboard::Clipboard>,
    /// Tree describing how the buffer viewport is partitioned into
    /// panes. A bare `PaneLayout::Leaf` means "no splits — single
    /// pane covering the whole viewport". See [`mod@pane`] for the
    /// active-pane convention.
    pub layout: PaneLayout,
    /// Pane id of the currently-active leaf in [`Self::layout`]. The
    /// session for that pane is `App.editor`; other leaves' sessions
    /// live in [`Self::pane_editors`].
    pub active_pane: PaneId,
    /// Per-pane editing sessions for every *inactive* leaf of
    /// [`Self::layout`], keyed by [`PaneId`]. The active pane's session
    /// is `App.editor` and is NOT also stored here. Keying by pane (not
    /// by buffer ref) is what lets two panes show the same document with
    /// independent cursors — each `Editor` carries its own cursor and
    /// names its document through `.doc`. The documents themselves live
    /// in [`Self::documents`].
    pub pane_editors: std::collections::HashMap<PaneId, Editor>,
    /// Counter for [`Self::mint_pane_id`]. Monotonic — never reused so
    /// a sleeping pane snapshot can't be confused with a fresh one.
    pub next_pane_id: PaneId,
    /// Rectangles each pane was drawn into on the most recent frame.
    /// Populated by the UI just before draw; read by directional
    /// focus navigation so `Ctrl-W h/j/k/l` resolves against what the
    /// user actually sees.
    pub last_pane_rects: RefCell<PaneRectMap>,
    /// Grammars we've already offered to install this session (via the
    /// open-time "install?" modal). Once a grammar is in here we never
    /// re-prompt for it — whether the user accepted, declined, or the
    /// install later failed — so opening more files of an uninstalled
    /// language stays quiet instead of nagging on every open.
    pub(super) asked_grammars: std::collections::HashSet<String>,
    /// Config-aware grammar recipe catalog (built-ins overlaid with
    /// `[grammars.*]`), resolved once at startup. Cached because
    /// [`crate::grammar::recipe::GrammarRecipe::from_config`] leaks its
    /// strings into `&'static`, so re-merging on every file-open / modal
    /// would leak afresh each time. `config` is frozen at startup, so a
    /// single resolution stays correct for the whole session.
    pub(super) grammar_recipes: Vec<crate::grammar::recipe::GrammarRecipe>,
    /// Multiplexer pane id of the agent launched by `:agent` this
    /// session, if any. A second `:agent` focuses this pane (when still
    /// alive) instead of opening another. Session-scoped — not persisted,
    /// so a vorto restart starts fresh.
    pub(super) agent_pane: Option<String>,
    /// Prompt staged by a `:agent <intent> @file` issued while no default
    /// agent is configured: the picker opens, and [`Self::select_agent`]
    /// consumes this so the chosen agent still launches seeded. `None`
    /// outside that window (a bare `:agent` clears it).
    pub(super) agent_pending_prompt: Option<String>,
    /// Text of the visual selection captured when `:` opened the command
    /// prompt from a visual mode — what `:agent <intent> @selection` reads.
    /// Set by that visual `:`, cleared when the prompt opens from a
    /// non-visual mode, so it always reflects the selection live at the
    /// moment the prompt was opened (or `None` when there wasn't one).
    pub(super) command_selection: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        config: Config,
        loader: Loader,
        event_tx: Sender<AppEvent>,
        startup_cwd: PathBuf,
    ) -> Self {
        let lsp = LspCoordinator::new(event_tx.clone(), startup_cwd.clone());
        // Resolve the merged recipe catalog once — see `grammar_recipes`.
        let grammar_recipes = crate::grammar::cli::merged_recipes(&config.grammars);
        let loader = Arc::new(Mutex::new(loader));
        let (preview_tx, preview_rx) = std::sync::mpsc::channel::<PathBuf>();
        // Spawn the fuzzy-finder preview worker. It owns the receiver,
        // clones of `loader` and the language registry, and an `emit`
        // closure that wraps results in `AppEvent::PreviewReady` so the
        // main loop just inserts them into the LRU on dispatch.
        let preview_emit_tx = event_tx.clone();
        finder::spawn_preview_worker(
            Arc::clone(&loader),
            config.languages.clone(),
            preview_rx,
            Box::new(move |entry| {
                let _ = preview_emit_tx.send(AppEvent::PreviewReady(entry));
            }),
        );
        let mut documents = HashMap::new();
        documents.insert(BufferRef::Scratch(0), crate::editor::Buffer::new());
        Self {
            editor: Editor::new(),
            documents,
            prompt: PromptController::new(),
            search: SearchState::default(),
            toasts: ToastQueue::new(),
            visual_anchor: None,
            config,
            loader,
            preview_lru: RefCell::new(PreviewLru::new(16)),
            preview_tx,
            last_preview_request: RefCell::new(None),
            startup_cwd,
            lsp,
            copilot: None,
            copilot_spawning: false,
            copilot_pending: CopilotPending::default(),
            copilot_auth: CopilotAuthState::default(),
            copilot_pending_code: None,
            event_tx,
            open_gen: 0,
            // Pre-seed with Scratch so the picker always offers a way
            // back to the unnamed empty buffer, even after opening a
            // real file over it.
            opened_paths: vec![BufferRef::Scratch(0)],
            current_scratch_id: Some(0),
            next_scratch_id: 1,
            sleeping: HashMap::new(),
            last_find: None,
            last_change: None,
            recording: None,
            visual_g_pending: false,
            jump_state: None,
            completion: None,
            signature: None,
            inline_suggestion: SuggestionState::default(),
            inline_request_deadline: None,
            clipboard: None,
            layout: PaneLayout::Leaf(pane::INITIAL_PANE_ID),
            active_pane: pane::INITIAL_PANE_ID,
            // pane_editors only tracks inactive panes — the initial
            // layout has just the active pane, so this starts empty.
            pane_editors: HashMap::new(),
            next_pane_id: pane::NEXT_PANE_ID_SEED,
            last_pane_rects: RefCell::new(PaneRectMap::default()),
            asked_grammars: std::collections::HashSet::new(),
            grammar_recipes,
            agent_pane: None,
            agent_pending_prompt: None,
            command_selection: None,
            should_quit: false,
        }
    }

    /// Record `r` as the most recent buffer the user touched. Moves
    /// existing entries to the front so the picker stays in MRU order,
    /// caps the list at [`MRU_CAP`] entries, and evicts the matching
    /// sleeping snapshot when one falls off the back of the MRU —
    /// otherwise the in-memory snapshots would grow unbounded.
    pub(super) fn record_opened(&mut self, r: BufferRef) {
        self.opened_paths.retain(|x| x != &r);
        self.opened_paths.push(r);
        while self.opened_paths.len() > MRU_CAP {
            let evicted = self.opened_paths.remove(0);
            self.sleeping.remove(&evicted);
        }
    }

    /// The document the active session ([`Self::editor`]) is editing.
    /// Panics if the pool invariant is broken (the active doc must
    /// always be present).
    pub fn active_doc(&self) -> &crate::editor::Buffer {
        self.documents
            .get(&self.editor.doc)
            .expect("active doc present in pool")
    }

    /// Mutable counterpart to [`Self::active_doc`].
    pub fn active_doc_mut(&mut self) -> &mut crate::editor::Buffer {
        self.documents
            .get_mut(&self.editor.doc)
            .expect("active doc present in pool")
    }

    /// Current selection range, if the editor is in any visual mode and
    /// an anchor is set. Returns `None` otherwise.
    pub fn selection(&self) -> Option<Selection> {
        types::selection(self.editor.mode, self.visual_anchor, self.editor.cursor)
    }

    /// The text of the current visual selection, read-only (does not touch
    /// the yank register). `None` outside a visual mode. The end of a
    /// char-wise selection is inclusive (vim-style), so it's advanced one
    /// char to match what visual `y` would capture.
    pub fn selection_text(&self) -> Option<String> {
        let doc = self.active_doc();
        Some(match self.selection()? {
            Selection::Char { from, to } => {
                let end = doc.advance_one(to);
                doc.range_text(from, end)
            }
            Selection::Line { from_row, to_row } => doc.lines_text(from_row, to_row),
            Selection::Block { r0, c0, r1, c1 } => doc.block_text(r0, c0, r1, c1),
        })
    }

    /// Advance the toast queue: drop expired non-fatal toasts and
    /// promote pending ones into the freed slots. The main loop calls
    /// this once per iteration before draw + [`toast_remaining`] so
    /// both see a fresh view.
    pub fn tick_toasts(&mut self) {
        self.toasts.tick();
    }

    /// Time until the next toast-queue state change. `None` means
    /// nothing is on screen and the main loop can block without a
    /// timeout; otherwise the value is the soonest non-fatal TTL
    /// expiry (or a long placeholder if only fatal toasts are live),
    /// so the loop wakes up to advance the queue.
    pub fn toast_remaining(&self) -> Option<std::time::Duration> {
        self.toasts.remaining()
    }

    /// Frame interval for an in-flight indent-guide animation, or
    /// `None` when the active buffer has no pending animation. The
    /// main loop merges this with [`toast_remaining`] via `min` so
    /// it wakes ~60 fps only while the bracket is expanding, then
    /// returns to fully blocked recv as soon as the animation
    /// clears its cache.
    pub fn indent_anim_remaining(&self) -> Option<std::time::Duration> {
        // Only wake during the in-flight phase. Settled entries
        // (Instant = None) sit in the cache purely to detect future
        // scope changes; they shouldn't keep the loop spinning.
        match self.active_doc().indent_anim.get() {
            Some((Some(_), _, _)) => Some(std::time::Duration::from_millis(16)),
            _ => None,
        }
    }

    /// Queue a toast for display. Goes straight to the visible stack
    /// while there's room (cap of 3); otherwise waits behind the
    /// already-visible toasts and is promoted as they expire.
    pub fn push_toast(&mut self, t: Toast) {
        // Mirror error/fatal toasts into the debug log so they survive
        // past the TTL — info/warn stay UI-only to avoid noise.
        match t.level() {
            Level::Error => crate::vlog!("toast error: {}", t.text()),
            Level::Fatal => crate::vlog!("toast fatal: {}", t.text()),
            _ => {}
        }
        self.toasts.push(t);
    }

    /// Wipe all toasts — visible and queued. Exposed for callers that
    /// want to take over the toast slot wholesale; not currently used
    /// in-tree.
    #[allow(dead_code)]
    pub fn clear_toast(&mut self) {
        self.toasts.clear();
    }

    /// Visual column (0-based cell offset, not char index) of the
    /// primary cursor on its current line, after tabs are expanded
    /// using the buffer's effective `tab_width`. Mirrors what
    /// [`ui::buffer::place_cursor`] places on screen, so the status
    /// bar and any other consumer can show a position that matches
    /// where the cursor actually sits.
    pub fn cursor_visual_col(&self) -> usize {
        self.char_col_visual(self.editor.cursor.row, self.editor.cursor.col)
    }

    /// Visual column for an arbitrary `(row, char_col)`. Resolves the
    /// effective `tab_width` from config, then defers to
    /// [`crate::text_width::visual_col_of`] — keep the math in one
    /// place so cursor placement, status bar, popup anchoring, and the
    /// renderer all agree.
    pub fn char_col_visual(&self, row: usize, char_col: usize) -> usize {
        let tab_width = self.effective_editor().tab_width.max(1);
        let Some(line) = self.active_doc().lines.get(row) else {
            return 0;
        };
        crate::text_width::visual_col_of(line, char_col, tab_width)
    }

    /// Visual y (within the buffer viewport) of `row`, given the
    /// current scroll. Accounts for inline diagnostic lines that push
    /// subsequent source rows down. Returns `None` when `row` is
    /// scrolled off the top.
    ///
    /// Cursor-anchored overlays (hover, completion, code-action menu)
    /// use this so they sit below the right visual line — `cursor.row -
    /// scroll` undercounts whenever any earlier visible row carries a
    /// diagnostic.
    pub fn visual_row_offset(&self, row: usize) -> Option<u16> {
        let scroll = self.active_doc().scroll.get();
        if row < scroll {
            return None;
        }
        // One extra visual row per source row whose diagnostics are
        // surfaced inline. Mirrors `ui::buffer`'s filter: the cursor's
        // row shows any severity, every other row only shows `Error`s.
        let cursor_row = self.editor.cursor.row;
        let mut diag_rows: std::collections::HashSet<usize> = std::collections::HashSet::new();
        if let Some(diags) = self.current_diagnostics() {
            for d in diags {
                let r = d.range.start.line as usize;
                if r != cursor_row && d.severity != crate::lsp::Severity::Error {
                    continue;
                }
                diag_rows.insert(r);
            }
        }
        let mut y: u16 = 0;
        for r in scroll..row {
            y = y.saturating_add(1);
            if diag_rows.contains(&r) {
                y = y.saturating_add(1);
            }
        }
        Some(y)
    }

    /// Drop any in-flight or shown inline suggestion. Cheap to call on
    /// every cursor-moving / mode-exiting key event so stale ghost text
    /// never paints against a shifted cursor.
    pub(super) fn cancel_inline_suggestion(&mut self) {
        self.inline_suggestion.dismiss();
        // A scheduled fire would surface a fresh ghost the user just
        // cancelled — drop the deadline alongside the local state.
        self.inline_request_deadline = None;
    }

    /// Schedule a debounced inline-completion request. Dismisses the
    /// current ghost immediately (so the stale text doesn't linger
    /// while the user keeps typing), then sets the deadline so the
    /// main loop fires `update_inline_suggestion` after a short
    /// quiet period. Cheap to call on every keystroke / motion;
    /// repeated calls just push the deadline back, which is exactly
    /// the debounce we want.
    pub(super) fn schedule_inline_suggestion(&mut self) {
        self.inline_suggestion.dismiss();
        self.inline_request_deadline = Some(std::time::Instant::now() + INLINE_REQUEST_DEBOUNCE);
    }

    /// Time until the next debounced inline-completion fire, or `None`
    /// when nothing is queued. Merged into the main loop's wake
    /// sources so the event-channel `recv_timeout` returns exactly
    /// when [`Self::tick_inline_suggestion`] needs to run.
    pub fn inline_request_remaining(&self) -> Option<std::time::Duration> {
        let deadline = self.inline_request_deadline?;
        Some(deadline.saturating_duration_since(std::time::Instant::now()))
    }

    /// If the debounce deadline has elapsed, clear it and fire the
    /// actual `textDocument/inlineCompletion`. Called once per main
    /// loop iteration; a no-op when no fire is queued or the deadline
    /// is still in the future.
    pub fn tick_inline_suggestion(&mut self) {
        let Some(deadline) = self.inline_request_deadline else {
            return;
        };
        if std::time::Instant::now() < deadline {
            return;
        }
        self.inline_request_deadline = None;
        self.update_inline_suggestion();
    }

    /// Accept the currently-showing inline suggestion at the cursor.
    /// Returns `true` when a suggestion was applied (so the caller can
    /// short-circuit other key handling); `false` when nothing was
    /// showing or the anchor no longer matches the cursor (stale).
    ///
    /// Insertion runs through [`crate::editor::Buffer::insert_char_smart`]
    /// so auto-pair / dedent / skip-over behave the same way they
    /// would for hand-typed text — Copilot frequently returns matched
    /// pairs (`()`, `{}`) and the skip-over rule prevents doubling
    /// them. Acceptance isn't recorded into the `.` replay stream:
    /// re-running the same accepted ghost is rarely what the user
    /// wants, and the next ghost would normally differ anyway.
    pub(super) fn accept_inline_suggestion(&mut self) -> bool {
        let text = match self.inline_suggestion.showing() {
            Some(s) if s.is_anchored_at(self.editor.cursor) => s.text.clone(),
            _ => {
                self.inline_suggestion.dismiss();
                return false;
            }
        };
        self.inline_suggestion.dismiss();
        // The LSP completion popup may have been open alongside the
        // ghost (since the popup-vs-ghost lock-out was lifted). The
        // inserted text invalidates the popup's filter, so close it
        // rather than leaving a stale list on screen.
        self.cancel_completion();
        if text.contains('\n') {
            // Multi-line: bypass insert_char_smart's per-char auto-indent
            // — Copilot already supplies its own indentation for each
            // continuation row, and `insert_newline`'s reindent would
            // stack on top of it.
            let r = self.editor.doc.clone();
            let doc = self.documents.get_mut(&r).expect("active doc present");
            self.editor.insert_text_raw(doc, &text);
        } else {
            let indent = self.indent_settings();
            let r = self.editor.doc.clone();
            let doc = self.documents.get_mut(&r).expect("active doc present");
            for c in text.chars() {
                self.editor.insert_char_smart(doc, c, indent);
            }
        }
        true
    }

    /// `IndentSettings` derived from the active buffer's effective
    /// editor config. Convenience wrapper so the input + eval layers
    /// don't have to redo the `EditorConfig → IndentSettings`
    /// conversion at every call site that inserts a new line.
    pub(super) fn indent_settings(&self) -> crate::editor::IndentSettings {
        let eff = self.effective_editor();
        crate::editor::IndentSettings {
            width: eff.indent_width.max(1),
            use_tabs: eff.use_tabs,
        }
    }

    /// Effective editor settings for the active buffer: the global
    /// `[editor]` defaults with the buffer-language's per-language
    /// overrides layered on top. When the buffer has no path or its
    /// extension doesn't resolve to a known language, the global
    /// defaults are returned as-is.
    pub fn effective_editor(&self) -> EditorConfig {
        let base = self.config.editor;
        let Some(path) = self.active_doc().path.as_ref() else {
            return base;
        };
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return base;
        };
        let Some(lang) = self.config.languages.by_extension(ext) else {
            return base;
        };
        base.overlay(&lang.editor)
    }
}

/// Walk an anyhow error chain to its innermost cause — keeps the
/// status-bar message focused on the actual filesystem / parser error
/// rather than the wrapping context.
pub(super) fn root_cause(e: &anyhow::Error) -> String {
    e.chain()
        .last()
        .map(|x| x.to_string())
        .unwrap_or_else(|| e.to_string())
}

/// True if the error chain contains an `io::Error` with `NotFound` kind —
/// i.e. the LSP server binary isn't on `PATH`. Lets us silently skip
/// built-in defaults the user hasn't installed.
pub(super) fn is_command_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}
