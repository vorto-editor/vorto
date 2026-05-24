//! Tab-completion machinery for the `:` command line — command-name
//! cycling and filesystem-path completion.

use std::fs;
use std::path::{Path, PathBuf};

use crate::app::COPILOT_SUBCOMMANDS;
use crate::config::COMMAND_BINDS;

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
        if let Some(c) = &self.completion {
            let head: String = self.input.as_str().chars().take(c.head_chars).collect();
            let new = format!("{}{}", head, c.matches[c.selected]);
            self.input = LineInput::new();
            for ch in new.chars() {
                self.input.insert(ch);
            }
        }
    }
}

/// Decide what to complete based on the current `:` input. Returns
/// `None` when nothing useful can be offered (no command match, or
/// the command doesn't take a path).
fn build_completion(input: &str, root: &Path) -> Option<CompletionState> {
    match input.find(' ') {
        None => {
            // Command-name completion: head is empty, prefix is the
            // whole input, candidates are every typeable name that
            // starts with it.
            let prefix = input.to_string();
            let matches: Vec<String> = COMMAND_BINDS
                .iter()
                .flat_map(|b| b.all_names())
                .chain(std::iter::once("copilot"))
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
            // Path completion (only after a path-taking command).
            // Preserve everything up to and including the first space,
            // and complete the partial path after it.
            let cmd = &input[..sp_byte];
            // `copilot` lives outside the COMMAND_BINDS table (the
            // `cmd == "copilot"` intercept in `eval` routes it to a
            // dedicated handler) but its subcommands should still cycle
            // via Tab — synthesize a CommandName completion against
            // the known subcommand set.
            if cmd == "copilot" {
                let partial = &input[sp_byte + 1..];
                if partial.contains(' ') {
                    return None;
                }
                let matches: Vec<String> = COPILOT_SUBCOMMANDS
                    .iter()
                    .flat_map(|s| std::iter::once(s.name).chain(s.aliases.iter().copied()))
                    .filter(|n| n.starts_with(partial))
                    .map(|n| n.to_string())
                    .collect();
                if matches.is_empty() {
                    return None;
                }
                return Some(CompletionState {
                    kind: CompletionKind::CommandName,
                    prefix: partial.to_string(),
                    head_chars: sp_byte + 1,
                    matches,
                    selected: 0,
                });
            }
            let bind = COMMAND_BINDS
                .iter()
                .find(|b| b.name == cmd || b.aliases.contains(&cmd))?;
            if !bind.takes_path {
                return None;
            }
            let partial = &input[sp_byte + 1..];
            // Disallow more args: if the user typed another space,
            // they're past the path — bail rather than complete the
            // wrong thing.
            if partial.contains(' ') {
                return None;
            }
            let matches = path_candidates(partial, root);
            if matches.is_empty() {
                return None;
            }
            // head = cmd + " " in chars. cmd is ASCII (commands are
            // ASCII), so byte and char counts agree there.
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
