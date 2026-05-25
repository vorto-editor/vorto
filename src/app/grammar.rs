//! In-editor `:grammar` command — the interactive counterpart to the
//! `vorto grammar` CLI subcommand.
//!
//! Three operations, dispatched by [`Self::run_grammar_command`]:
//!
//! * bare / `list` — open the [`Prompt::GrammarList`] modal (browse +
//!   install/remove inline).
//! * `install <name>… | --all` — kick off async installs without the
//!   modal (for users who know the name).
//! * `remove <name>…` — delete installed libraries.
//!
//! Installs run on a worker thread (`git`/tarball fetch + cc compile is
//! seconds-long) and report back via [`AppEvent::GrammarInstalled`].
//! Because the [`Loader`](crate::syntax::Loader) never negatively-caches
//! a missing grammar, the completion handler can simply re-run the
//! highlighter build for every open buffer using the new grammar and the
//! colors appear a frame later — no restart needed.

use std::thread;

use anyhow::Result;

use crate::event::AppEvent;
use crate::grammar::recipe::GrammarRecipe;
use crate::prompt::{GrammarRow, GrammarState};

use super::{App, Toast, root_cause};

/// One `:grammar <sub>` subcommand. Source of truth for the hint panel
/// ([`crate::ui::hints`]) and the `:` prompt's Tab completion
/// ([`crate::prompt`]); [`App::run_grammar_command`] resolves an input
/// token against this list so names/aliases live in exactly one place.
///
/// No `handler` field (unlike [`crate::app::COPILOT_SUBCOMMANDS`]):
/// grammar subcommands take arguments, so dispatch stays a `match` on
/// the canonical name rather than a nullary fn pointer.
pub struct GrammarSubcommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
}

pub const GRAMMAR_SUBCOMMANDS: &[GrammarSubcommand] = &[
    GrammarSubcommand {
        name: "list",
        aliases: &["ls"],
        description: "browse, install & remove (modal)",
    },
    GrammarSubcommand {
        name: "install",
        aliases: &["add"],
        description: "install <name>... | --all",
    },
    GrammarSubcommand {
        name: "remove",
        aliases: &["rm", "uninstall"],
        description: "remove <name>...",
    },
];

impl App {
    /// `:grammar [list|install|remove] …`. Bare `:grammar` opens the
    /// browse modal; the subcommands mirror the CLI so muscle memory
    /// (`:grammar install rust`) carries over. Names/aliases are resolved
    /// through [`GRAMMAR_SUBCOMMANDS`] so the hint panel and the
    /// dispatcher never drift.
    pub(super) fn run_grammar_command(&mut self, sub: &str) {
        let sub = sub.trim();
        let (cmd, rest) = match sub.split_once(char::is_whitespace) {
            Some((c, r)) => (c, r.trim()),
            None => (sub, ""),
        };
        // Bare `:grammar` defaults to the list modal.
        let canonical = if cmd.is_empty() {
            "list"
        } else {
            match GRAMMAR_SUBCOMMANDS
                .iter()
                .find(|s| s.name == cmd || s.aliases.contains(&cmd))
            {
                Some(s) => s.name,
                None => {
                    self.push_toast(Toast::error(format!("unknown grammar subcommand: {cmd}")));
                    return;
                }
            }
        };
        match canonical {
            "list" => self.open_grammar_list(),
            "install" => self.grammar_install_cmd(rest),
            "remove" => self.grammar_remove_cmd(rest),
            _ => unreachable!("canonical comes from GRAMMAR_SUBCOMMANDS"),
        }
    }

