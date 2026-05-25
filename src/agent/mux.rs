//! Terminal-multiplexer backends for the `:agent` launcher.
//!
//! vorto doesn't embed a terminal emulator — to give an AI agent a live,
//! interactive pane we ask the multiplexer hosting our own process (tmux
//! or zellij) to open one. The backend differences (argv shape, how the
//! working dir and command are passed, how a pane is addressed) live
//! behind the [`Multiplexer`] trait so the `:agent` command code stays
//! backend-agnostic and a third backend is a new `impl` rather than a new
//! `match` arm.
//!
//! Reuse: [`Multiplexer::open_agent_pane`] hands back an opaque pane id;
//! the caller stashes it and, on the next `:agent`, calls
//! [`Multiplexer::focus_if_alive`] to jump to the existing pane instead
//! of opening a second one. Both backends expose addressable pane ids —
//! tmux via `-P -F '#{pane_id}'`, zellij via `list-panes --json` (the
//! freshly-opened pane is the focused one).

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use super::AgentSpec;

/// A terminal multiplexer that can host an agent pane alongside vorto.
pub trait Multiplexer {
    /// Backend name, for status messages (`"tmux"` / `"zellij"`).
    fn name(&self) -> &'static str;

    /// The argv that opens a new pane running `agent` in `cwd`. Split out
    /// from [`Self::open_agent_pane`] so command construction is
    /// unit-testable without spawning a process.
    fn open_argv(&self, agent: &AgentSpec, cwd: &Path) -> Vec<String>;

    /// Open a pane running `agent` (working dir `cwd`) and return an
    /// opaque id that [`Self::focus_if_alive`] can later refocus, or
    /// `None` when the backend couldn't determine the new pane's id (the
    /// caller then loses reuse but the pane still opened).
    fn open_agent_pane(&self, agent: &AgentSpec, cwd: &Path) -> Result<Option<String>>;

    /// If `id` still names a live pane, focus it and return `true`;
    /// return `false` when no such pane exists (the caller opens a fresh
    /// one). Never errors — a flaky query just means "open a new pane".
    fn focus_if_alive(&self, id: &str) -> bool;
}

/// tmux backend. Active when `$TMUX` is set (we're running inside a tmux
/// client). Uses `split-window` so the agent lands beside vorto.
pub struct Tmux;

/// zellij backend. Active when `$ZELLIJ` is set. Uses `zellij action
/// new-pane`; the new pane is focused on creation, so its id is read
/// back from `list-panes --json`.
pub struct Zellij;

impl Multiplexer for Tmux {
    fn name(&self) -> &'static str {
        "tmux"
    }

    fn open_argv(&self, agent: &AgentSpec, cwd: &Path) -> Vec<String> {
        // `-h` puts the new pane to the right; `-c` sets its start dir;
        // `-P -F '#{pane_id}'` prints the new pane id so we can refocus
        // it later. tmux's `split-window` takes the program as a single
        // shell-command argument, so join + quote the agent invocation.
        vec![
            "tmux".into(),
            "split-window".into(),
            "-h".into(),
            "-P".into(),
            "-F".into(),
            "#{pane_id}".into(),
            "-c".into(),
            cwd.to_string_lossy().into_owned(),
            shell_join(&agent.command, &agent.args),
        ]
    }

    fn open_agent_pane(&self, agent: &AgentSpec, cwd: &Path) -> Result<Option<String>> {
        let out = capture(&self.open_argv(agent, cwd), self.name())?;
        // `-P -F '#{pane_id}'` prints just the new pane id (e.g. `%7`).
        let id = out
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(id)
    }

    fn focus_if_alive(&self, id: &str) -> bool {
        // List the current session's pane ids; bail to "not alive" if the
        // query fails or the id is gone.
        let Ok(out) = capture(
            &argv(&["tmux", "list-panes", "-s", "-F", "#{pane_id}"]),
            "tmux",
        ) else {
            return false;
        };
        if !out.lines().any(|l| l.trim() == id) {
            return false;
        }
        // Switch to the pane's window first (it may not be the current
        // one), then focus the pane. Ignore errors — the alive check
        // already confirmed it exists.
        run_ok(&argv(&["tmux", "select-window", "-t", id]));
        run_ok(&argv(&["tmux", "select-pane", "-t", id]));
        true
    }
}

