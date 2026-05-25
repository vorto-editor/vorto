//! Owns the bottom-line prompt (`:cmd`, `/search`, fuzzy pickers,
//! rename input) and translates key events into outcomes the App
//! reacts to.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer_ref::BufferRef;
use crate::finder::{ExplorerMode, ExplorerState, Finder, FuzzyKind, IgnoreOpts};
use crate::lsp::{CodeAction, Location};

mod completion;
mod line_input;

pub use completion::{CommandPrompt, CompletionKind};
pub use line_input::LineInput;
pub(crate) use line_input::apply_line_key;

/// Active prompt state. Mirrors the four ways the user can interact
/// with the bottom-line input: `:` command line, `/` (or `?`) search,
/// fuzzy pickers, and rename.
pub enum Prompt {
    None,
    Command(CommandPrompt),
    Search {
        forward: bool,
        query: LineInput,
    },
    Fuzzy(Finder),
    /// `<space>r` — text input for the new identifier. The cursor and
    /// URI captured at open-time aren't stored here: the LSP rename
    /// request is built against the live cursor at submit, which
    /// matches what the user sees (the cursor is locked while the
    /// prompt is up because Normal-mode input is suspended).
    Rename(LineInput),
    /// `<space>a` — popup menu of LSP code actions, anchored just under
    /// the buffer cursor. Up/Down navigate, Enter submits, Esc cancels.
    /// Filtering is intentionally omitted: action lists are short and
    /// users want to read titles, not type query strings.
    CodeActionMenu {
        actions: Vec<CodeAction>,
        selected: usize,
    },
    /// `K` — read-only popup showing `textDocument/hover` content
    /// anchored at the cursor. j/k/Up/Down/PageUp/PageDown scroll the
    /// content; any other key (including Enter and Esc) closes it.
    Hover {
        content: String,
        scroll: usize,
    },
    /// `:lsp` — read-only, screen-centered modal listing every
    /// configured LSP server and whether it's currently running.
    /// Same key model as [`Self::Hover`]: scroll keys page the
    /// content, any other key dismisses.
    LspStatus {
        content: String,
        scroll: usize,
    },
    /// Copilot device-flow signin: screen-centered modal showing the
    /// user code and verification URL. The clipboard already holds
    /// `code` (the open path sets that up); the modal exists so the
    /// info doesn't scroll away in the toast queue while the user is
    /// in the browser. Any key dismisses; signin_confirm continues to
    /// run in the background.
    CopilotSignin {
        code: String,
        url: String,
    },
    /// `<space>e` — tree file explorer. Owns the full node list, the
    /// expand state, and the live fuzzy query. Submit on a file
    /// produces `OpenRelativeFile`; submit on a dir toggles its
    /// expand state in-place and stays open.
    Explorer(ExplorerState),
    /// `:grammar` — interactive list of tree-sitter grammars with their
    /// install status. Unlike the read-only modals above, this one
    /// drives actions: `j`/`k` move the selection, Enter installs the
    /// selected grammar (asynchronously — the row flips to
    /// [`GrammarState::Installing`] and the worker reports back via
    /// [`crate::event::AppEvent::GrammarInstalled`]), and `d` removes an
    /// installed one. The modal stays open across installs so the user
    /// can queue several.
    ///
    /// `/` opens a live filter — printable keys flow into `query` and the
    /// list narrows to grammars whose name contains it (case-insensitive,
    /// order preserved). Enter leaves the filter input *and* installs the
    /// highlighted grammar (the type-narrow-Enter flow); Esc just leaves
    /// the input. Either way the query stays in effect — a further Esc
    /// clears it, and one more closes the modal. While `filtering`, letter
    /// keys type into the query rather than triggering `j/k/i/d/x`; arrows
    /// and Ctrl-N/P still move the cursor either way.
    GrammarList {
        rows: Vec<GrammarRow>,
        /// Index into the *visible* (filtered) rows — see
        /// [`grammar_visible_indices`] — not into `rows` directly.
        selected: usize,
        /// Live filter text. Empty shows every row.
        query: String,
        /// `/` filter input is live; printable keys edit `query`.
        filtering: bool,
    },
    /// Shown at file-open time when the active language has an install
    /// recipe (built-in or `[grammars.*]`) but its parser/queries aren't
    /// fully installed yet — offered in place of the old "highlight
    /// failed" error toast. `y` installs (async), `n`/Esc dismisses; the
    /// arrow keys (or `h`/`l`/Tab) move between the Yes/No buttons and
    /// Enter confirms the highlighted one. We only ask once per grammar
    /// per session (see [`App::asked_grammars`]).
    GrammarInstallConfirm {
        /// Recipe/grammar name to install on accept.
        grammar: String,
        /// Language name, for the prompt message.
        language: String,
        /// Highlighted button: `true` = Yes (install), `false` = No.
        /// Toggled by the arrow keys; Enter acts on it. Opens on Yes so a
        /// bare Enter still accepts, matching the old behavior.
        accept: bool,
    },
}

