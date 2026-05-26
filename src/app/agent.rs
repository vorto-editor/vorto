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
//! Bare `:agent` just launches. `:agent <intent> @target` (e.g.
//! `:agent explain @file`, `:agent explain @selection`) additionally
//! builds a prompt from the active buffer and forwards it: a fresh pane
//! launches seeded with it ([`AgentSpec::prompt_argv`]), an already-open
//! pane gets it pasted in via [`Multiplexer::send_prompt`]. `@file`
//! resolves to the active buffer's path (an unsaved/scratch buffer is
//! snapshotted to a temp file the agent can read); `@selection` embeds the
//! visual selection — captured when `:` opened the prompt — inline as a
//! code block. The reuse path uses bracketed paste, so a multi-line
//! selection block pastes whole instead of submitting line by line.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::{self, AgentSpec, Multiplexer};
use crate::config::{AGENT_SUBCOMMANDS, resolve_subcommand};

use super::{App, Toast, root_cause};

impl App {
    /// `:agent [<intent> [@target]]`. Bare `:agent` launches the default
    /// agent (or opens the picker). With an intent (`explain` / `chat`) it
    /// builds a prompt from the active buffer and forwards it to the
    /// agent.
    pub(super) fn run_agent_command(&mut self, rest: &str) {
        let rest = rest.trim();
        let prompt = if rest.is_empty() {
            None
        } else {
            match self.build_agent_prompt(rest) {
                Ok(p) => Some(p),
                Err(msg) => {
                    self.push_toast(Toast::error(msg));
                    return;
                }
            }
        };
        let Some(mux) = agent::detect() else {
            self.push_toast(Toast::error(
                "no terminal multiplexer detected — run vorto inside tmux or zellij",
            ));
            return;
        };
        match self.config.agents.default.clone() {
            Some(name) => self.launch_agent(&name, mux.as_ref(), prompt),
            None => {
                // No default yet: stage the prompt so the picker's choice
                // launches seeded, then ask which agent to use.
                self.agent_pending_prompt = prompt;
                self.open_agent_picker();
            }
        }
    }

    /// Parse `<intent> [@target]` and render it into a prompt against the
    /// active buffer. `Err` carries a ready-to-toast message.
    fn build_agent_prompt(&self, rest: &str) -> Result<String, String> {
        let (intent_tok, target) = match rest.split_once(char::is_whitespace) {
            Some((i, t)) => (i, t.trim()),
            None => (rest, ""),
        };
        let intent = resolve_subcommand(AGENT_SUBCOMMANDS, intent_tok)
            .ok_or_else(|| format!("unknown agent subcommand: {intent_tok}"))?;
        let ctx = self.resolve_agent_target(target)?;
        Ok(agent_prompt(intent, &ctx))
    }

    /// Resolve the `@target` token to a path the agent can read. An empty
    /// target defaults to `@file`. Anything unrecognised is rejected with a
    /// hint rather than silently launching against the wrong thing.
    fn resolve_agent_target(&self, target: &str) -> Result<AgentContext, String> {
        match target {
            "" | "@file" | "@buffer" => self.agent_file_target(),
            "@selection" | "@sel" => self.agent_selection_target(),
            other => Err(format!(
                "unknown agent target: {other} (try @file or @selection)"
            )),
        }
    }

