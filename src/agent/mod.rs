//! AI-agent launcher domain types, shared between the config layer
//! (which resolves the agent catalog + default) and the `:agent` command
//! (which picks a backend and opens a pane). Pure data + the
//! [`Multiplexer`] abstraction; no `App` dependency, so it stays
//! unit-testable.

mod mux;

pub use mux::{Multiplexer, detect};

/// A launchable AI agent: a display name plus the command line to run in
/// the new pane. Resolved from the built-in catalog overlaid with the
/// user's `[agents.<name>]` config blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// Catalog key — what the picker shows and `[agent].default` names.
    pub name: String,
    /// Executable to launch (looked up on `PATH`).
    pub command: String,
    /// Extra arguments passed to `command`.
    pub args: Vec<String>,
}

/// Built-in agent catalog. Offered by the picker when the user has no
/// `[agents.*]` of their own; any entry can be overridden by a config
/// block of the same name. Commands assume the agent's CLI is on `PATH`.
pub fn builtin_agents() -> Vec<AgentSpec> {
    ["claude", "codex", "gemini", "aider"]
        .into_iter()
        .map(|name| AgentSpec {
            name: name.to_string(),
            command: name.to_string(),
            args: Vec::new(),
        })
        .collect()
}