/// One grammar entry in the `:grammar` modal.
#[derive(Clone)]
pub struct GrammarRow {
    /// Grammar name — the `.so` stem and the `grammar install <name>`
    /// handle.
    pub name: String,
    pub state: GrammarState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GrammarState {
    /// Library present on disk.
    Installed,
    /// No library yet — Enter installs it.
    Missing,
    /// Install worker is in flight.
    Installing,
}

/// Indices into `rows` that match `query`, case-insensitive substring,
/// order preserved (the list stays alphabetically sorted while it
/// narrows). An empty query matches every row. Shared by the renderer
/// and the key handler so the visible projection can't drift between
/// what's drawn and what `selected` acts on.
pub(crate) fn grammar_visible_indices(rows: &[GrammarRow], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..rows.len()).collect();
    }
    let needle = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Flip the row at `idx` to [`GrammarState::Installing`] and yield its
/// name to install, if it was [`Missing`](GrammarState::Missing). An
/// already-installed or in-flight row is a no-op (returns `None`).
/// Shared by the modal's Enter and `i`.
fn grammar_install_at(rows: &mut [GrammarRow], idx: usize) -> Option<String> {
    let row = rows.get_mut(idx)?;
    (row.state == GrammarState::Missing).then(|| {
        row.state = GrammarState::Installing;
        row.name.clone()
    })
}

/// Flip the row at `idx` back to [`Missing`](GrammarState::Missing) and
/// yield its name to remove, if it was
/// [`Installed`](GrammarState::Installed). No-op otherwise.
fn grammar_remove_at(rows: &mut [GrammarRow], idx: usize) -> Option<String> {
    let row = rows.get_mut(idx)?;
    (row.state == GrammarState::Installed).then(|| {
        row.state = GrammarState::Missing;
        row.name.clone()
    })
}

impl Prompt {
    pub fn is_open(&self) -> bool {
        !matches!(self, Prompt::None)
    }
}

/// What a key event produced. `Nothing` means "input absorbed, prompt
/// stays open"; everything else closes the prompt and asks the caller
/// to act.
pub enum PromptOutcome {
    Nothing,
    /// User pressed Esc / Ctrl-C — prompt closed, no action.
    Cancelled,
    /// `:cmd` submitted. Caller parses and dispatches.
    RunCommand(String),
    /// `/` or `?` submitted.
    Search {
        forward: bool,
        query: String,
    },
    /// Fuzzy file picker submission. The path is relative to
    /// `startup_cwd` — re-anchored by the caller.
    OpenRelativeFile(String),
    /// Fuzzy line picker submission. 0-based row in the active buffer.
    GotoLine(usize),
    /// Fuzzy references picker submission.
    JumpToLocation(Location),
    /// Fuzzy buffer picker submission. The caller maps the
    /// [`BufferRef`] back to an actual buffer load (`Scratch` →
    /// fresh empty buffer, `File(path)` → `open_path`).
    OpenBuffer(BufferRef),
    /// Rename submitted with the new identifier.
    SubmitRename(String),
    /// Code action picker selection. The caller either applies the
    /// embedded `WorkspaceEdit` or sends a `codeAction/resolve` round
    /// trip first when `edit` is `None`.
    SelectCodeAction(CodeAction),
    /// Explorer rename/move — a file or directory was relocated on
    /// disk from `old` to `new` (both absolute). The caller rewrites
    /// any open or sleeping buffer whose path falls under `old` so the
    /// next save lands at the new location instead of recreating the
    /// source. The explorer prompt stays open.
    PathMoved {
        old: PathBuf,
        new: PathBuf,
    },
    /// `:grammar` modal — Enter on a missing grammar. The caller spawns
    /// the install worker; the modal stays open with the row already
    /// flipped to [`GrammarState::Installing`].
    InstallGrammar(String),
    /// `:grammar` modal — `d` on an installed grammar. The caller
    /// deletes the library; the modal stays open with the row flipped
    /// back to [`GrammarState::Missing`].
    RemoveGrammar(String),
}

pub struct PromptController {
    pub state: Prompt,
    /// Side-channel for `Fuzzy(Locations)` pickers — `locations[idx]`
    /// matches the picker's `items[idx]`. Cleared on submit or cancel.
    locations: Vec<Location>,
    /// Side-channel for `Fuzzy(Buffers)` pickers — `buffer_paths[idx]`
    /// is the buffer to open when the user submits the matching item.
    /// Cleared on submit or cancel.
    buffer_paths: Vec<BufferRef>,
}

