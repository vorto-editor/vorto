//! Off-main-thread workers for file open: tree-sitter highlighter
//! build, LSP server spawn + initialize, and the fuzzy-preview producer
//! handoff. Each worker fires an [`AppEvent`] when done; the matching
//! `handle_*_ready` reconciles the result against the current
//! `open_gen` (so a stale result from a previous file open is dropped
//! instead of clobbering the active buffer).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use anyhow::Result;

use crate::event::AppEvent;
use crate::finder::PreviewEntry;
use crate::lsp::{self, LspClient};
use crate::syntax::Engine;
use crate::vlog;

use super::lsp_coordinator::client_key;
use super::{App, Toast, is_command_not_found, root_cause};

impl App {
    /// Build a tree-sitter `Engine` for `path` off the main thread
    /// (grammar dlopen + query compile + initial full parse). The result
    /// arrives via [`AppEvent::EngineReady`] and is installed on the
    /// buffer in [`Self::handle_engine_ready`] when the generation
    /// still matches.
    pub(super) fn spawn_engine_worker(&mut self, path: &Path) {
        self.active_doc_mut().highlighter = None;
        let Some(spec) = self.config.languages.by_path(path).cloned() else {
            return;
        };
        // When the grammar/queries aren't installed but a recipe exists,
        // offer to install rather than spawning a worker that would just
        // fail with a "highlight failed" toast.
        if self.maybe_prompt_grammar_install(&spec) {
            return;
        }
        let loader = Arc::clone(&self.loader);
        let tx = self.event_tx.clone();
        let generation = self.open_gen;
        // Snapshot the source we'll parse against. The user might edit
        // the buffer while the worker runs; we recover by re-`refresh`-
        // ing on the main thread when the highlighter arrives.
        let source = self.active_doc().lines.join("\n");
        let buffer_version = self.active_doc().version;
        thread::spawn(move || {
            let result = (|| -> Result<Engine> {
                let mut h = loader.lock().unwrap().engine_for(&spec)?;
                h.refresh(&source, buffer_version);
                Ok(h)
            })();
            let _ = tx.send(AppEvent::EngineReady { generation, result });
        });
    }

    /// Spawn one LSP server per `[[languages.<lang>.lsp]]` entry off
    /// the main thread. Servers that are already running for this
    /// language get an inline `didOpen` (cheap — no process spawn);
    /// new ones fire `initialize` on a worker thread and arrive back
    /// via [`AppEvent::LspReady`].
    pub(super) fn spawn_lsp_worker(&mut self, path: &Path) {
        let Some(spec) = self.config.languages.by_path(path).cloned() else {
            return;
        };
        if spec.lsp.is_empty() {
            return;
        }
        let lang_name = spec.name.clone();
        // Per-extension `languageId` override — e.g. `.tsx` advertises
        // `"typescriptreact"` even though our internal language name
        // is `"tsx"`. The override is extension-keyed because the LSP
        // protocol's id space is extension-driven; bare-filename
        // languages (Dockerfile, Makefile) fall back to the language
        // name, which is already the right answer for them.
        let language_id_override = path
            .extension()
            .and_then(|s| s.to_str())
            .and_then(|ext| self.config.languages.language_id_for_extension(ext))
            .map(str::to_string);

        for mut lsp_cfg in spec.lsp {
            if let Some(ref id) = language_id_override {
                lsp_cfg.language_id = Some(id.clone());
            }
            let key = client_key(&lang_name, &lsp_cfg.name);

            if self.lsp.has_client(&key) {
                let text = self.active_doc().lines.join("\n");
                if let Err(e) = self.lsp.did_open(&key, &lang_name, path, &text) {
                    self.push_toast(Toast::fatal(format!(
                        "lsp didOpen ({}): {}",
                        key,
                        root_cause(&e)
                    )));
                }
                continue;
            }

            let tx = self.event_tx.clone();
            let emit = self.lsp.make_emit();
            let startup_cwd = self.lsp.startup_cwd().to_path_buf();
            let generation = self.open_gen;
            let path_buf = path.to_path_buf();
            let lang_for_thread = lang_name.clone();
            let key_for_thread = key.clone();
            let cfg = lsp_cfg;

            thread::spawn(move || {
                let root_dir = lsp::discover_root(&startup_cwd, Some(&path_buf), &cfg.root_markers);
                let root_uri = lsp::path_to_uri(&root_dir);
                vlog!(
                    "lsp spawn start key={} cmd={} root={}",
                    key_for_thread,
                    cfg.command,
                    root_dir.display(),
                );
                let result =
                    LspClient::spawn(&key_for_thread, &lang_for_thread, &cfg, &root_uri, emit);
                match &result {
                    Ok(_) => vlog!("lsp spawn ok key={}", key_for_thread),
                    Err(e) => vlog!("lsp spawn err key={} err={:#}", key_for_thread, e),
                }
                let _ = tx.send(AppEvent::LspReady {
                    generation,
                    client_key: key_for_thread,
                    lang: lang_for_thread,
                    path: path_buf,
                    result,
                });
            });
        }
    }