impl Multiplexer for Zellij {
    fn name(&self) -> &'static str {
        "zellij"
    }

    fn open_argv(&self, agent: &AgentSpec, cwd: &Path) -> Vec<String> {
        // zellij runs the command + args directly (no shell), so they go
        // as separate argv elements after `--`.
        let mut v = vec![
            "zellij".into(),
            "action".into(),
            "new-pane".into(),
            "--direction".into(),
            "right".into(),
            "--cwd".into(),
            cwd.to_string_lossy().into_owned(),
            "--".into(),
            agent.command.clone(),
        ];
        v.extend(agent.args.iter().cloned());
        v
    }

    fn open_agent_pane(&self, agent: &AgentSpec, cwd: &Path) -> Result<Option<String>> {
        run_checked(&self.open_argv(agent, cwd), self.name())?;
        // zellij focuses the freshly-opened pane, so the focused
        // non-plugin pane in the live list is the one we just created.
        let panes = list_zellij_panes().unwrap_or_default();
        let id = panes
            .iter()
            .find(|p| p.focused && !p.plugin)
            .map(|p| p.id.clone());
        Ok(id)
    }

    fn focus_if_alive(&self, id: &str) -> bool {
        let Some(panes) = list_zellij_panes() else {
            return false;
        };
        if !panes.iter().any(|p| p.id == id) {
            return false;
        }
        run_ok(&argv(&["zellij", "action", "focus-pane-id", id]));
        true
    }
}

/// Detect the multiplexer hosting the current process from the env vars
/// each one exports into its panes. Returns `None` outside both — the
/// caller surfaces an error rather than opening a standalone OS window,
/// since vorto has no window of its own to attach one to.
pub fn detect() -> Option<Box<dyn Multiplexer>> {
    if std::env::var_os("TMUX").is_some() {
        return Some(Box::new(Tmux));
    }
    if std::env::var_os("ZELLIJ").is_some() {
        return Some(Box::new(Zellij));
    }
    None
}

/// A pane as reported by `zellij action list-panes --json`, reduced to
/// the fields we need.
struct ZellijPane {
    id: String,
    focused: bool,
    plugin: bool,
}

/// Query zellij for its current panes. `None` on any spawn/parse failure
/// — callers treat that as "couldn't find the pane" rather than erroring.
fn list_zellij_panes() -> Option<Vec<ZellijPane>> {
    let out = capture(
        &argv(&["zellij", "action", "list-panes", "--json"]),
        "zellij",
    )
    .ok()?;
    let json: serde_json::Value = serde_json::from_str(&out).ok()?;
    let mut panes = Vec::new();
    collect_zellij_panes(&json, &mut panes);
    Some(panes)
}

/// Walk the (possibly tab-nested) list-panes JSON and collect every pane
/// object. A pane is any object carrying an `is_focused` key alongside a
/// numeric `id` — that distinguishes panes from the tab objects that
/// wrap them (which also carry an `id`).
fn collect_zellij_panes(v: &serde_json::Value, out: &mut Vec<ZellijPane>) {
    match v {
        serde_json::Value::Object(m) => {
            if m.contains_key("is_focused")
                && let Some(id) = m.get("id").and_then(|x| x.as_u64())
            {
                out.push(ZellijPane {
                    id: id.to_string(),
                    focused: m
                        .get("is_focused")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false),
                    plugin: m
                        .get("is_plugin")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false),
                });
            }
            for vv in m.values() {
                collect_zellij_panes(vv, out);
            }
        }
        serde_json::Value::Array(a) => {
            for vv in a {
                collect_zellij_panes(vv, out);
            }
        }
        _ => {}
    }
}