impl PromptController {
    pub fn new() -> Self {
        Self {
            state: Prompt::None,
            locations: Vec::new(),
            buffer_paths: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Side-channel `Location`s that mirror the active `Locations` picker.
    /// Returns `&[]` for any other prompt state. The UI uses this to read
    /// `locations[idx]` for preview rendering.
    pub fn locations(&self) -> &[Location] {
        &self.locations
    }

    pub fn open_command(&mut self) {
        self.state = Prompt::Command(CommandPrompt::new());
    }

    pub fn open_search(&mut self, forward: bool) {
        self.state = Prompt::Search {
            forward,
            query: LineInput::new(),
        };
    }

    pub fn open_files(
        &mut self,
        startup_cwd: &Path,
        ignore: IgnoreOpts,
        hidden_patterns: &[String],
        max_items: usize,
    ) {
        self.state = Prompt::Fuzzy(Finder::files(
            startup_cwd,
            ignore,
            hidden_patterns,
            max_items,
        ));
    }

    /// `<space>e` — tree file explorer. All dirs start collapsed; the
    /// user expands by pressing Enter / Right on a dir row. Typing
    /// into the query box fuzzy-filters files and auto-expands their
    /// ancestor dirs so matches stay reachable.
    pub fn open_explorer(
        &mut self,
        startup_cwd: &Path,
        ignore: IgnoreOpts,
        hidden_patterns: Vec<String>,
        max_items: usize,
        compact: bool,
    ) {
        self.state = Prompt::Explorer(ExplorerState::new(
            startup_cwd,
            ignore,
            hidden_patterns,
            max_items,
            compact,
        ));
    }

    pub fn open_lines(&mut self, lines: &[String]) {
        self.state = Prompt::Fuzzy(Finder::lines(lines));
    }

    pub fn open_locations(&mut self, items: Vec<String>, locations: Vec<Location>) {
        self.locations = locations;
        self.state = Prompt::Fuzzy(Finder::locations(items));
    }

    /// `<space>d` / `<space>D` — diagnostics picker. Same `Location`
    /// side-channel as references; only the picker kind (and therefore
    /// the title / formatting) differs.
    pub fn open_diagnostics(
        &mut self,
        items: Vec<String>,
        locations: Vec<Location>,
        workspace: bool,
    ) {
        self.locations = locations;
        self.state = Prompt::Fuzzy(Finder::diagnostics(items, workspace));
    }

    /// `<space>/` — workspace-wide content picker. One candidate per
    /// file; on each keystroke, the Finder scans every line of every
    /// file and exposes the matched line numbers via
    /// `MatchItem::line_hits`. Submit jumps to the file's best-scoring
    /// match.
    ///
    /// `locations[i]` is the *base* `Location` for `items[i]`: same
    /// URI, line 0. Submit clones it and overrides the line with
    /// `selection.line_hits[0]`.
    pub fn open_workspace_search(
        &mut self,
        items: Vec<String>,
        file_lines: Vec<Vec<String>>,
        locations: Vec<Location>,
    ) {
        self.locations = locations;
        self.state = Prompt::Fuzzy(Finder::workspace_search(items, file_lines));
    }

    /// Open a fuzzy buffer picker. `items` are the display strings;
    /// `refs` are the matching [`BufferRef`]s in parallel order —
    /// the controller stores them and produces an `OpenBuffer(…)`
    /// outcome on submit.
    pub fn open_buffers(&mut self, items: Vec<String>, refs: Vec<BufferRef>) {
        self.buffer_paths = refs;
        self.state = Prompt::Fuzzy(Finder::buffers(items));
    }

    /// Read-only view of the buffer-picker side-channel, mirroring
    /// [`Self::locations`]. The UI uses this for preview rendering.
    pub fn buffer_paths(&self) -> &[BufferRef] {
        &self.buffer_paths
    }

    pub fn open_rename(&mut self) {
        self.state = Prompt::Rename(LineInput::new());
    }

    /// Open the cursor-anchored code-actions popup. `actions` is consumed
    /// — we own them while the menu is up so submit can hand a fully-
    /// owned `CodeAction` to the caller without an extra clone.
    pub fn open_code_actions(&mut self, actions: Vec<CodeAction>) {
        self.state = Prompt::CodeActionMenu {
            actions,
            selected: 0,
        };
    }

    /// Open a hover popup with the given content. Cursor position is
    /// captured by the renderer at draw time, so `App` doesn't need to
    /// store it.
    pub fn open_hover(&mut self, content: String) {
        self.state = Prompt::Hover { content, scroll: 0 };
    }

    /// Open the `:lsp` status modal with pre-formatted content.
    pub fn open_lsp_status(&mut self, content: String) {
        self.state = Prompt::LspStatus { content, scroll: 0 };
    }

    /// Open the `:grammar` modal with the given rows. Selection starts
    /// at the top.
    pub fn open_grammar_list(&mut self, rows: Vec<GrammarRow>) {
        self.state = Prompt::GrammarList {
            rows,
            selected: 0,
            query: String::new(),
            filtering: false,
        };
    }

    /// Update a grammar row's state in place when the modal is open and
    /// still showing this grammar. No-op otherwise (the user may have
    /// closed the modal before the install worker reported back). Called
    /// from the `GrammarInstalled` handler.
    pub fn grammar_set_state(&mut self, name: &str, state: GrammarState) {
        if let Prompt::GrammarList { rows, .. } = &mut self.state
            && let Some(row) = rows.iter_mut().find(|r| r.name == name)
        {
            row.state = state;
        }
    }

    /// Open the open-time "install this grammar?" confirmation modal.
    pub fn open_grammar_install_prompt(&mut self, grammar: String, language: String) {
        self.state = Prompt::GrammarInstallConfirm {
            grammar,
            language,
            accept: true,
        };
    }

    /// Open the Copilot signin modal. Any prior modal is replaced.
    pub fn open_copilot_signin(&mut self, code: String, url: String) {
        self.state = Prompt::CopilotSignin { code, url };
    }

    pub fn handle_key(&mut self, key: KeyEvent, root: &Path) -> PromptOutcome {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');

        // Explorer steals Esc/Enter ahead of the generic close/submit
        // path so its sub-modes can carry their own meaning: Esc may
        // pop back to selection (from filter / pending input) rather
        // than closing the prompt, and Enter routes to file ops when a
        // pending mode is up.
        if let Prompt::Explorer(state) = &mut self.state {
            if key.code == KeyCode::Esc || ctrl_c {
                if matches!(state.mode, ExplorerMode::Selection) {
                    self.close();
                    return PromptOutcome::Cancelled;
                }
                state.cancel_pending();
                return PromptOutcome::Nothing;
            }
            if key.code == KeyCode::Enter {
                return self.handle_explorer_enter();
            }
            // `y` confirms a pending delete; the explorer's apply_key
            // treats anything else (besides Enter/Esc above) as a
            // cancel.
            if matches!(state.mode, ExplorerMode::PendingDelete)
                && matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'))
            {
                return self.run_explorer_delete();
            }
            state.apply_key(key);
            return PromptOutcome::Nothing;
        }

        // `:grammar` modal owns Enter (install) and `d` (remove) ahead
        // of the generic submit/close path, since Enter must *not* close
        // the modal — the user queues an install and keeps browsing. It
        // also owns `/` (filter) and, while filtering, every printable
        // key (they edit the query rather than firing j/k/i/d/x).
        if let Prompt::GrammarList {
            rows,
            selected,
            query,
            filtering,
        } = &mut self.state
        {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

            // Ctrl-C always closes; Esc peels back one layer at a time —
            // out of filter input, then a non-empty filter, then the modal.
            if ctrl_c {
                self.close();
                return PromptOutcome::Cancelled;
            }
            if key.code == KeyCode::Esc {
                if *filtering {
                    *filtering = false;
                } else if !query.is_empty() {
                    query.clear();
                    *selected = 0;
                } else {
                    self.close();
                    return PromptOutcome::Cancelled;
                }
                return PromptOutcome::Nothing;
            }

            // `selected` indexes the visible (filtered) projection.
            let visible = grammar_visible_indices(rows, query);
            let last = visible.len().saturating_sub(1);

            // Navigation that works in both modes: arrows + Ctrl-N/P never
            // collide with query text.
            match key.code {
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    return PromptOutcome::Nothing;
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(last);
                    return PromptOutcome::Nothing;
                }
                KeyCode::Char('p') if ctrl => {
                    *selected = selected.saturating_sub(1);
                    return PromptOutcome::Nothing;
                }
                KeyCode::Char('n') if ctrl => {
                    *selected = (*selected + 1).min(last);
                    return PromptOutcome::Nothing;
                }
                _ => {}
            }

            if *filtering {
                // Filter input: printable keys (no Ctrl) edit the query;
                // each edit resets the cursor to the top match. Enter
                // leaves the input (the query stays in effect) *and*
                // installs the highlighted grammar if it's missing — the
                // type-narrow-Enter flow installs in one keystroke. Letter
                // actions (i/d/x) aren't available while typing.
                match key.code {
                    KeyCode::Enter => {
                        *filtering = false;
                        if let Some(&idx) = visible.get(*selected)
                            && let Some(name) = grammar_install_at(rows, idx)
                        {
                            return PromptOutcome::InstallGrammar(name);
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *selected = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        query.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
                return PromptOutcome::Nothing;
            }

            // Selection mode: vim motions + single-key actions.
            match key.code {
                KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Char('j') => *selected = (*selected + 1).min(last),
                KeyCode::Char('/') => *filtering = true,
                KeyCode::Enter | KeyCode::Char('i') => {
                    if let Some(&idx) = visible.get(*selected)
                        && let Some(name) = grammar_install_at(rows, idx)
                    {
                        return PromptOutcome::InstallGrammar(name);
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('x') => {
                    if let Some(&idx) = visible.get(*selected)
                        && let Some(name) = grammar_remove_at(rows, idx)
                    {
                        return PromptOutcome::RemoveGrammar(name);
                    }
                }
                _ => {}
            }
            return PromptOutcome::Nothing;
        }

        // Open-time "install this grammar?" confirmation. `y` installs,
        // `n`/Esc/Ctrl-C decline; the arrow keys (or `h`/`l`/Tab) move
        // between the Yes/No buttons and Enter acts on the highlighted
        // one. Stray keys are ignored so the dialog stays put. The App
        // already recorded the grammar in `asked_grammars`, so dismissing
        // won't re-prompt this session.
        if let Prompt::GrammarInstallConfirm {
            grammar, accept, ..
        } = &mut self.state
        {
            // Arrow / Tab navigation toggles the highlighted button and
            // keeps the dialog open.
            match key.code {
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('h')
                | KeyCode::Char('l')
                | KeyCode::Tab
                | KeyCode::BackTab => {
                    *accept = !*accept;
                    return PromptOutcome::Nothing;
                }
                _ => {}
            }

            // Decision keys. Copy the choice out before `close()` so the
            // mutable borrow of `state` is released.
            let decision = if ctrl_c || key.code == KeyCode::Esc {
                Some(false)
            } else {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => Some(true),
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(false),
                    KeyCode::Enter => Some(*accept),
                    _ => None,
                }
            };
            let Some(install) = decision else {
                return PromptOutcome::Nothing;
            };
            let grammar = grammar.clone();
            self.close();
            return if install {
                PromptOutcome::InstallGrammar(grammar)
            } else {
                PromptOutcome::Cancelled
            };
        }

        if key.code == KeyCode::Esc || ctrl_c {
            self.close();
            return PromptOutcome::Cancelled;
        }
        if key.code == KeyCode::Enter {
            return self.submit();
        }

        match &mut self.state {
            Prompt::None => PromptOutcome::Nothing,
            Prompt::Command(cp) => {
                match key.code {
                    KeyCode::Tab => cp.tab(1, root),
                    KeyCode::BackTab => cp.tab(-1, root),
                    _ => {
                        cp.completion = None;
                        apply_line_key(&mut cp.input, key);
                    }
                }
                PromptOutcome::Nothing
            }
            Prompt::Rename(input) => {
                apply_line_key(input, key);
                PromptOutcome::Nothing
            }
            Prompt::Search { query, .. } => {
                apply_line_key(query, key);
                PromptOutcome::Nothing
            }
            Prompt::Fuzzy(finder) => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Up => finder.prev(),
                    KeyCode::Down => finder.next(),
                    KeyCode::Char('n') if ctrl => finder.next(),
                    KeyCode::Char('p') if ctrl => finder.prev(),
                    _ => finder.apply_line_key(key),
                }
                PromptOutcome::Nothing
            }
            Prompt::CodeActionMenu { actions, selected } => {
                let last = actions.len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                    KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        *selected = selected.saturating_sub(1)
                    }
                    KeyCode::Down => *selected = (*selected + 1).min(last),
                    KeyCode::Char('j') => *selected = (*selected + 1).min(last),
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        *selected = (*selected + 1).min(last)
                    }
                    _ => {}
                }
                PromptOutcome::Nothing
            }
            Prompt::Explorer(_)
            | Prompt::GrammarList { .. }
            | Prompt::GrammarInstallConfirm { .. } => {
                // All three are fully handled by their early intercepts
                // above (they own Esc/Enter and per-key dispatch).
                // Reaching this arm means the early branch didn't match —
                // which shouldn't happen — but stay defensive.
                PromptOutcome::Nothing
            }
            Prompt::Hover { scroll, .. } | Prompt::LspStatus { scroll, .. } => {
                // Read-only popup. Esc/Ctrl-C/Enter are intercepted by
                // the top of `handle_key`, so here we only see scroll
                // keys and "anything else" (which we treat as dismiss).
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *scroll = scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *scroll = scroll.saturating_add(1);
                    }
                    KeyCode::PageUp => {
                        *scroll = scroll.saturating_sub(5);
                    }
                    KeyCode::PageDown => {
                        *scroll = scroll.saturating_add(5);
                    }
                    _ => {
                        self.close();
                        return PromptOutcome::Cancelled;
                    }
                }
                PromptOutcome::Nothing
            }
            Prompt::CopilotSignin { .. } => {
                // No scrollable body — any non-Esc/Ctrl-C/Enter key
                // dismisses too. Esc/Ctrl-C handled at the top.
                self.close();
                PromptOutcome::Cancelled
            }
        }
    }

    /// Enter handler for the Explorer prompt. The meaning depends on
    /// the current mode:
    ///   * `Selection` / `Filter` — on a dir, toggle expand; on a file,
    ///     close the prompt and signal `OpenRelativeFile`.
    ///   * `PendingCreate` / `PendingRename` / `PendingMove` — run the
    ///     filesystem op against the input buffer.
    ///   * `PendingDelete` — Enter on the confirmation is treated as
    ///     "yes" (matching the shell convention for default-yes when
    ///     the user explicitly pressed `d` already; we still default
    ///     the chord to N for any other key).
    fn handle_explorer_enter(&mut self) -> PromptOutcome {
        let Prompt::Explorer(state) = &mut self.state else {
            return PromptOutcome::Nothing;
        };
        match state.mode {
            ExplorerMode::Selection | ExplorerMode::Filter => {
                let Some(node) = state.selection() else {
                    return PromptOutcome::Nothing;
                };
                if node.is_dir {
                    state.toggle_selected();
                    state.refilter();
                    return PromptOutcome::Nothing;
                }
                let rel = node.rel_path.clone();
                self.close();
                PromptOutcome::OpenRelativeFile(rel)
            }
            ExplorerMode::PendingCreate => self.run_explorer_create(),
            ExplorerMode::PendingRename => self.run_explorer_rename(),
            ExplorerMode::PendingMove => self.run_explorer_move(),
            ExplorerMode::PendingDelete => self.run_explorer_delete(),
        }
    }

    fn run_explorer_create(&mut self) -> PromptOutcome {
        let Prompt::Explorer(state) = &mut self.state else {
            return PromptOutcome::Nothing;
        };
        let Some(input) = state.action.as_ref() else {
            return PromptOutcome::Nothing;
        };
        let raw = input.text.clone();
        match state.perform_create(&raw) {
            Ok(_) => {
                state.cancel_pending();
                PromptOutcome::Nothing
            }
            Err(msg) => {
                state.error = Some(msg);
                PromptOutcome::Nothing
            }
        }
    }

    fn run_explorer_rename(&mut self) -> PromptOutcome {
        let Prompt::Explorer(state) = &mut self.state else {
            return PromptOutcome::Nothing;
        };
        let Some(node) = state.selection() else {
            return PromptOutcome::Nothing;
        };
        let old_rel = node.rel_path.clone();
        let Some(input) = state.action.as_ref() else {
            return PromptOutcome::Nothing;
        };
        let new_name = input.text.clone();
        match state.perform_rename(&old_rel, &new_name) {
            Ok(new_rel) => {
                let old_abs = state.root.join(&old_rel);
                let new_abs = state.root.join(&new_rel);
                state.cancel_pending();
                PromptOutcome::PathMoved {
                    old: old_abs,
                    new: new_abs,
                }
            }
            Err(msg) => {
                state.error = Some(msg);
                PromptOutcome::Nothing
            }
        }
    }

    fn run_explorer_move(&mut self) -> PromptOutcome {
        let Prompt::Explorer(state) = &mut self.state else {
            return PromptOutcome::Nothing;
        };
        let Some(node) = state.selection() else {
            return PromptOutcome::Nothing;
        };
        let old_rel = node.rel_path.clone();
        let Some(input) = state.action.as_ref() else {
            return PromptOutcome::Nothing;
        };
        let new_rel = input.text.clone();
        match state.perform_move(&old_rel, &new_rel) {
            Ok(new_rel) => {
                let old_abs = state.root.join(&old_rel);
                let new_abs = state.root.join(&new_rel);
                state.cancel_pending();
                PromptOutcome::PathMoved {
                    old: old_abs,
                    new: new_abs,
                }
            }
            Err(msg) => {
                state.error = Some(msg);
                PromptOutcome::Nothing
            }
        }
    }

    fn run_explorer_delete(&mut self) -> PromptOutcome {
        let Prompt::Explorer(state) = &mut self.state else {
            return PromptOutcome::Nothing;
        };
        let Some(node) = state.selection() else {
            return PromptOutcome::Nothing;
        };
        let rel = node.rel_path.clone();
        match state.perform_delete(&rel) {
            Ok(()) => {
                state.cancel_pending();
                PromptOutcome::Nothing
            }
            Err(msg) => {
                state.error = Some(msg);
                PromptOutcome::Nothing
            }
        }
    }

    fn close(&mut self) {
        self.state = Prompt::None;
        self.locations.clear();
        self.buffer_paths.clear();
    }

    fn submit(&mut self) -> PromptOutcome {
        let prompt = std::mem::replace(&mut self.state, Prompt::None);
        match prompt {
            Prompt::None => PromptOutcome::Nothing,
            Prompt::Command(cp) => PromptOutcome::RunCommand(cp.input.as_str().trim().to_string()),
            Prompt::Search { forward, query } => PromptOutcome::Search {
                forward,
                query: query.into_string(),
            },
            Prompt::Rename(new_name) => PromptOutcome::SubmitRename(new_name.into_string()),
            Prompt::Fuzzy(finder) => self.submit_fuzzy(finder),
            // Explorer's and the grammar modal's submits are short-
            // circuited inside `handle_key` (their early intercepts own
            // Enter), so these branches are only reached through a
            // future caller that bypasses the key path — no-op.
            Prompt::Explorer(_)
            | Prompt::GrammarList { .. }
            | Prompt::GrammarInstallConfirm { .. } => PromptOutcome::Nothing,
            Prompt::CodeActionMenu {
                mut actions,
                selected,
            } => {
                if selected < actions.len() {
                    PromptOutcome::SelectCodeAction(actions.swap_remove(selected))
                } else {
                    PromptOutcome::Nothing
                }
            }
            // Read-only popups — Enter just dismisses them.
            Prompt::Hover { .. } | Prompt::LspStatus { .. } | Prompt::CopilotSignin { .. } => {
                PromptOutcome::Cancelled
            }
        }
    }

    fn submit_fuzzy(&mut self, finder: Finder) -> PromptOutcome {
        let Some(sel) = finder.selection() else {
            self.locations.clear();
            return PromptOutcome::Nothing;
        };
        match finder.kind {
            FuzzyKind::Files { .. } => {
                PromptOutcome::OpenRelativeFile(finder.items[sel.idx].clone())
            }
            FuzzyKind::Lines => PromptOutcome::GotoLine(sel.idx),
            FuzzyKind::Locations | FuzzyKind::Diagnostics { .. } => {
                let loc = self.locations.get(sel.idx).cloned();
                self.locations.clear();
                match loc {
                    Some(loc) => PromptOutcome::JumpToLocation(loc),
                    None => PromptOutcome::Nothing,
                }
            }
            FuzzyKind::WorkspaceSearch => {
                // Workspace search: `locations[idx]` is the file's base
                // (line 0, char 0); the actual jump target lives on the
                // match item — the matched row, and the column where
                // the substring starts, so the cursor lands on the hit.
                let target_line = sel.line_hits.first().copied().unwrap_or(0) as u32;
                let target_col = sel.match_col;
                let loc = self.locations.get(sel.idx).cloned().map(|mut l| {
                    l.range.start.line = target_line;
                    l.range.start.character = target_col;
                    l.range.end.line = target_line;
                    l.range.end.character = target_col;
                    l
                });
                self.locations.clear();
                match loc {
                    Some(loc) => PromptOutcome::JumpToLocation(loc),
                    None => PromptOutcome::Nothing,
                }
            }
            FuzzyKind::Buffers => {
                let r = self.buffer_paths.get(sel.idx).cloned();
                self.buffer_paths.clear();
                match r {
                    Some(r) => PromptOutcome::OpenBuffer(r),
                    None => PromptOutcome::Nothing,
                }
            }
        }
    }
}