    /// Read the HEAD blob for `path` off the main thread and deliver it
    /// via [`AppEvent::VcsBaseReady`]. `vcs::head_blob_lines` shells out
    /// to git twice (`rev-parse` + `show`), which on a cold cache or a
    /// large repo is slow enough to be felt at the first paint — so we
    /// keep it off the open path and let the gutter fill in a frame
    /// later. Reconciled by `open_gen` in [`Self::handle_vcs_base_ready`].
    pub(super) fn spawn_vcs_worker(&mut self) {
        let Some(path) = self.active_doc().path.clone() else {
            // Scratch buffer — nothing to diff against.
            return;
        };
        let tx = self.event_tx.clone();
        let generation = self.open_gen;
        thread::spawn(move || {
            let base = crate::vcs::head_blob_lines(&path);
            let _ = tx.send(AppEvent::VcsBaseReady {
                generation,
                path,
                base,
            });
        });
    }

    /// Install the HEAD base on the active buffer. Dropped when
    /// `generation` is stale (another file was opened since) or the
    /// active buffer's path drifted from the one we read — either way
    /// the result no longer describes what's on screen.
    pub fn handle_vcs_base_ready(
        &mut self,
        generation: u64,
        path: PathBuf,
        base: Option<Vec<String>>,
    ) {
        if generation != self.open_gen {
            return;
        }
        if self.active_doc().path.as_deref() != Some(path.as_path()) {
            return;
        }
        self.active_doc_mut().vcs_base = base;
        self.active_doc().vcs_diff.borrow_mut().take();
    }

    /// Install a freshly-built highlighter on the active buffer. Dropped
    /// when `generation` doesn't match — the user opened another file
    /// while the worker was running.
    pub fn handle_engine_ready(&mut self, generation: u64, result: Result<Engine>) {
        if generation != self.open_gen {
            return;
        }
        match result {
            Ok(mut h) => {
                // The user may have edited the buffer while the worker
                // was parsing the snapshot we handed it. Re-`refresh`
                // here so the tree matches the live source.
                if self.active_doc().version != 0 {
                    let source = self.active_doc().lines.join("\n");
                    h.refresh(&source, self.active_doc().version);
                }
                if let Some(msg) = h.warnings.drain(..).next() {
                    self.push_toast(Toast::error(msg));
                }
                self.active_doc_mut().highlighter = Some(h);
            }
            Err(e) => {
                self.push_toast(Toast::fatal(format!("highlight: {}", root_cause(&e))));
            }
        }
    }

    /// Adopt a freshly-spawned LSP client and send the deferred
    /// `didOpen`. Dropped when `generation` doesn't match — the freshly
    /// spawned client gets dropped here, which closes its stdin and
    /// shuts the server down. `client_key` is the unique identifier
    /// the coordinator stores the client under (typically
    /// `"<lang>::<server-name>"`); a single `<lang>` may produce
    /// multiple `LspReady` events when several servers are configured.
    pub fn handle_lsp_ready(
        &mut self,
        generation: u64,
        client_key: String,
        lang: String,
        path: PathBuf,
        result: Result<LspClient>,
    ) {
        if generation != self.open_gen {
            return;
        }
        let client = match result {
            Ok(c) => c,
            Err(e) => {
                // Built-in defaults reference servers most users won't
                // have installed. Stay quiet when the binary isn't on
                // PATH; surface every other failure.
                if !is_command_not_found(&e) {
                    self.push_toast(Toast::fatal(format!(
                        "lsp ({}): {}",
                        client_key,
                        root_cause(&e)
                    )));
                } else {
                    vlog!("lsp not on PATH key={} err={:#}", client_key, e);
                }
                return;
            }
        };
        if !self.lsp.attach_client(&client_key, client) {
            // A client for this key was attached between spawn and
            // now (parallel open of another file with the same
            // language). The freshly-spawned one is dropped here.
            return;
        }
        // Re-snapshot the buffer — the user may have edited while the
        // server was initializing.
        let text = self.active_doc().lines.join("\n");
        if let Err(e) = self.lsp.did_open(&client_key, &lang, &path, &text) {
            self.push_toast(Toast::fatal(format!(
                "lsp didOpen ({}): {}",
                client_key,
                root_cause(&e)
            )));
        }
        self.lsp.set_last_synced_version(self.active_doc().version);
    }

    /// Insert a freshly-built fuzzy preview into the LRU. `last_preview_
    /// request` is cleared when the arriving path matches it so the
    /// draw path will re-enqueue if the user has already moved on.
    pub fn handle_preview_ready(&mut self, entry: PreviewEntry) {
        let mut pending = self.last_preview_request.borrow_mut();
        if pending.as_deref() == Some(entry.path.as_path()) {
            *pending = None;
        }
        self.preview_lru.borrow_mut().insert(entry);
    }
}
