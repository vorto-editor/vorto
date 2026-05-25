//! Overlay panels: `:command` autocomplete and the which-key panel that
//! pops up while an operator/leader/scope sequence is mid-parse.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::action::{Operator, Token};
use crate::app::{App, COPILOT_SUBCOMMANDS, GRAMMAR_SUBCOMMANDS};
use crate::config::COMMAND_BINDS;
use crate::config::{
    BRACKET_NEXT_BINDINGS, BRACKET_PREV_BINDINGS, CTRL_W_BINDINGS, GOTO_BINDINGS, LEADER_DEFAULTS,
    OBJECT_BINDINGS, OP_PENDING_BINDINGS, WINDOW_BINDINGS, Z_BINDINGS,
};
use crate::prompt::{CommandPrompt, CompletionKind};

const HINT_COLS: usize = 2;
const HINT_ROWS_MAX: usize = 24;
const HINT_MAX: usize = HINT_COLS * HINT_ROWS_MAX;
const HINT_PAD_X: u16 = 1;
const HINT_PAD_Y: u16 = 1;

/// Virtual `:` commands not represented in [`COMMAND_BINDS`] — used by
/// the inline `cmd == "copilot"` interception in
/// [`crate::app::eval`]. Listed here so the hint panel still surfaces
/// them as completion targets. The subcommand variants are *not*
/// included here on purpose: once the user has typed `copilot `, the
/// separate [`draw_command_subhints`] panel takes over with a focused
/// menu so the main commands list stays uncluttered.
const VIRTUAL_COMMANDS: &[(&str, &str)] = &[
    ("copilot", "Copilot status / signin / signout"),
    ("grammar", "install / remove tree-sitter grammars"),
    ("agent", "launch an AI agent in a tmux/zellij pane"),
];

/// Resolve `<head>` to a (title, entries) pair when there's a known
/// subcommand menu attached to it. Adding new subcommand-bearing
/// commands means extending this match — the `entries` themselves are
/// owned by their respective dispatcher modules (e.g.
/// [`COPILOT_SUBCOMMANDS`]) so this layer never duplicates them.
fn subcommand_menu(head: &str) -> Option<(&'static str, Vec<(&'static str, &'static str)>)> {
    match head {
        "copilot" => Some((
            " copilot ",
            COPILOT_SUBCOMMANDS
                .iter()
                .map(|s| (s.name, s.description))
                .collect(),
        )),
        "grammar" => Some((
            " grammar ",
            GRAMMAR_SUBCOMMANDS
                .iter()
                .map(|s| (s.name, s.description))
                .collect(),
        )),
        _ => None,
    }
}

const PENDING_HINT_WIDTH: u16 = 32;
const PENDING_HINT_ROWS_MAX: u16 = 12;