impl Default for PromptController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod grammar_filter_tests {
    use super::*;

    fn rows() -> Vec<GrammarRow> {
        ["bash", "python", "rust", "toml", "typescript"]
            .into_iter()
            .map(|name| GrammarRow {
                name: name.to_string(),
                state: GrammarState::Missing,
            })
            .collect()
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn press(pc: &mut PromptController, key: KeyEvent) -> PromptOutcome {
        pc.handle_key(key, Path::new(""))
    }

    fn names(rows: &[GrammarRow], query: &str) -> Vec<String> {
        grammar_visible_indices(rows, query)
            .into_iter()
            .map(|i| rows[i].name.clone())
            .collect()
    }

    #[test]
    fn visible_indices_empty_query_keeps_order() {
        let rows = rows();
        assert_eq!(
            names(&rows, ""),
            ["bash", "python", "rust", "toml", "typescript"]
        );
    }

    #[test]
    fn visible_indices_substring_case_insensitive() {
        let rows = rows();
        // Matches anywhere in the name, not just a prefix.
        assert_eq!(names(&rows, "t"), ["python", "rust", "toml", "typescript"]);
        assert_eq!(names(&rows, "PY"), ["python"]);
        assert_eq!(names(&rows, "script"), ["typescript"]);
        assert!(names(&rows, "zzz").is_empty());
    }

    #[test]
    fn slash_enters_filter_and_chars_edit_query() {
        let mut pc = PromptController::new();
        pc.open_grammar_list(rows());

        press(&mut pc, key('/'));
        let Prompt::GrammarList {
            query, filtering, ..
        } = &pc.state
        else {
            panic!("expected grammar list");
        };
        assert!(*filtering);
        assert!(query.is_empty());

        for c in "ty".chars() {
            press(&mut pc, key(c));
        }
        let Prompt::GrammarList {
            query,
            filtering,
            selected,
            rows,
        } = &pc.state
        else {
            panic!("expected grammar list");
        };
        assert!(*filtering);
        assert_eq!(query, "ty");
        // Only typescript matches "ty"; selection resets to the top match.
        assert_eq!(*selected, 0);
        assert_eq!(names(rows, query), ["typescript"]);
    }

    #[test]
    fn esc_peels_filter_then_query_then_closes() {
        let mut pc = PromptController::new();
        pc.open_grammar_list(rows());
        press(&mut pc, key('/'));
        press(&mut pc, key('r'));

        // First Esc leaves the input but keeps the query in effect.
        assert!(matches!(
            press(&mut pc, code(KeyCode::Esc)),
            PromptOutcome::Nothing
        ));
        let Prompt::GrammarList {
            query, filtering, ..
        } = &pc.state
        else {
            panic!("expected grammar list");
        };
        assert!(!*filtering);
        assert_eq!(query, "r");

        // Second Esc clears the query (list reopens to full).
        assert!(matches!(
            press(&mut pc, code(KeyCode::Esc)),
            PromptOutcome::Nothing
        ));
        let Prompt::GrammarList { query, .. } = &pc.state else {
            panic!("expected grammar list");
        };
        assert!(query.is_empty());

        // Third Esc closes the modal.
        assert!(matches!(
            press(&mut pc, code(KeyCode::Esc)),
            PromptOutcome::Cancelled
        ));
        assert!(matches!(pc.state, Prompt::None));
    }

    #[test]
    fn enter_in_filter_installs_and_leaves_input() {
        let mut pc = PromptController::new();
        pc.open_grammar_list(rows());
        press(&mut pc, key('/'));
        press(&mut pc, key('t'));
        press(&mut pc, key('o')); // "to" → only "toml"

        // Enter installs the highlighted match and leaves the filter
        // input, keeping the query in effect.
        match press(&mut pc, code(KeyCode::Enter)) {
            PromptOutcome::InstallGrammar(name) => assert_eq!(name, "toml"),
            _ => panic!("expected InstallGrammar(toml)"),
        }
        let Prompt::GrammarList {
            rows,
            query,
            filtering,
            ..
        } = &pc.state
        else {
            panic!("expected grammar list still open");
        };
        assert!(!*filtering);
        assert_eq!(query, "to");
        let toml = rows.iter().find(|r| r.name == "toml").unwrap();
        assert!(matches!(toml.state, GrammarState::Installing));
    }

    #[test]
    fn enter_in_selection_mode_installs() {
        let mut pc = PromptController::new();
        pc.open_grammar_list(rows());
        // No filter — Enter on the first row installs it.
        match press(&mut pc, code(KeyCode::Enter)) {
            PromptOutcome::InstallGrammar(name) => assert_eq!(name, "bash"),
            _ => panic!("expected InstallGrammar(bash)"),
        }
    }

    #[test]
    fn confirm_opens_on_yes_and_bare_enter_installs() {
        let mut pc = PromptController::new();
        pc.open_grammar_install_prompt("rust".into(), "Rust".into());
        match press(&mut pc, code(KeyCode::Enter)) {
            PromptOutcome::InstallGrammar(name) => assert_eq!(name, "rust"),
            _ => panic!("expected InstallGrammar(rust)"),
        }
        assert!(matches!(pc.state, Prompt::None));
    }

    #[test]
    fn confirm_arrow_toggles_then_enter_declines() {
        let mut pc = PromptController::new();
        pc.open_grammar_install_prompt("rust".into(), "Rust".into());
        // Move to the No button; the dialog stays open.
        assert!(matches!(
            press(&mut pc, code(KeyCode::Right)),
            PromptOutcome::Nothing
        ));
        let Prompt::GrammarInstallConfirm { accept, .. } = &pc.state else {
            panic!("expected confirm dialog");
        };
        assert!(!*accept);
        // Enter now acts on the highlighted "No".
        assert!(matches!(
            press(&mut pc, code(KeyCode::Enter)),
            PromptOutcome::Cancelled
        ));
        assert!(matches!(pc.state, Prompt::None));
    }

    #[test]
    fn confirm_y_and_n_pick_directly() {
        let mut pc = PromptController::new();
        pc.open_grammar_install_prompt("rust".into(), "Rust".into());
        // `n` declines even though Yes is highlighted.
        assert!(matches!(press(&mut pc, key('n')), PromptOutcome::Cancelled));

        let mut pc = PromptController::new();
        pc.open_grammar_install_prompt("rust".into(), "Rust".into());
        // Highlight No, then `y` still installs.
        press(&mut pc, code(KeyCode::Left));
        match press(&mut pc, key('y')) {
            PromptOutcome::InstallGrammar(name) => assert_eq!(name, "rust"),
            _ => panic!("expected InstallGrammar(rust)"),
        }
    }

    #[test]
    fn confirm_stray_key_keeps_dialog_open() {
        let mut pc = PromptController::new();
        pc.open_grammar_install_prompt("rust".into(), "Rust".into());
        assert!(matches!(press(&mut pc, key('q')), PromptOutcome::Nothing));
        assert!(matches!(pc.state, Prompt::GrammarInstallConfirm { .. }));
    }

    #[test]
    fn letter_keys_type_into_query_not_install() {
        let mut pc = PromptController::new();
        pc.open_grammar_list(rows());
        press(&mut pc, key('/'));
        // `i` is an install shortcut in selection mode, but here it must
        // be query text — no install fires.
        let outcome = press(&mut pc, key('i'));
        assert!(matches!(outcome, PromptOutcome::Nothing));
        let Prompt::GrammarList { query, .. } = &pc.state else {
            panic!("expected grammar list");
        };
        assert_eq!(query, "i");
    }
}