/// Build a `Vec<String>` argv from string slices.
fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Spawn `argv` and return its captured stdout, erroring on spawn failure
/// or a non-zero exit. `output()` captures stderr into a pipe (it never
/// reaches vorto's alternate screen), so the backend's own error message
/// is folded into the returned error rather than discarded.
fn capture(argv: &[String], backend: &str) -> Result<String> {
    let (bin, rest) = argv
        .split_first()
        .expect("open_argv never returns an empty argv");
    let out = Command::new(bin)
        .args(rest)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("spawning `{bin}` (is {backend} installed?)"))?;
    if !out.status.success() {
        return Err(exit_error(backend, out.status, &out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Spawn `argv` for effect, erroring on spawn failure or non-zero exit.
/// stdout is ignored; stderr is captured (not inherited) so a backend
/// error message reaches the toast instead of the terminal.
fn run_checked(argv: &[String], backend: &str) -> Result<()> {
    let (bin, rest) = argv
        .split_first()
        .expect("open_argv never returns an empty argv");
    let out = Command::new(bin)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .with_context(|| format!("spawning `{bin}` (is {backend} installed?)"))?;
    if !out.status.success() {
        return Err(exit_error(backend, out.status, &out.stderr));
    }
    Ok(())
}

/// Build an error for a non-zero backend exit, preferring the captured
/// stderr text (the multiplexer's own diagnostic) and falling back to the
/// exit status when stderr is empty.
fn exit_error(backend: &str, status: std::process::ExitStatus, stderr: &[u8]) -> anyhow::Error {
    let msg = String::from_utf8_lossy(stderr);
    let msg = msg.trim();
    if msg.is_empty() {
        anyhow!("{backend} exited with {status}")
    } else {
        anyhow!("{backend}: {msg}")
    }
}

/// Best-effort spawn for follow-up actions (focus / select) whose failure
/// shouldn't abort the flow — the alive check already gated them.
fn run_ok(argv: &[String]) {
    if let Some((bin, rest)) = argv.split_first() {
        let _ = Command::new(bin)
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Join a command and its args into one POSIX shell-command line,
/// single-quoting any token that isn't already shell-safe. Used for the
/// tmux backend, which passes the program as a single string.
fn shell_join(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote `s` for a POSIX shell. Leaves bare tokens made only of safe
/// characters untouched; otherwise wraps in single quotes, escaping any
/// embedded single quote with the `'\''` idiom.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec(command: &str, args: &[&str]) -> AgentSpec {
        AgentSpec {
            name: command.to_string(),
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn tmux_argv_prints_pane_id_and_passes_command_as_single_string() {
        let argv = Tmux.open_argv(&spec("claude", &[]), &PathBuf::from("/work/repo"));
        assert_eq!(
            argv,
            vec![
                "tmux",
                "split-window",
                "-h",
                "-P",
                "-F",
                "#{pane_id}",
                "-c",
                "/work/repo",
                "claude",
            ]
        );
    }

    #[test]
    fn tmux_argv_joins_and_quotes_args() {
        let argv = Tmux.open_argv(
            &spec("claude", &["--model", "opus 4"]),
            &PathBuf::from("/tmp"),
        );
        // The last element is the single shell-command string; the arg
        // with a space gets single-quoted.
        assert_eq!(argv.last().unwrap(), "claude --model 'opus 4'");
    }

    #[test]
    fn zellij_argv_passes_command_and_args_separately() {
        let argv = Zellij.open_argv(&spec("aider", &["--yes"]), &PathBuf::from("/work/repo"));
        assert_eq!(
            argv,
            vec![
                "zellij",
                "action",
                "new-pane",
                "--direction",
                "right",
                "--cwd",
                "/work/repo",
                "--",
                "aider",
                "--yes",
            ]
        );
    }

    #[test]
    fn collect_zellij_panes_finds_panes_in_nested_tabs() {
        // Shape: tabs (each with an `id`) wrapping a `panes` array. Only
        // the pane objects (those with `is_focused`) should be collected.
        let json = serde_json::json!({
            "tabs": [
                {
                    "id": 0,
                    "panes": [
                        {"id": 1, "is_focused": false, "is_plugin": true, "title": "status"},
                        {"id": 2, "is_focused": true, "is_plugin": false, "title": "claude"}
                    ]
                }
            ]
        });
        let mut panes = Vec::new();
        collect_zellij_panes(&json, &mut panes);
        assert_eq!(panes.len(), 2);
        let focused = panes.iter().find(|p| p.focused && !p.plugin).unwrap();
        assert_eq!(focused.id, "2");
    }

    #[test]
    fn collect_zellij_panes_handles_flat_array() {
        let json = serde_json::json!([
            {"id": 5, "is_focused": true, "is_plugin": false}
        ]);
        let mut panes = Vec::new();
        collect_zellij_panes(&json, &mut panes);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].id, "5");
        assert!(panes[0].focused);
    }

    #[test]
    fn shell_quote_leaves_safe_tokens_bare() {
        assert_eq!(shell_quote("claude"), "claude");
        assert_eq!(shell_quote("--model=opus"), "--model=opus");
        assert_eq!(shell_quote("./bin/agent"), "./bin/agent");
    }

    #[test]
    fn shell_quote_wraps_unsafe_tokens() {
        assert_eq!(shell_quote("opus 4"), "'opus 4'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
    }
}