    /// Decide whether opening a file with language `spec` should pause
    /// to offer a grammar install instead of building a highlighter.
    ///
    /// Returns `true` when the caller should **skip** spawning the engine
    /// worker:
    /// * the grammar/queries aren't fully on disk, a recipe exists, and
    ///   we haven't asked this session → open the confirm modal;
    /// * we already asked this session and it's still missing → stay
    ///   quiet (no modal, no error toast — the buffer is plain text).
    ///
    /// Returns `false` (spawn normally) when the grammar is fully
    /// installed, or when there's no recipe to install from (preserving
    /// the prior behavior, including the worker's error toast for a
    /// genuinely broken/absent grammar the user must supply themselves).
    pub(super) fn maybe_prompt_grammar_install(&mut self, spec: &crate::config::Language) -> bool {
        let grammar = &spec.grammar;
        let fully_installed = {
            let grammar_dir = spec
                .grammar_dir
                .as_deref()
                .unwrap_or(&self.config.grammar_dir);
            let query_dir = spec.query_dir.as_deref().unwrap_or(&self.config.query_dir);
            crate::grammar::build::is_fully_installed(grammar, grammar_dir, query_dir)
        };
        if fully_installed {
            return false;
        }
        let has_recipe = crate::grammar::cli::merged_recipes(&self.config.grammars)
            .iter()
            .any(|r| r.name == grammar.as_str());
        if !has_recipe {
            return false;
        }
        if self.asked_grammars.contains(grammar) {
            return true;
        }
        // Don't paint over a modal/picker the user is already in — leave
        // `asked_grammars` untouched so a later open re-evaluates.
        if self.prompt.is_open() {
            return true;
        }
        self.asked_grammars.insert(grammar.clone());
        self.prompt
            .open_grammar_install_prompt(grammar.clone(), spec.name.clone());
        true
    }