pub(super) fn draw_command_hints(f: &mut Frame, cp: &CommandPrompt, cmd_area: Rect) {
    // Resolve "what list should the panel show right now?" and which
    // entry (if any) is selected. Tab-cycling pins the panel to the
    // candidate list captured at first Tab; otherwise we filter live
    // from what the user has typed so far.
    let (title, items, selected_idx): (&str, Vec<(String, String)>, Option<usize>) =
        match &cp.completion {
            Some(c) if c.kind == CompletionKind::Path => {
                let items = c
                    .matches
                    .iter()
                    .take(HINT_MAX)
                    .map(|p| (p.clone(), String::new()))
                    .collect();
                (" files ", items, Some(c.selected))
            }
            Some(c) => {
                // Command-name cycling. The visible input has been
                // replaced with a candidate, so filter against the
                // captured prefix instead.
                let items = command_items(&c.prefix);
                let sel = c
                    .matches
                    .get(c.selected)
                    .and_then(|name| items.iter().position(|(n, _)| n == name));
                (" commands ", items, sel)
            }
            None => {
                // No cycle in progress — live filtering against what
                // the user has typed. Three shapes share this panel:
                //   - bare command name (no space)         → commands
                //   - `<head> <partial>` for a known menu  → subcommands
                //   - anything else (path arg, etc.)       → no panel
                let input = cp.input.as_str();
                match input.find([' ', '\t']) {
                    None => (" commands ", command_items(input), None),
                    Some(sp) => {
                        let head = &input[..sp];
                        let Some((title, entries)) = subcommand_menu(head) else {
                            return;
                        };
                        let partial = input[sp + 1..].trim_start_matches(['\t', ' ']);
                        (title, filter_subcommand_items(&entries, partial), None)
                    }
                }
            }
        };
    if items.is_empty() {
        return;
    }

    let rows = items.len().div_ceil(HINT_COLS).min(HINT_ROWS_MAX);
    let height = rows as u16 + 2 * HINT_PAD_Y + 2;

    let screen = f.area();
    let area = Rect {
        x: 0,
        y: cmd_area.y.saturating_sub(height),
        width: screen.width,
        height: height.min(cmd_area.y),
    };
    if area.height == 0 {
        return;
    }

    let bg = Style::default().bg(super::PANEL_BG);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(bg.fg(super::PANEL_BORDER_FG))
        .title(Span::styled(
            title.to_string(),
            bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .style(bg)
        .padding(Padding::new(HINT_PAD_X, HINT_PAD_X, HINT_PAD_Y, HINT_PAD_Y));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    // Split the inner area into two equal columns. Hints flow column-major
    // (column 0 takes items[0..rows], column 1 takes items[rows..2*rows]).
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    // Width the longest name in the matched set, so all rows align
    // in their column. Cap at 10 to keep things tidy if someone adds
    // a comically long command name later.
    let name_w = items
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(5)
        .min(20);
    // 2 cells for the `> ` / `  ` selection marker, 1 for the space
    // between name and description.
    let col_w = columns[0].width as usize;
    let desc_w = col_w.saturating_sub(2 + name_w + 1);
    let render_column = |start: usize| -> Vec<Line<'static>> {
        items
            .iter()
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(i, (name, description))| {
                let is_sel = selected_idx == Some(i);
                let row_bg = if is_sel {
                    Style::default().bg(Color::DarkGray)
                } else {
                    bg
                };
                let marker = if is_sel { "> " } else { "  " };
                let desc = truncate_or_pad(description, desc_w);
                Line::from(vec![
                    Span::styled(marker, row_bg.fg(Color::Yellow)),
                    Span::styled(
                        format!("{:<width$}", name, width = name_w),
                        row_bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {}", desc), row_bg.fg(Color::Gray)),
                ])
            })
            .collect()
    };
    f.render_widget(Paragraph::new(render_column(0)).style(bg), columns[0]);
    f.render_widget(Paragraph::new(render_column(rows)).style(bg), columns[1]);
}

/// Fit `s` into exactly `width` terminal cells: truncate with `…` if it
/// overflows, right-pad with spaces if it falls short. CJK-safe via
/// `str_cell_width`. Used so the description column's row highlight
/// covers the full column width even on short descriptions.
fn truncate_or_pad(s: &str, width: usize) -> String {
    use crate::text_width::{prefix_byte_len_for_width, str_cell_width};
    if width == 0 {
        return String::new();
    }
    let w = str_cell_width(s);
    if w == width {
        return s.to_string();
    }
    if w < width {
        let mut out = String::with_capacity(s.len() + (width - w));
        out.push_str(s);
        for _ in 0..(width - w) {
            out.push(' ');
        }
        return out;
    }
    let cut = prefix_byte_len_for_width(s, width.saturating_sub(1));
    let mut out = String::with_capacity(cut + 3);
    out.push_str(&s[..cut]);
    out.push('…');
    let after = str_cell_width(&out);
    for _ in after..width {
        out.push(' ');
    }
    out
}

/// Live-filtered command-name candidates for a given prefix, formatted
/// for the hint panel. Each row is one typeable form (primary name or
/// alias) paired with its description.
fn command_items(prefix: &str) -> Vec<(String, String)> {
    COMMAND_BINDS
        .iter()
        .flat_map(|b| b.all_names().map(move |n| (n, b.description)))
        .chain(VIRTUAL_COMMANDS.iter().copied())
        .filter(|(name, _)| name.starts_with(prefix))
        .take(HINT_MAX)
        .map(|(n, d)| (n.to_string(), d.to_string()))
        .collect()
}

fn filter_subcommand_items(
    entries: &[(&'static str, &'static str)],
    partial: &str,
) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|(name, _)| name.starts_with(partial))
        .map(|(n, d)| ((*n).to_string(), (*d).to_string()))
        .collect()
}

