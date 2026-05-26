//! `:` command table — the single source of truth for every `:`-prompt
//! command: its name/aliases, description, what it completes after the
//! head (nothing / a path / subcommands), and how it dispatches.
//!
//! Three layers read this table and none of them duplicate the list:
//! - [`crate::app`] evaluates a command by matching on [`Command::kind`];
//! - [`crate::prompt`] builds Tab-completion candidates from the names
//!   and [`Args`];
//! - [`crate::ui`] builds the `:`-hint / which-key panel from the same.
//!
//! Adding a command is one [`COMMANDS`] entry (plus, for an
//! [`Inline`] command, one arm in the evaluator's dispatch `match`, which
//! the compiler enforces). Kept out of `app` so the lower layers can read
//! it without depending on the editor state machine — hence [`Inline`] is
//! a plain enum rather than a `fn(&mut App)`.

use crate::action::DirectKind;

/// One `:` command.
pub struct Command {
    /// Canonical (primary) name — the documentation handle and what the
    /// hint panel shows.
    pub name: &'static str,
    /// Extra spellings that resolve to the same command.
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    /// What follows the head: nothing, a path, or a fixed subcommand set.
    /// Drives both Tab-completion and the hint panel.
    pub args: Args,
    /// How the evaluator runs it.
    pub kind: Kind,
}

/// Shape of a command's argument.
pub enum Args {
    /// No argument completion.
    None,
    /// Completes a filesystem path (`:e `, `:w `).
    Path,
    /// Completes one of a fixed subcommand set (`:grammar `, `:copilot `).
    Sub(&'static [Subcommand]),
}

/// A subcommand of an [`Args::Sub`] command. Metadata only — the actual
/// dispatch lives in the owning command's `App` handler, which resolves a
/// token with [`resolve_subcommand`].
pub struct Subcommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
}

/// How [`crate::app::App::execute_command`] runs a command.
pub enum Kind {
    /// Maps to a single [`DirectKind`], dispatched via `Expr::Direct`.
    Direct(DirectKind),
    /// Routed to a dedicated `App` handler — these open modals, spawn
    /// processes, or take subcommands that don't fit the single-`DirectKind`
    /// shape. The [`Inline`] variant names which handler.
    Inline(Inline),
}

/// Inline-dispatched commands. A plain enum (no `fn(&mut App)`) so this
/// table stays free of any `app` dependency; the evaluator matches on it,
/// and adding a variant forces the dispatch site to handle it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    Copilot,
    Grammar,
    Agent,
    Theme,
}

/// `:copilot` subcommands. Source of truth for completion + the hint
/// panel; `App::run_copilot_command` resolves a token against this list.
pub const COPILOT_SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "status",
        aliases: &[],
        description: "show Copilot auth state",
    },
    Subcommand {
        name: "signin",
        aliases: &["login"],
        description: "start device-flow sign-in",
    },
    Subcommand {
        name: "signout",
        aliases: &["logout"],
        description: "sign out of Copilot",
    },
    Subcommand {
        name: "code",
        aliases: &[],
        description: "re-show signin modal + re-copy code",
    },
];

/// `:agent` subcommands — intents that build a prompt from editor context
/// and forward it to the agent. Bare `:agent` (no subcommand) still just
/// launches the default agent. `App::run_agent_command` resolves a token
/// against this list.
pub const AGENT_SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "explain",
        aliases: &[],
        description: "explain @file / @selection with the agent",
    },
    Subcommand {
        name: "chat",
        aliases: &[],
        description: "discuss @file / @selection with the agent",
    },
];

/// `:grammar` subcommands. Mirrors the `vorto grammar` CLI so muscle
/// memory carries over.
pub const GRAMMAR_SUBCOMMANDS: &[Subcommand] = &[
    Subcommand {
        name: "list",
        aliases: &["ls"],
        description: "browse, install & remove (modal)",
    },
    Subcommand {
        name: "install",
        aliases: &["add"],
        description: "install <name>... | --all",
    },
    Subcommand {
        name: "remove",
        aliases: &["rm", "uninstall"],
        description: "remove <name>...",
    },
];

impl Command {
    /// Resolve a command head (canonical name or alias) to its entry.
    pub fn find(name: &str) -> Option<&'static Command> {
        COMMANDS
            .iter()
            .find(|c| c.name == name || c.aliases.contains(&name))
    }

    /// The primary name followed by each alias — every typeable form.
    pub fn all_names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }
}