    /// Build the modal rows from the config-aware recipe catalog
    /// (built-ins overlaid with `[grammars.*]`), tagged with on-disk
    /// install status, sorted by name.
    fn open_grammar_list(&mut self) {
        let recipes = crate::grammar::cli::merged_recipes(&self.config.grammars);
        let grammar_dir = self.config.grammar_dir.as_path();
        let mut rows: Vec<GrammarRow> = recipes
            .iter()
            .map(|r| {
                let installed =
                    crate::grammar::build::installed_path(r.name, grammar_dir).is_some();
                GrammarRow {
                    name: r.name.to_string(),
                    state: if installed {
                        GrammarState::Installed
                    } else {
                        GrammarState::Missing
                    },
                }
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        self.prompt.open_grammar_list(rows);
    }

    /// `:grammar install <name>… | --all` — spawn an install worker per
    /// requested grammar, skipping any already on disk. Names with no
    /// recipe are reported and skipped.
    fn grammar_install_cmd(&mut self, rest: &str) {
        let recipes = crate::grammar::cli::merged_recipes(&self.config.grammars);
        let names: Vec<String> = if rest == "--all" {
            recipes.iter().map(|r| r.name.to_string()).collect()
        } else {
            rest.split_whitespace().map(str::to_string).collect()
        };
        if names.is_empty() {
            self.push_toast(Toast::error("usage: :grammar install <name>... | --all"));
            return;
        }

        let mut started = Vec::new();
        for name in &names {
            let Some(recipe) = recipes.iter().find(|r| r.name == name.as_str()) else {
                self.push_toast(Toast::error(format!("unknown grammar: {name}")));
                continue;
            };
            if crate::grammar::build::installed_path(recipe.name, &self.config.grammar_dir)
                .is_some()
            {
                self.push_toast(Toast::info(format!("{name} already installed")));
                continue;
            }
            self.spawn_grammar_install(recipe.clone());
            started.push(name.clone());
        }
        if !started.is_empty() {
            self.push_toast(Toast::info(format!("installing: {}", started.join(", "))));
        }
    }

    /// `:grammar remove <name>…` — delete installed libraries. Note the
    /// already-`dlopen`'d library stays mapped for this process, so open
    /// buffers keep their current highlighting until the next launch;
    /// the removal takes full effect then.
    fn grammar_remove_cmd(&mut self, rest: &str) {
        let names: Vec<&str> = rest.split_whitespace().collect();
        if names.is_empty() {
            self.push_toast(Toast::error("usage: :grammar remove <name>..."));
            return;
        }
        for name in names {
            self.remove_grammar(name);
        }
    }

    /// Delete a single grammar library from disk. Shared by the modal's
    /// `d` key and `:grammar remove`.
    pub(super) fn remove_grammar(&mut self, name: &str) {
        match crate::grammar::build::remove(name, &self.config.grammar_dir) {
            Ok(true) => self.push_toast(Toast::info(format!("grammar removed: {name}"))),
            Ok(false) => self.push_toast(Toast::info(format!("not installed: {name}"))),
            Err(e) => self.push_toast(Toast::error(format!("remove {name}: {}", root_cause(&e)))),
        }
    }

    /// Look up `name` in the recipe catalog and spawn its install
    /// worker. Used by the `:grammar` modal, which only carries the
    /// grammar name. A missing recipe reverts the modal row (since the
    /// row was optimistically flipped to "installing") and toasts.
    pub(super) fn spawn_grammar_install_by_name(&mut self, name: &str) {
        let recipes = crate::grammar::cli::merged_recipes(&self.config.grammars);
        match recipes.iter().find(|r| r.name == name) {
            Some(recipe) => {
                self.spawn_grammar_install(recipe.clone());
                self.push_toast(Toast::info(format!("installing {name}…")));
            }
            None => {
                self.prompt.grammar_set_state(name, GrammarState::Missing);
                self.push_toast(Toast::error(format!("unknown grammar: {name}")));
            }
        }
    }

    /// Spawn a worker that fetches + compiles `recipe` and reports back
    /// via [`AppEvent::GrammarInstalled`]. The recipe's `&'static str`
    /// fields make it `Send`; the dirs are cloned in.
    pub(super) fn spawn_grammar_install(&self, recipe: GrammarRecipe) {
        let tx = self.event_tx.clone();
        let grammar_dir = self.config.grammar_dir.clone();
        let query_dir = self.config.query_dir.clone();
        let name = recipe.name.to_string();
        thread::spawn(move || {
            let result =
                crate::grammar::build::install(&recipe, &grammar_dir, &query_dir).map(|_| ());
            let _ = tx.send(AppEvent::GrammarInstalled { name, result });
        });
    }

    /// Worker completion. On success, update the modal row (if still
    /// open) and re-highlight every open buffer using this grammar; on
    /// failure, surface the error and flip the row back to missing.
    pub fn handle_grammar_installed(&mut self, name: String, result: Result<()>) {
        match result {
            Ok(()) => {
                self.prompt
                    .grammar_set_state(&name, GrammarState::Installed);
                self.push_toast(Toast::info(format!("grammar installed: {name}")));
                self.rehighlight_for_grammar(&name);
            }
            Err(e) => {
                self.prompt.grammar_set_state(&name, GrammarState::Missing);
                self.push_toast(Toast::fatal(format!(
                    "grammar install failed ({name}): {}",
                    root_cause(&e)
                )));
            }
        }
    }

    /// Rebuild highlighters for every open buffer whose language uses
    /// `grammar`. The active buffer goes through the normal off-thread
    /// [`Self::spawn_engine_worker`]; parked (inactive-pane) buffers are
    /// rebuilt synchronously here — they're already resident and this is
    /// a deliberate one-shot action, so the brief lock is acceptable.
    /// Sleeping buffers need nothing: their highlighter is rebuilt on
    /// thaw anyway.
    fn rehighlight_for_grammar(&mut self, grammar: &str) {
        // Active buffer.
        if let Some(path) = self.buffer.path.clone() {
            let uses = self
                .config
                .languages
                .by_path(&path)
                .is_some_and(|spec| spec.grammar == grammar);
            if uses {
                self.spawn_engine_worker(&path);
            }
        }

        // Parked pane buffers. Bind the disjoint fields to locals so the
        // mutable `parked_buffers` borrow doesn't collide with the
        // immutable `config` / `loader` borrows.
        let languages = &self.config.languages;
        let loader = &self.loader;
        for buf in self.parked_buffers.values_mut() {
            let Some(path) = buf.path.clone() else {
                continue;
            };
            let Some(spec) = languages.by_path(&path) else {
                continue;
            };
            if spec.grammar != grammar {
                continue;
            }
            if let Ok(mut engine) = loader.lock().unwrap().engine_for(spec) {
                let source = buf.lines.join("\n");
                engine.refresh(&source, buf.version);
                buf.highlighter = Some(engine);
            }
        }
    }
}
