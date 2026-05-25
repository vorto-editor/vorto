//! AI-agent launcher domain types, shared between the config layer
//! (which resolves the agent catalog + default) and the `:agent` command
//! (which picks a backend and opens a pane). Pure data + the
//! [`Multiplexer`] abstraction; no `App` dependency, so it stays
//! unit-testable.

mod mux;

pub use mux::{Multiplexer, detect};

/// Placeholder substituted with the prompt text in [`AgentSpec::prompt_args`].
pub const PROMPT_PLACEHOLDER: &str = "{prompt}";

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
    /// Template argv for handing the agent an initial prompt at launch
    /// (e.g. `:agent explain @file`). Appended after [`Self::args`], with
    /// every [`PROMPT_PLACEHOLDER`] occurrence replaced by the prompt text.
    /// Most CLIs take the prompt as a positional arg (`["{prompt}"]`);
    /// aider wants `["--message", "{prompt}"]`. Empty → the agent takes no
    /// launch-time prompt, so a prompted `:agent` opens it bare.
    pub prompt_args: Vec<String>,
}

impl AgentSpec {
    /// The launch-time argv tail that passes `prompt` to the agent,
    /// substituting [`PROMPT_PLACEHOLDER`] in [`Self::prompt_args`]. Empty
    /// when the agent declares no `prompt_args`.
    pub fn prompt_argv(&self, prompt: &str) -> Vec<String> {
        self.prompt_args
            .iter()
            .map(|a| a.replace(PROMPT_PLACEHOLDER, prompt))
            .collect()
    }
}

/// Default `prompt_args` for an agent that doesn't specify its own: pass
/// the prompt as a single positional argument, which claude and codex
/// both accept ("starts an interactive session ... [prompt]").
pub fn default_prompt_args() -> Vec<String> {
    vec![PROMPT_PLACEHOLDER.to_string()]
}

/// Built-in agent catalog. Offered by the picker when the user has no
/// `[agents.*]` of their own; any entry can be overridden by a config
/// block of the same name. Commands assume the agent's CLI is on `PATH`.
pub fn builtin_agents() -> Vec<AgentSpec> {
    // claude/codex/gemini take the prompt positionally; aider's positional
    // args are files, so it gets the prompt via `--message`.
    [
        ("claude", &["{prompt}"][..]),
        ("codex", &["{prompt}"][..]),
        ("gemini", &["{prompt}"][..]),
        ("aider", &["--message", "{prompt}"][..]),
    ]
    .into_iter()
    .map(|(name, prompt_args)| AgentSpec {
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        prompt_args: prompt_args.iter().map(|s| s.to_string()).collect(),
    })
    .collect()
}