    /// Resolve `@file` to a path the agent can open. A clean file-backed
    /// buffer yields its own path; an unsaved or scratch buffer is
    /// snapshotted to a temp file first (the agent runs in a separate
    /// process and can't see vorto's in-memory edits).
    fn agent_file_target(&self) -> Result<AgentContext, String> {
        if let Some(p) = &self.active_doc().path
            && !self.active_doc().dirty
        {
            return Ok(AgentContext::File(self.display_agent_path(p)));
        }
        let name = self
            .active_doc()
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "scratch".to_string());
        let p = self
            .write_agent_temp(&name, &self.active_doc().lines.join("\n"))
            .map_err(|e| format!("couldn't stage buffer for the agent: {e}"))?;
        Ok(AgentContext::File(self.display_agent_path(&p)))
    }

    /// Resolve `@selection` to the text captured when `:` opened the
    /// prompt, embedded inline in the prompt as a code block. The reuse
    /// path delivers it via bracketed paste, so the embedded newlines are
    /// fine.
    fn agent_selection_target(&self) -> Result<AgentContext, String> {
        let code = self.command_selection.clone().ok_or_else(|| {
            "no selection — select text in visual mode, then run :agent".to_string()
        })?;
        let origin = self
            .active_doc()
            .path
            .as_ref()
            .map(|p| self.display_agent_path(p));
        Ok(AgentContext::Selection { code, origin })
    }

    /// Write `contents` to a uniquely-named temp file ending in `name` (so
    /// its extension drives the agent's syntax detection) under a `vorto-`
    /// prefix. Left on disk for the agent to read; cleaned up by the OS
    /// temp reaper.
    fn write_agent_temp(&self, name: &str, contents: &str) -> std::io::Result<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("vorto-{}-{stamp}-{name}", std::process::id()));
        // Unsaved buffer contents can be sensitive — keep the temp file
        // private to the user rather than relying on the umask (a default
        // 0o666 would be world-readable in a shared /tmp).
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(contents.as_bytes())?;
        }
        #[cfg(not(unix))]
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    /// Render `p` for the prompt: relative to the agent's working dir
    /// (`startup_cwd`) when it lives under it, else the full path. Temp
    /// snapshots fall through to absolute, which is what the agent needs.
    fn display_agent_path(&self, p: &Path) -> String {
        p.strip_prefix(&self.startup_cwd)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    }

    /// Resolve `name` against the catalog, then either reuse the existing
    /// agent pane (focusing it, and typing in `prompt` when present) or
    /// open a new one. Toasts on an unknown name or a backend failure.
    fn launch_agent(&mut self, name: &str, mux: &dyn Multiplexer, prompt: Option<String>) {
        let Some(spec) = self.config.agents.find(name).cloned() else {
            self.push_toast(Toast::error(format!("unknown agent: {name}")));
            return;
        };
        // Reuse: if we opened a pane earlier this session and it's still
        // alive, jump to it instead of spawning a second one. A prompt
        // gets typed into the running session.
        if let Some(id) = self.agent_pane.clone() {
            if mux.focus_if_alive(&id) {
                match &prompt {
                    Some(p) if mux.send_prompt(&id, p) => {
                        self.push_toast(Toast::info(format!("sent prompt to {} pane", spec.name)))
                    }
                    Some(_) => self.push_toast(Toast::error(format!(
                        "couldn't send prompt to {} pane",
                        spec.name
                    ))),
                    None => self.push_toast(Toast::info(format!("focused {} pane", spec.name))),
                }
                return;
            }
            // Stale — the pane was closed. Drop it and open afresh below.
            self.agent_pane = None;
        }
        self.spawn_agent_pane(&spec, mux, prompt.as_deref());
    }

    fn spawn_agent_pane(&mut self, spec: &AgentSpec, mux: &dyn Multiplexer, prompt: Option<&str>) {
        let seeded = prompt.is_some();
        match mux.open_agent_pane(spec, &self.startup_cwd, prompt) {
            Ok(pane_id) => {
                self.agent_pane = pane_id;
                let how = if seeded { " with prompt" } else { "" };
                self.push_toast(Toast::info(format!(
                    "launched {} in {} pane{how}",
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
    /// then launches it — forwarding any prompt staged by the `:agent
    /// <intent>` that triggered the picker.
    pub(super) fn select_agent(&mut self, name: String) {
        let prompt = self.agent_pending_prompt.take();
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
        self.launch_agent(&name, mux.as_ref(), prompt);
    }
}

/// What a resolved `@target` points the agent at. `@file` is handed over
/// as a path (the agent reads it); `@selection` is embedded inline as a
/// code block so the agent sees it immediately without a temp file.
enum AgentContext {
    /// The active file (or a temp snapshot of an unsaved buffer) by path.
    File(String),
    /// The visual selection, with the file it came from when known.
    Selection {
        code: String,
        origin: Option<String>,
    },
}

/// Render the prompt for an `:agent <intent> @target`. The intent is
/// already resolved to its canonical name by the caller.
fn agent_prompt(intent: &str, ctx: &AgentContext) -> String {
    match (intent, ctx) {
        ("explain", AgentContext::File(p)) => {
            format!("Explain the code in `{p}` — what it does and how it works.")
        }
        ("explain", AgentContext::Selection { code, origin }) => format!(
            "Explain this selection{}, what it does and how it works:\n\n{}",
            origin_suffix(origin),
            fence(code)
        ),
        ("chat", AgentContext::File(p)) => {
            format!("Let's talk about `{p}`. Read it first, then wait for my questions.")
        }
        ("chat", AgentContext::Selection { code, origin }) => format!(
            "Let's talk about this selection{}. Read it, then wait for my questions:\n\n{}",
            origin_suffix(origin),
            fence(code)
        ),
        (other, _) => unreachable!("agent intent `{other}` has no prompt template"),
    }
}

/// ` from \`path\`` when the selection's origin file is known, else empty.
fn origin_suffix(origin: &Option<String>) -> String {
    origin
        .as_ref()
        .map(|o| format!(" from `{o}`"))
        .unwrap_or_default()
}

/// Wrap `code` in a fenced block. Language detection is left to the agent
/// (the origin path in the prompt is the hint).
fn fence(code: &str) -> String {
    format!("```\n{code}\n```")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_prompt_renders_each_intent_with_the_path() {
        let explain = agent_prompt("explain", &AgentContext::File("src/foo.rs".into()));
        assert!(explain.contains("src/foo.rs"));
        assert!(explain.to_lowercase().contains("explain"));

        let chat = agent_prompt("chat", &AgentContext::File("src/foo.rs".into()));
        assert!(chat.contains("src/foo.rs"));
    }

    #[test]
    fn agent_prompt_embeds_selection_as_a_code_block_with_origin() {
        let p = agent_prompt(
            "explain",
            &AgentContext::Selection {
                code: "let x = 1;".into(),
                origin: Some("src/foo.rs".into()),
            },
        );
        assert!(p.contains("let x = 1;"));
        assert!(p.contains("```"));
        assert!(p.contains("src/foo.rs"));
    }

    #[test]
    fn agent_prompt_selection_without_origin_omits_the_from_clause() {
        let p = agent_prompt(
            "chat",
            &AgentContext::Selection {
                code: "x".into(),
                origin: None,
            },
        );
        assert!(!p.contains(" from `"));
        assert!(p.contains("```"));
    }
}
