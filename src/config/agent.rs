//! `[agent]` / `[agents.*]` configuration: which AI agents `:agent` can
//! launch and which one is the default.
//!
//! ```toml
//! [agent]
//! default = "claude"     # bare `:agent` launches this; unset → picker
//!
//! [agents.claude]        # override / add a catalog entry
//! command = "claude"
//! args = ["--model", "opus"]
//! ```
//!
//! The catalog is the built-in list ([`crate::agent::builtin_agents`])
//! overlaid with user `[agents.*]` blocks (same-name overrides), sorted
//! by name. `default` is carried through verbatim.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

use crate::agent::AgentSpec;

/// `[agent]` settings table.
#[derive(Debug, Default, Deserialize)]
pub struct AgentSettingsToml {
    /// Agent launched by a bare `:agent`. When unset, `:agent` opens a
    /// picker and writes the chosen name back here (first-time only).
    #[serde(default)]
    pub default: Option<String>,
}

/// One `[agents.<name>]` entry — the command line for that agent.
#[derive(Debug, Deserialize, Clone)]
pub struct AgentToml {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// How a `:agent <intent>` prompt is passed at launch (see
    /// [`AgentSpec::prompt_args`]). Omitted → the positional default, which
    /// suits claude/codex; an agent whose positional arg isn't a prompt
    /// (e.g. aider) sets this explicitly, and `[]` opts out of seeding.
    #[serde(default)]
    pub prompt_args: Option<Vec<String>>,
}

/// Resolved agent configuration: the merged catalog plus the configured
/// default name (if any).
pub struct AgentRegistry {
    /// Built-ins overlaid with `[agents.*]`, sorted by name.
    pub agents: Vec<AgentSpec>,
    /// `[agent].default`, if set.
    pub default: Option<String>,
}

impl AgentRegistry {
    /// Merge built-ins with the user's `[agents.*]` (same-name entries
    /// override) and fold in the `[agent]` settings.
    pub fn build(user: HashMap<String, AgentToml>, settings: AgentSettingsToml) -> Self {
        // BTreeMap keys give name-sorted output for free.
        let mut by_name: BTreeMap<String, AgentSpec> = crate::agent::builtin_agents()
            .into_iter()
            .map(|a| (a.name.clone(), a))
            .collect();
        for (name, t) in user {
            by_name.insert(
                name.clone(),
                AgentSpec {
                    name,
                    command: t.command,
                    args: t.args,
                    prompt_args: t
                        .prompt_args
                        .unwrap_or_else(crate::agent::default_prompt_args),
                },
            );
        }
        AgentRegistry {
            agents: by_name.into_values().collect(),
            default: settings.default,
        }
    }

    /// Find a resolved agent by name.
    pub fn find(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|a| a.name == name)
    }
}

/// Persist `name` as `[agent].default` in the active config file
/// (workspace-local `.vorto/config.toml` when present, else the global
/// `~/.config/vorto/config.toml`), creating the file and its parent
/// directory if absent. Existing content and comments are preserved —
/// see [`insert_default_agent`]. Returns the path written.
///
/// Intended for the first-time case (no default configured yet); the
/// caller gates on that.
pub fn persist_default_agent(name: &str) -> Result<PathBuf> {
    let path = super::default_path().ok_or_else(|| anyhow!("no config path (is $HOME set?)"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    // A missing file is the normal first-time case → start from empty.
    // Any other read error (permissions, etc.) is propagated rather than
    // silently treated as empty, which would overwrite the file body.
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let updated = insert_default_agent(&existing, name);
    std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Add `default = "<name>"` to the `[agent]` table of a config file's
/// text, preserving everything else verbatim. If an `[agent]` header
/// already exists the key is inserted right after it; otherwise a fresh
/// `[agent]` table is appended. The trailing-newline shape of the input
/// is kept.
fn insert_default_agent(existing: &str, name: &str) -> String {
    let line = format!("default = {}", toml_basic_string(name));
    if let Some(header_idx) = existing.lines().position(|l| l.trim() == "[agent]") {
        let mut out: Vec<String> = existing.lines().map(str::to_string).collect();
        out.insert(header_idx + 1, line);
        let mut s = out.join("\n");
        if existing.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    let mut s = existing.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    if !s.is_empty() {
        s.push('\n');
    }
    s.push_str("[agent]\n");
    s.push_str(&line);
    s.push('\n');
    s
}

/// Render `s` as a quoted TOML basic string, escaping the characters TOML
/// requires. Agent names normally come from bare table keys (so they're
/// already simple), but a quoted key like `[agents."odd\"name"]` could
/// carry `"`, `\`, or control chars — writing those unescaped would
/// produce a config file that fails to parse on the next load.
pub(super) fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_present_and_sorted() {
        let reg = AgentRegistry::build(HashMap::new(), AgentSettingsToml::default());
        let names: Vec<&str> = reg.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"claude"));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(reg.default.is_none());
    }

    #[test]
    fn user_entry_overrides_builtin_and_adds_new() {
        let mut user = HashMap::new();
        user.insert(
            "claude".to_string(),
            AgentToml {
                command: "claude".into(),
                args: vec!["--model".into(), "opus".into()],
                prompt_args: None,
            },
        );
        user.insert(
            "mybot".to_string(),
            AgentToml {
                command: "/usr/local/bin/mybot".into(),
                args: vec![],
                prompt_args: Some(vec!["-m".into(), "{prompt}".into()]),
            },
        );
        let reg = AgentRegistry::build(
            user,
            AgentSettingsToml {
                default: Some("claude".into()),
            },
        );
        let claude = reg.find("claude").unwrap();
        assert_eq!(claude.args, vec!["--model", "opus"]);
        // No `prompt_args` in the override → positional default.
        assert_eq!(claude.prompt_args, vec!["{prompt}"]);
        let mybot = reg.find("mybot").unwrap();
        assert_eq!(mybot.command, "/usr/local/bin/mybot");
        // Explicit `prompt_args` carried through verbatim.
        assert_eq!(mybot.prompt_args, vec!["-m", "{prompt}"]);
        assert_eq!(reg.default.as_deref(), Some("claude"));
    }

    #[test]
    fn insert_appends_fresh_table_to_empty() {
        assert_eq!(
            insert_default_agent("", "claude"),
            "[agent]\ndefault = \"claude\"\n"
        );
    }

    #[test]
    fn insert_appends_after_existing_content_preserving_it() {
        let existing = "[editor]\ntab_width = 4\n";
        let out = insert_default_agent(existing, "codex");
        assert_eq!(
            out,
            "[editor]\ntab_width = 4\n\n[agent]\ndefault = \"codex\"\n"
        );
    }

    #[test]
    fn insert_escapes_special_chars_in_name() {
        // A name with a quote/backslash must be written as a valid TOML
        // basic string, not break the file.
        let out = insert_default_agent("", "od\"d\\name");
        assert_eq!(out, "[agent]\ndefault = \"od\\\"d\\\\name\"\n");
        // And it must round-trip back through the TOML parser.
        let parsed: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(parsed["agent"]["default"].as_str(), Some("od\"d\\name"));
    }

    #[test]
    fn insert_into_existing_agent_table_keeps_comments() {
        let existing = "# my config\n[agent]\n# pick later\n[editor]\ntab_width = 2\n";
        let out = insert_default_agent(existing, "gemini");
        assert_eq!(
            out,
            "# my config\n[agent]\ndefault = \"gemini\"\n# pick later\n[editor]\ntab_width = 2\n"
        );
    }
}
