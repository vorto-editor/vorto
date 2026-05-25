//! In-editor `:agent` command — launch an AI agent in a terminal pane.
//!
//! vorto has no terminal emulator of its own, so the agent runs in a new
//! pane opened by the multiplexer hosting vorto (tmux or zellij). Flow:
//!
//! * detect the multiplexer; error out when neither is running (vorto
//!   has no window to attach a standalone terminal to);
//! * resolve the configured default agent — or open a picker when none
//!   is set, persisting the choice the first time (see [`Self::select_agent`]);
//! * ask the backend to open the pane.
//!
//! Reuse of an already-open agent pane and `:agent <subcommand> @path`
//! prompt-passing are deliberately out of scope here — the
//! [`crate::agent::Multiplexer`] trait is where those will land.

use crate::agent::{self, AgentSpec, Multiplexer};

use super::{App, Toast, root_cause};

impl App {
    /// `:agent [..]`. Bare `:agent` launches the default agent (or opens
    /// the picker). Any argument is an unsupported subcommand for now —
    /// rejected with a hint rather than silently launching.
    pub(super) fn run_agent_command(&mut self, rest: &str) {
        let rest = rest.trim();
        if !rest.is_empty() {
            self.push_toast(Toast::error(format!(
                "unknown agent subcommand: {rest} (only bare :agent is supported)"
            )));
            return;
        }
        let Some(mux) = agent::detect() else {
            self.push_toast(Toast::error(
                "no terminal multiplexer detected — run vorto inside tmux or zellij",
            ));
            return;
        };
        match self.config.agents.default.clone() {
            Some(name) => self.launch_agent(&name, mux.as_ref()),
            None => self.open_agent_picker(),
        }
    }

    /// Resolve `name` against the catalog, then focus the existing agent
    /// pane if one is still open, otherwise open a new one. Toasts on an
    /// unknown name or a backend failure.
    fn launch_agent(&mut self, name: &str, mux: &dyn Multiplexer) {
        let Some(spec) = self.config.agents.find(name).cloned() else {
            self.push_toast(Toast::error(format!("unknown agent: {name}")));
            return;
        };
        // Reuse: if we opened a pane earlier this session and it's still
        // alive, jump to it instead of spawning a second one.
        if let Some(id) = self.agent_pane.clone() {
            if mux.focus_if_alive(&id) {
                self.push_toast(Toast::info(format!("focused {} pane", spec.name)));
                return;
            }
            // Stale — the pane was closed. Drop it and open afresh below.
            self.agent_pane = None;
        }
        self.spawn_agent_pane(&spec, mux);
    }

    fn spawn_agent_pane(&mut self, spec: &AgentSpec, mux: &dyn Multiplexer) {
        match mux.open_agent_pane(spec, &self.startup_cwd) {
            Ok(pane_id) => {
                self.agent_pane = pane_id;
                self.push_toast(Toast::info(format!(
                    "launched {} in {} pane",
                    spec.name,
                    mux.name()
                )));
            }
            Err(e) => self.push_toast(Toast::error(format!(
                "agent launch failed: {}",
                root_cause(&e)
            ))),
        }
    }

    /// Open the agent picker (shown when no default is configured).
    fn open_agent_picker(&mut self) {
        let names: Vec<String> = self
            .config
            .agents
            .agents
            .iter()
            .map(|a| a.name.clone())
            .collect();
        if names.is_empty() {
            self.push_toast(Toast::error("no agents available to launch"));
            return;
        }
        self.prompt.open_agent_picker(names);
    }

    /// Picker selection. Persists `name` as the default (first time only,
    /// preserving the rest of the config) so the picker doesn't reappear,
    /// then launches it.
    pub(super) fn select_agent(&mut self, name: String) {
        let Some(mux) = agent::detect() else {
            self.push_toast(Toast::error(
                "no terminal multiplexer detected — run vorto inside tmux or zellij",
            ));
            return;
        };
        if self.config.agents.default.is_none() {
            match crate::config::persist_default_agent(&name) {
                Ok(path) => {
                    self.config.agents.default = Some(name.clone());
                    self.push_toast(Toast::info(format!(
                        "default agent set to {name} ({})",
                        path.display()
                    )));
                }
                Err(e) => self.push_toast(Toast::error(format!(
                    "couldn't save default agent: {}",
                    root_cause(&e)
                ))),
            }
        }
        self.launch_agent(&name, mux.as_ref());
    }
}