/// Resolve a subcommand token (canonical name or alias) to its canonical
/// name. Used by inline handlers to dispatch.
pub fn resolve_subcommand(subs: &'static [Subcommand], token: &str) -> Option<&'static str> {
    subs.iter()
        .find(|s| s.name == token || s.aliases.contains(&token))
        .map(|s| s.name)
}

/// Every `:` command. See the module docs for the contract.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "q",
        aliases: &["quit"],
        description: "quit",
        args: Args::None,
        kind: Kind::Direct(DirectKind::Quit),
    },
    Command {
        name: "q!",
        aliases: &["quit!"],
        description: "force quit",
        args: Args::None,
        kind: Kind::Direct(DirectKind::QuitForce),
    },
    Command {
        name: "w",
        aliases: &["write"],
        description: "save (or :w <path>)",
        args: Args::Path,
        kind: Kind::Direct(DirectKind::Save),
    },
    Command {
        name: "w!",
        aliases: &["write!"],
        description: "save, creating dirs",
        args: Args::Path,
        kind: Kind::Direct(DirectKind::SaveForce),
    },
    Command {
        name: "wq",
        aliases: &["x"],
        description: "save & quit",
        args: Args::Path,
        kind: Kind::Direct(DirectKind::SaveAndQuit),
    },
    Command {
        name: "e",
        aliases: &["edit"],
        description: "open <path>",
        args: Args::Path,
        kind: Kind::Direct(DirectKind::Open),
    },
    Command {
        name: "bn",
        aliases: &["bnext"],
        description: "next buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferNext),
    },
    Command {
        name: "bp",
        aliases: &["bprev"],
        description: "previous buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferPrev),
    },
    Command {
        name: "bd",
        aliases: &["bdelete"],
        description: "delete buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferDelete),
    },
    Command {
        name: "bd!",
        aliases: &["bdelete!", "bc", "bc!"],
        description: "force delete buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferDeleteForce),
    },
    Command {
        name: "bca",
        aliases: &["bca!"],
        description: "force delete all buffers",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferDeleteAll),
    },
    Command {
        name: "bls",
        aliases: &["buffers"],
        description: "buffer picker",
        args: Args::None,
        kind: Kind::Direct(DirectKind::BufferList),
    },
    Command {
        name: "new",
        aliases: &["enew"],
        description: "new scratch buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::NewScratchBuffer),
    },
    Command {
        name: "goto",
        aliases: &[],
        description: "go to line <n>",
        args: Args::None,
        kind: Kind::Direct(DirectKind::GotoLine),
    },
    Command {
        name: "log",
        aliases: &[],
        description: "open debug log file",
        args: Args::None,
        kind: Kind::Direct(DirectKind::OpenLog),
    },
    Command {
        name: "lsp",
        aliases: &[],
        description: "show LSP for current buffer (:lsp all for every language)",
        args: Args::None,
        kind: Kind::Direct(DirectKind::LspStatus),
    },
    Command {
        name: "reload",
        aliases: &["e!"],
        description: "reload buffer from disk (undo restores)",
        args: Args::None,
        kind: Kind::Direct(DirectKind::Reload),
    },
    Command {
        name: "reload-all",
        aliases: &[],
        description: "reload every file-backed buffer",
        args: Args::None,
        kind: Kind::Direct(DirectKind::ReloadAll),
    },
    Command {
        name: "noh",
        aliases: &["nohl", "nohlsearch"],
        description: "clear search highlight",
        args: Args::None,
        kind: Kind::Direct(DirectKind::ClearSearch),
    },
    Command {
        name: "split",
        aliases: &["sp", "nh"],
        description: "split pane below",
        args: Args::None,
        kind: Kind::Direct(DirectKind::SplitWindowHorizontal),
    },
    Command {
        name: "vsplit",
        aliases: &["vsp", "vs", "nv"],
        description: "split pane right",
        args: Args::None,
        kind: Kind::Direct(DirectKind::SplitWindowVertical),
    },
    Command {
        name: "close",
        aliases: &["clo"],
        description: "close active pane",
        args: Args::None,
        kind: Kind::Direct(DirectKind::CloseWindow),
    },
    Command {
        name: "only",
        aliases: &["on"],
        description: "(future) close all but active pane",
        args: Args::None,
        kind: Kind::Direct(DirectKind::CloseWindow),
    },
    Command {
        name: "copilot",
        aliases: &[],
        description: "Copilot status / signin / signout",
        args: Args::Sub(COPILOT_SUBCOMMANDS),
        kind: Kind::Inline(Inline::Copilot),
    },
    Command {
        name: "grammar",
        aliases: &[],
        description: "install / remove tree-sitter grammars",
        args: Args::Sub(GRAMMAR_SUBCOMMANDS),
        kind: Kind::Inline(Inline::Grammar),
    },
    Command {
        name: "agent",
        aliases: &[],
        description: "launch an AI agent (or :agent explain/chat @file)",
        args: Args::Sub(AGENT_SUBCOMMANDS),
        kind: Kind::Inline(Inline::Agent),
    },
    Command {
        name: "theme",
        aliases: &["colorscheme", "colo"],
        description: "pick a color theme (live preview)",
        args: Args::None,
        kind: Kind::Inline(Inline::Theme),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_resolves_name_and_alias() {
        assert!(matches!(
            Command::find("w").map(|c| &c.kind),
            Some(Kind::Direct(DirectKind::Save))
        ));
        // alias
        assert!(matches!(
            Command::find("write").map(|c| &c.kind),
            Some(Kind::Direct(DirectKind::Save))
        ));
        assert!(Command::find("nope").is_none());
    }

    #[test]
    fn inline_commands_are_in_the_table() {
        for (name, inline) in [
            ("copilot", Inline::Copilot),
            ("grammar", Inline::Grammar),
            ("agent", Inline::Agent),
        ] {
            match Command::find(name).map(|c| &c.kind) {
                Some(Kind::Inline(i)) => assert_eq!(*i, inline),
                other => panic!("{name} should be Inline, got {:?}", other.is_some()),
            }
        }
    }

    #[test]
    fn path_commands_take_a_path() {
        assert!(matches!(
            Command::find("e").map(|c| &c.args),
            Some(Args::Path)
        ));
        assert!(matches!(
            Command::find("q").map(|c| &c.args),
            Some(Args::None)
        ));
    }

    #[test]
    fn resolve_subcommand_handles_aliases() {
        assert_eq!(
            resolve_subcommand(COPILOT_SUBCOMMANDS, "login"),
            Some("signin")
        );
        assert_eq!(resolve_subcommand(GRAMMAR_SUBCOMMANDS, "ls"), Some("list"));
        assert_eq!(resolve_subcommand(GRAMMAR_SUBCOMMANDS, "nope"), None);
    }
}
