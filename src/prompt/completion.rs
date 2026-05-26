//! Tab-completion machinery for the `:` command line — command-name
//! cycling and filesystem-path completion.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Args, COMMANDS, Command};

use super::line_input::LineInput;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// Cycling command names at the head of the input.
    CommandName,
    /// Cycling filesystem paths supplied as the command argument.
    Path,
}

/// In-flight Tab completion for the `:` command line. `prefix` is the
/// partial substring being completed at the time Tab was first
/// pressed — kept so successive Tabs cycle the same candidate set
/// even after the visible input has been replaced with a candidate.
/// `head_chars` is the number of chars from the start of the input
/// that sit *before* the completion target (0 for command-name
/// completion, `"<cmd> ".chars().count()` for path completion).
pub struct CompletionState {
    pub kind: CompletionKind,
    pub prefix: String,
    pub head_chars: usize,
    pub matches: Vec<String>,
    pub selected: usize,
}

pub struct CommandPrompt {
    pub input: LineInput,
    /// Set while the user is Tab-cycling. Cleared as soon as any key
    /// that isn't Tab / Shift-Tab arrives, so editing reverts to the
    /// normal "typed text" flow.
    pub completion: Option<CompletionState>,
}

impl CommandPrompt {
    pub(super) fn new() -> Self {
        Self {
            input: LineInput::new(),
            completion: None,
        }
    }

    /// Build (or refresh) the completion list against the current input
    /// and step the selection by `step` (+1 for Tab, -1 for Shift-Tab).
    /// The visible input is replaced with the chosen candidate so the
    /// user can immediately submit it or keep typing past it.
    ///
    /// When the prefix uniquely resolves to a single top-level command
    /// that takes a second stage (a subcommand set or a path), a separating
    /// space is appended and the cycle ends, so the very next Tab — and
    /// the live hint panel — descend straight into that stage instead of
    /// requiring the user to type the space by hand.
    ///
    /// `root` anchors relative path completions, mirroring `:e`'s
    /// resolution against `startup_cwd`.
    pub(super) fn tab(&mut self, step: i32, root: &Path) {
        if self.completion.is_none() {
            let Some(state) = build_completion(self.input.as_str(), root) else {
                return;
            };
            self.completion = Some(state);
        } else if let Some(c) = self.completion.as_mut() {
            let len = c.matches.len() as i32;
            let next = (c.selected as i32 + step).rem_euclid(len);
            c.selected = next as usize;
        }
        let Some(c) = &self.completion else {
            return;
        };
        let head: String = self.input.as_str().chars().take(c.head_chars).collect();
        let new = format!("{}{}", head, c.matches[c.selected]);
        // A top-level command name that uniquely resolved and carries a
        // second stage: append a space and drop the cycle so the live
        // hint panel re-derives against the new head + space and shows
        // the next stage, and the next Tab completes within it.
        let descend = c.kind == CompletionKind::CommandName
            && c.head_chars == 0
            && c.matches.len() == 1
            && Command::find(&c.matches[0]).is_some_and(|cmd| !matches!(cmd.args, Args::None));
        self.input = LineInput::new();
        for ch in new.chars() {
            self.input.insert(ch);
        }
        if descend {
            self.input.insert(' ');
            self.completion = None;
        }
    }
}

/// Decide what to complete based on the current `:` input. Returns
/// `None` when nothing useful can be offered (no command match, or
/// the command doesn't take a path).
fn build_completion(input: &str, root: &Path) -> Option<CompletionState> {
    match input.find(' ') {
        None => {
            // Command-name completion: prefix is the whole input,
            // candidates are every typeable name in the command table.
            let prefix = input.to_string();
            let matches: Vec<String> = COMMANDS
                .iter()
                .flat_map(|c| c.all_names())
                .filter(|n| n.starts_with(&prefix))
                .map(|n| n.to_string())
                .collect();
            if matches.is_empty() {
                return None;
            }
            Some(CompletionState {
                kind: CompletionKind::CommandName,
                prefix,
                head_chars: 0,
                matches,
                selected: 0,
            })
        }
        Some(sp_byte) => {
            // Argument completion, driven by the command's `Args`: a path
            // for `:e`/`:w`, a subcommand name for `:copilot`/`:grammar`,
            // nothing otherwise. Bail past a second space — the user is
            // beyond the single argument we complete.
            let cmd = &input[..sp_byte];
            let c = Command::find(cmd)?;
            let partial = &input[sp_byte + 1..];
            if partial.contains(' ') {
                return None;
            }
            match &c.args {
                Args::None => None,
                Args::Sub(subs) => {
                    let matches: Vec<String> = subs
                        .iter()
                        .flat_map(|s| std::iter::once(s.name).chain(s.aliases.iter().copied()))
                        .filter(|n| n.starts_with(partial))
                        .map(|n| n.to_string())
                        .collect();
                    if matches.is_empty() {
                        return None;
                    }
                    Some(CompletionState {
                        kind: CompletionKind::CommandName,
                        prefix: partial.to_string(),
                        head_chars: sp_byte + 1,
                        matches,
                        selected: 0,
                    })
                }
                Args::Path => {
                    let matches = path_candidates(partial, root);
                    if matches.is_empty() {
                        return None;
                    }
                    // head = cmd + " " in chars. Command heads are ASCII,
                    // so byte and char counts agree.
                    let head_chars = cmd.chars().count() + 1;
                    Some(CompletionState {
                        kind: CompletionKind::Path,
                        prefix: partial.to_string(),
                        head_chars,
                        matches,
                        selected: 0,
                    })
                }
            }
        }
    }
}

