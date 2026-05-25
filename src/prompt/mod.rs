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
    GrammarList {
        rows: Vec<GrammarRow>,
        selected: usize,
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
        self.state = Prompt::GrammarList { rows, selected: 0 };
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
        // the modal — the user queues an install and keeps browsing.
        // Esc/Ctrl-C still close (handled below, after navigation).
        if let Prompt::GrammarList { rows, selected } = &mut self.state {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            if key.code == KeyCode::Esc || ctrl_c {
                self.close();
                return PromptOutcome::Cancelled;
            }
            let last = rows.len().saturating_sub(1);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Char('p') if ctrl => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(last),
                KeyCode::Char('n') if ctrl => *selected = (*selected + 1).min(last),
                KeyCode::Enter | KeyCode::Char('i') => {
                    // Only missing grammars are installable; an already-
                    // installed or in-flight row is a no-op.
                    if let Some(row) = rows.get_mut(*selected)
                        && row.state == GrammarState::Missing
                    {
                        row.state = GrammarState::Installing;
                        return PromptOutcome::InstallGrammar(row.name.clone());
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('x') => {
                    if let Some(row) = rows.get_mut(*selected)
                        && row.state == GrammarState::Installed
                    {
                        row.state = GrammarState::Missing;
                        return PromptOutcome::RemoveGrammar(row.name.clone());
                    }
                }
                _ => {}
            }
            return PromptOutcome::Nothing;
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
            Prompt::Explorer(_) | Prompt::GrammarList { .. } => {
                // Both are fully handled by their early intercepts above
                // (they own Esc/Enter and per-mode dispatch). Reaching
                // this arm means the early branch didn't match — i.e.
                // none, which shouldn't happen — but stay defensive.
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
            Prompt::Explorer(_) | Prompt::GrammarList { .. } => PromptOutcome::Nothing,
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