/// Which-key-style panel that lists valid continuations when the token
/// stream is mid-sequence. Derives hints by inspecting the trailing
/// token to figure out which parse context we're in.
pub(super) fn draw_pending_hints(f: &mut Frame, app: &App, status_area: Rect) {
    let (name, entries) = match pending_hints(&app.tokens) {
        Some(p) => p,
        None => return,
    };
    if entries.is_empty() {
        return;
    }

    let rows = (entries.len() as u16).min(PENDING_HINT_ROWS_MAX);
    let width = PENDING_HINT_WIDTH + 2 * HINT_PAD_X + 2;
    let height = rows + 2 * HINT_PAD_Y + 2;

    let screen = f.area();
    let x = screen.width.saturating_sub(width);
    let y = status_area.y.saturating_sub(height);
    let area = Rect {
        x,
        y,
        width: width.min(screen.width.saturating_sub(x)),
        height: height.min(status_area.y),
    };
    if area.height == 0 {
        return;
    }

    let bg = Style::default().bg(super::PANEL_BG);
    let title = format!(" {} ", name);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(bg.fg(super::PANEL_BORDER_FG))
        .title(Span::styled(
            title,
            bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .style(bg)
        .padding(Padding::new(HINT_PAD_X, HINT_PAD_X, HINT_PAD_Y, HINT_PAD_Y));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let body_rows = inner.height as usize;
    let lines: Vec<Line> = entries
        .iter()
        .take(body_rows)
        .map(|(k, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>4} ", k),
                    bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(desc.to_string(), bg.fg(Color::Gray)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(bg), inner);
}

/// Hint entries for the current token state. Returns `None` when nothing
/// useful can be hinted (initial state, or in the middle of a count
/// without further context).
fn pending_hints(tokens: &[Token]) -> Option<(&'static str, Vec<(String, &'static str)>)> {
    // Surround dispatch wins over the per-last-token rules: char-pending
    // states leave a Motion/Object/SelfDouble sitting on the stack and
    // the generic dispatch would otherwise miss them.
    if surround_char_pending(tokens) {
        return Some(surround_char_menu());
    }

    // Find the trailing non-Count token — counts don't change what the
    // hint context is.
    let last = tokens
        .iter()
        .rev()
        .find(|t| !matches!(t, Token::Count(_)))?;
    let (name, entries) = match last {
        // After `ys` the user is picking a target — same menu as op-
        // pending plus `s` for the line-wise self-double (`yss`).
        Token::SurroundAddPrefix => {
            let mut entries = vec![("s".to_string(), "current line (yss)")];
            entries.extend(
                OP_PENDING_BINDINGS
                    .iter()
                    .map(|b| (display_key(b.key), b.label)),
            );
            ("surround target", entries)
        }
        // `cs` / `ds` — char menu (target-less surround variants).
        Token::SurroundChangePrefix => ("change surround (from)", surround_char_entries()),
        Token::SurroundDeletePrefix => ("delete surround", surround_char_entries()),
        Token::LeaderPrefix => (
            "leader",
            LEADER_DEFAULTS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect(),
        ),
        Token::GotoPrefix => (
            "goto",
            GOTO_BINDINGS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect(),
        ),
        Token::ZPrefix => (
            "viewport",
            Z_BINDINGS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect(),
        ),
        Token::WindowPrefix => (
            "window",
            WINDOW_BINDINGS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect(),
        ),
        Token::CtrlWPrefix => (
            "window (C-w)",
            CTRL_W_BINDINGS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect(),
        ),
        Token::BracketPrefix { forward } => {
            let (name, table) = if *forward {
                ("next", BRACKET_NEXT_BINDINGS)
            } else {
                ("prev", BRACKET_PREV_BINDINGS)
            };
            (
                name,
                table
                    .iter()
                    .map(|b| (display_key(b.key), b.label))
                    .collect(),
            )
        }
        Token::FindCharPrefix { forward, till } => {
            let label = match (forward, till) {
                (true, false) => "type char to find forward",
                (false, false) => "type char to find backward",
                (true, true) => "type char to step before",
                (false, true) => "type char to step after",
            };
            ("find char", vec![("…".to_string(), label)])
        }
        Token::ReplaceCharPrefix => (
            "replace",
            vec![("…".to_string(), "type the replacement char")],
        ),
        Token::Op(op) => {
            // Each operator's repeat-self shortcut (dd/yy/cc) is the only
            // hint entry that depends on `op`; the rest of the menu is
            // the static OpPending binding table.
            let (name, self_key, self_label) = match op {
                Operator::Delete => ("delete", "d", "delete line (dd)"),
                Operator::Yank => ("yank", "y", "yank line (yy)"),
                Operator::Change => ("change", "c", "change line (cc)"),
                Operator::Indent => ("indent", ">", "indent line (>>)"),
                Operator::Dedent => ("dedent", "<", "dedent line (<<)"),
                Operator::Comment => ("comment", "c", "comment line (gcc)"),
                Operator::BlockComment => ("block-comment", "c", "block-comment line (gbc)"),
            };
            let mut entries = vec![(self_key.to_string(), self_label)];
            // y/c/d open the surround grammar with `s` — list it here
            // so users can discover ys/cs/ds without reading docs.
            let surround_hint = match op {
                Operator::Yank => Some("add surround (ys…)"),
                Operator::Change => Some("change surround (cs…)"),
                Operator::Delete => Some("delete surround (ds…)"),
                _ => None,
            };
            if let Some(label) = surround_hint {
                entries.push(("s".to_string(), label));
            }
            entries.extend(
                OP_PENDING_BINDINGS
                    .iter()
                    .map(|b| (display_key(b.key), b.label)),
            );
            (name, entries)
        }
        Token::Scope(scope) => {
            let name = match scope {
                crate::action::Scope::Inner => "inner",
                crate::action::Scope::Around => "around",
            };
            let entries = OBJECT_BINDINGS
                .iter()
                .map(|b| (display_key(b.key), b.label))
                .collect();
            (name, entries)
        }
        _ => return None,
    };
    Some((name, entries))
}

/// True when the next key in `prev` should be captured as a literal
/// surround char. Mirrors `parse::is_surround_char_pending` — kept
/// inline so the hint layer doesn't need a `pub` peephole into the
/// parser internals.
fn surround_char_pending(prev: &[Token]) -> bool {
    use Token::*;
    if matches!(
        prev,
        [.., SurroundDeletePrefix]
            | [.., SurroundChangePrefix]
            | [.., SurroundChangePrefix, SurroundChar(_)]
    ) {
        return true;
    }
    // ys with a complete target. Anything after the prefix that lands
    // on a Motion/Object/SelfDouble (the three target-terminator
    // tokens) counts as "ready for the surround char."
    let Some(prefix_idx) = prev.iter().rposition(|t| matches!(t, SurroundAddPrefix)) else {
        return false;
    };
    matches!(
        prev[prefix_idx + 1..].last(),
        Some(Motion(_)) | Some(Object(_)) | Some(SelfDouble(_))
    )
}

fn surround_char_menu() -> (&'static str, Vec<(String, &'static str)>) {
    ("surround char", surround_char_entries())
}

/// Hint table for the surround char prompt. Quotes first, then the
/// asymmetric pairs. Both opening and closing forms are valid (and
/// `b` / `B` alias `)` / `}`), but each pair gets a single row to
/// keep the menu compact — the alias hint goes in the label.
fn surround_char_entries() -> Vec<(String, &'static str)> {
    vec![
        ("\"".to_string(), "double quotes"),
        ("'".to_string(), "single quotes"),
        ("`".to_string(), "backticks"),
        ("( )".to_string(), "parens (b)"),
        ("{ }".to_string(), "braces (B)"),
        ("[ ]".to_string(), "brackets"),
        ("< >".to_string(), "angles"),
    ]
}

/// Human-readable form of a `KeyCode` for which-key hint rendering.
/// Single chars stringify to themselves; the few special keys that
/// appear as binding primaries have explicit names.
fn display_key(code: crossterm::event::KeyCode) -> String {
    use crossterm::event::KeyCode;
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        other => format!("{:?}", other),
    }
}