/// List the filesystem entries that match `partial`, anchored at
/// `root` for relative inputs. The returned strings are full
/// replacements for `partial`: they preserve any directory portion
/// the user already typed and append `/` to directory entries so
/// further Tabs descend naturally.
fn path_candidates(partial: &str, root: &Path) -> Vec<String> {
    // Split into "directory prefix the user already typed" + "basename
    // prefix we're filtering on". For "src/m" → ("src/", "m"); for
    // "main" → ("", "main"); for "" → ("", "").
    let (dir_str, base_prefix) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };
    let listing_dir: PathBuf = if dir_str.is_empty() {
        root.to_path_buf()
    } else {
        let p = Path::new(dir_str);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    let Ok(rd) = fs::read_dir(&listing_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Hidden files only show when the user explicitly types `.`.
        if name.starts_with('.') && !base_prefix.starts_with('.') {
            continue;
        }
        if !name.starts_with(base_prefix) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut s = String::with_capacity(dir_str.len() + name.len() + 1);
        s.push_str(dir_str);
        s.push_str(name);
        if is_dir {
            s.push('/');
        }
        out.push(s);
    }
    // Stable, predictable order: directories first, then files,
    // alphabetical within each group.
    out.sort_by(|a, b| {
        let a_dir = a.ends_with('/');
        let b_dir = b.ends_with('/');
        b_dir.cmp(&a_dir).then_with(|| a.cmp(b))
    });
    out.truncate(200);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> CommandPrompt {
        let mut cp = CommandPrompt::new();
        for ch in s.chars() {
            cp.input.insert(ch);
        }
        cp
    }

    // A command-name completion that resolves uniquely to a command with a
    // second stage gets a trailing space and ends the cycle, so the next
    // Tab descends into the subcommand stage.
    #[test]
    fn unique_subcommand_command_descends_with_space() {
        let mut cp = typed("cop");
        cp.tab(1, Path::new(""));
        assert_eq!(cp.input.as_str(), "copilot ");
        assert!(cp.completion.is_none());

        // Next Tab completes within the subcommand stage.
        cp.tab(1, Path::new(""));
        assert_eq!(cp.input.as_str(), "copilot status");
    }

    // Path commands count as a second stage too.
    #[test]
    fn unique_path_command_descends_with_space() {
        let mut cp = typed("edit");
        cp.tab(1, Path::new(""));
        assert_eq!(cp.input.as_str(), "edit ");
        assert!(cp.completion.is_none());
    }

    // An ambiguous prefix keeps cycling without inserting a space, even
    // when the first candidate happens to take arguments — the user is
    // still choosing which command.
    #[test]
    fn ambiguous_prefix_cycles_without_space() {
        let mut cp = typed("e");
        cp.tab(1, Path::new(""));
        // "e" and its alias "edit" both match → no descend.
        assert_eq!(cp.input.as_str(), "e");
        assert!(cp.completion.is_some());
        cp.tab(1, Path::new(""));
        assert_eq!(cp.input.as_str(), "edit");
    }

    // A command that uniquely resolves but has no second stage gets no
    // trailing space — `log` is the only name starting with "log" and is
    // `Args::None`, so this exercises the `matches.len() == 1` + `Args::None`
    // path (unlike an ambiguous prefix, which is rejected earlier).
    #[test]
    fn argless_command_gets_no_space() {
        let mut cp = typed("log");
        cp.tab(1, Path::new(""));
        assert_eq!(cp.input.as_str(), "log");
        assert!(cp.completion.is_some());
    }
}
