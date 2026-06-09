//! Pure parsing — `KeyEvent` → `Token`, `&[Token]` → `Expr`.
//!
//! Two stages live here:
//!
//! 1. [`tokenize`] resolves a single `KeyEvent` to an `Option<Token>` in
//!    the current parse context, looking at the trailing tokens to decide
//!    whether the next key is a count, an operator's argument, a text
//!    object follower, etc.
//! 2. [`classify`] inspects the running token list and decides if it's a
//!    completed command ([`Parse::Complete`]), a valid prefix that should
//!    keep accumulating ([`Parse::Incomplete`]), or junk to drop
//!    ([`Parse::Invalid`]).
//!
//! Both are free functions of the token slice + `Keymap` — no `App`
//! borrow, no side effects. The evaluator in `super` consumes the
//! `Expr` they produce.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::{DirectKind, Expr, MotionExpr, MotionKind, Operator, Target, Token};
use crate::config::{
    BOOKMARK_BINDINGS, BRACKET_NEXT_BINDINGS, BRACKET_PREV_BINDINGS, CTRL_W_BINDINGS,
    GOTO_BINDINGS, KeySig, Keymap, OBJECT_BINDINGS, OP_PENDING_BINDINGS, WINDOW_BINDINGS,
    Z_BINDINGS,
};
use crate::mode::Mode;

/// Result of [`classify`].
#[derive(Debug)]
pub(in crate::app) enum Parse {
    Complete(Expr),
    Incomplete,
    Invalid,
}

/// Tokenization context — what the parser is "expecting" next, derived
/// from the trailing tokens of the current command.
#[derive(Debug, Clone, Copy)]
enum ParseCtx {
    /// Top of a fresh command, or right after one or more Count tokens.
    Initial,
    /// Right after `<space>` — looking for a leader-bound action.
    LeaderPending,
    /// Right after an operator (or `<count><op>`). Now expecting
    /// a motion, a Scope marker, a Count, or the operator key itself
    /// again for the SelfDouble shortcut.
    OpPending,
    /// Right after a Scope marker (`i` / `a`). Expecting an object.
    ObjectExpected,
    /// Right after `g`. Expecting the second `g` for goto-file-start.
    GotoPending,
    /// Right after `f`/`F`/`t`/`T` (or `r`). Expecting the literal
    /// target/replacement character — the next key (whatever it is)
    /// becomes the argument. The emitted token depends on which
    /// prefix is on the stack (see [`char_arg_token`]).
    CharArgPending,
    /// Right after `z`. Expecting one of `z`/`t`/`b` for the viewport
    /// scroll-to family.
    ZPending,
    /// Right after `<space>w` (the window sub-leader). Expecting one
    /// of the keys in `WINDOW_BINDINGS` (split / focus / close /
    /// cycle).
    WindowPending,
    /// Right after `<space>m` (the bookmark sub-leader). Expecting one
    /// of the keys in `BOOKMARK_BINDINGS` (`a` add / `d` remove / `m`
    /// picker).
    BookmarkPending,
    /// Right after `Ctrl-W`. Expecting one of the keys in
    /// `CTRL_W_BINDINGS` (vim's window-prefix chord — h/j/k/l move
    /// focus, v / s split, c close, w cycle).
    CtrlWPending,
    /// Right after `]` or `[`. The follower picks a "next/prev X"
    /// target (currently just `d` for diagnostics). Which table is
    /// consulted depends on the prefix's direction, read back off
    /// the token stack in [`bracket_pending_token`].
    BracketPending,
    /// Waiting for the literal char arg of a surround command. Three
    /// shapes converge here:
    /// - `ds` / `cs` / `ds<c>cs` — char immediately after the prefix
    ///   (no target needed).
    /// - `cs<from>` — waiting for the *second* char.
    /// - `ys{target}` — waiting for the surround char after a complete
    ///   motion / text-object / `s` self-double target.
    SurroundCharPending,
}

/// Decide which tokenization context the next key falls into by looking
/// at the trailing tokens. Pure function of the token slice.
fn context_of(prev: &[Token]) -> ParseCtx {
    use Token::*;

    // Surround dispatch wins over the per-last-token rules — the
    // surround grammar weaves through the same Op/Scope/Object tokens
    // and needs a dedicated context for capturing its trailing char.
    if is_surround_char_pending(prev) {
        return ParseCtx::SurroundCharPending;
    }

    // Skip trailing Counts when deciding context — counts don't change
    // what kind of token is expected next, only the magnitude.
    let mut last: Option<&Token> = None;
    for t in prev.iter().rev() {
        if !matches!(t, Count(_)) {
            last = Some(t);
            break;
        }
    }
    match last {
        None => ParseCtx::Initial,
        Some(LeaderPrefix) => ParseCtx::LeaderPending,
        // `ys` extends operator-pending: the parser is still waiting for
        // a motion / scope / object before the final surround char.
        Some(Op(_) | SurroundAddPrefix) => ParseCtx::OpPending,
        Some(Scope(_)) => ParseCtx::ObjectExpected,
        Some(GotoPrefix) => ParseCtx::GotoPending,
        Some(FindCharPrefix { .. } | ReplaceCharPrefix) => ParseCtx::CharArgPending,
        Some(ZPrefix) => ParseCtx::ZPending,
        Some(WindowPrefix) => ParseCtx::WindowPending,
        Some(BookmarkPrefix) => ParseCtx::BookmarkPending,
        Some(CtrlWPrefix) => ParseCtx::CtrlWPending,
        Some(BracketPrefix { .. }) => ParseCtx::BracketPending,
        // After Motion/Direct/Object/SelfDouble the command is already
        // Complete; we shouldn't be tokenizing in those contexts.
        _ => ParseCtx::Initial,
    }
}

/// True when the next key should be captured as a literal
/// [`Token::SurroundChar`]. Three shapes:
/// - `ds` / `cs` — char immediately follows the prefix.
/// - `cs<c>` — second char follows the first.
/// - `ys{target}` — char follows a complete target (motion / object /
///   self-double).
fn is_surround_char_pending(prev: &[Token]) -> bool {
    use Token::*;
    if matches!(
        prev,
        [.., SurroundDeletePrefix]
            | [.., SurroundChangePrefix]
            | [.., SurroundChangePrefix, SurroundChar(_)]
    ) {
        return true;
    }
    // ys: locate SurroundAddPrefix, see if everything after is a
    // completed operator target.
    for (i, t) in prev.iter().enumerate().rev() {
        if matches!(t, SurroundAddPrefix) {
            return surround_add_target_complete(&prev[i + 1..]);
        }
    }
    false
}

/// True when `tail` (the tokens after `SurroundAddPrefix`) forms a
/// complete operator target. Mirrors the shapes accepted by
/// [`build_op_expr`] minus search-match (which `ys` doesn't support).
fn surround_add_target_complete(tail: &[Token]) -> bool {
    use Token::*;
    let (_, after) = take_count(tail);
    matches!(
        after,
        [Motion(_)]
            | [FindCharPrefix { .. }, Motion(_)]
            | [GotoPrefix, Motion(_)]
            | [Scope(_), Object(_)]
            | [SelfDouble(_)]
    )
}

/// Resolve a key to its token in the current parse context.
///
/// Returns `None` when the key has no meaning in the current context —
/// the caller should treat this as a parse abort (clear the token
/// list). Only called for Normal mode.
pub(in crate::app) fn tokenize(
    km: &Keymap,
    prev: &[Token],
    mode: Mode,
    key: KeyEvent,
) -> Option<Token> {
    debug_assert_eq!(mode, Mode::Normal);

    // Ctrl-r is redo (vim convention). Works in any context.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
        return Some(Token::Direct(DirectKind::Redo));
    }
    // Ctrl-w opens the window-prefix sub-grammar; the next key
    // resolves through `CTRL_W_BINDINGS`. Only fires at the start of a
    // fresh command — using `<C-w>` mid-sequence would clobber a
    // pending operator/scope state.
    if prev.is_empty()
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('w')
    {
        return Some(Token::CtrlWPrefix);
    }

    let ctx = context_of(prev);
    let code = key.code;

    // Digit handling stays special: count parsing is a parser
    // primitive, not a user-rebindable shortcut.
    if let Some(c) = ascii_digit(code) {
        let already_counting = matches!(prev.last(), Some(Token::Count(_)));
        let d = c.to_digit(10).unwrap();
        return match (ctx, c, already_counting) {
            // 0 alone in Initial is the line-start motion, not a count.
            (ParseCtx::Initial, '0', false) => Some(Token::Motion(MotionKind::LineStart)),
            // 0 inside a running count extends it.
            (_, '0', true) => Some(Token::Count(0)),
            // 1-9 always starts/extends a count (Initial or OpPending).
            (ParseCtx::Initial | ParseCtx::OpPending, '1'..='9', _) => Some(Token::Count(d)),
            // In LeaderPending / ObjectExpected, digits don't make sense.
            _ => None,
        };
    }

    let sig = KeySig::from_event(key);
    match ctx {
        ParseCtx::Initial => km.initial.get(&sig).copied(),
        ParseCtx::LeaderPending => km.leader.get(&sig).copied(),
        ParseCtx::OpPending => op_pending_token(code, prev),
        ParseCtx::ObjectExpected => object_token(code),
        ParseCtx::GotoPending => goto_pending_token(code),
        ParseCtx::CharArgPending => char_arg_token(code, prev),
        ParseCtx::ZPending => z_pending_token(code),
        ParseCtx::WindowPending => window_pending_token(code),
        ParseCtx::BookmarkPending => bookmark_pending_token(code),
        ParseCtx::CtrlWPending => ctrl_w_pending_token(code),
        ParseCtx::BracketPending => bracket_pending_token(code, prev),
        ParseCtx::SurroundCharPending => surround_char_token(code),
    }
}

/// In `SurroundCharPending`, capture any printable character as the
/// literal surround arg. Non-char keys (Esc, arrow, etc.) return `None`
/// so the caller aborts the parse.
fn surround_char_token(code: KeyCode) -> Option<Token> {
    match code {
        KeyCode::Char(c) => Some(Token::SurroundChar(c)),
        _ => None,
    }
}

/// Resolve the follower after `]` or `[`. Direction is read back off
/// the token stack: the most recent `BracketPrefix` carries `forward`,
/// which picks between [`BRACKET_NEXT_BINDINGS`] and
/// [`BRACKET_PREV_BINDINGS`].
fn bracket_pending_token(code: KeyCode, prev: &[Token]) -> Option<Token> {
    let forward = prev.iter().rev().find_map(|t| match t {
        Token::BracketPrefix { forward } => Some(*forward),
        _ => None,
    })?;
    let table = if forward {
        BRACKET_NEXT_BINDINGS
    } else {
        BRACKET_PREV_BINDINGS
    };
    table.iter().find(|b| b.matches(code)).map(|b| b.token)
}

fn window_pending_token(code: crossterm::event::KeyCode) -> Option<Token> {
    WINDOW_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

fn ctrl_w_pending_token(code: crossterm::event::KeyCode) -> Option<Token> {
    CTRL_W_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

fn bookmark_pending_token(code: crossterm::event::KeyCode) -> Option<Token> {
    BOOKMARK_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

/// In CharArgPending, any printable character becomes the literal
/// argument. The output token depends on the most recent pending
/// prefix — `f`/`F`/`t`/`T` produce a `FindChar` motion, `r`
/// produces a `ReplaceChar` direct.
fn char_arg_token(code: KeyCode, prev: &[Token]) -> Option<Token> {
    let prefix = prev
        .iter()
        .rev()
        .find(|t| matches!(t, Token::FindCharPrefix { .. } | Token::ReplaceCharPrefix))?;
    let KeyCode::Char(ch) = code else {
        // Escape/arrow/etc abort the pending arg — return None so the
        // caller clears the token stack.
        return None;
    };
    match prefix {
        Token::FindCharPrefix { forward, till } => Some(Token::Motion(MotionKind::FindChar {
            ch,
            forward: *forward,
            till: *till,
        })),
        Token::ReplaceCharPrefix => Some(Token::Direct(DirectKind::ReplaceChar { ch })),
        _ => None,
    }
}

fn z_pending_token(code: KeyCode) -> Option<Token> {
    Z_BINDINGS.iter().find(|b| b.matches(code)).map(|b| b.token)
}

fn goto_pending_token(code: KeyCode) -> Option<Token> {
    GOTO_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

fn op_pending_token(code: KeyCode, prev: &[Token]) -> Option<Token> {
    // The most recent Op token is the one we're following.
    let pending_op = prev.iter().rev().find_map(|t| match t {
        Token::Op(o) => Some(*o),
        _ => None,
    })?;

    // `ys` / `cs` / `ds` — `s` immediately after a fresh y/c/d operator
    // opens the surround grammar. After the prefix is already on the
    // stack the same `s` falls through to the self-double branch below
    // (for `yss`).
    if matches!(code, KeyCode::Char('s'))
        && matches!(prev.last(), Some(Token::Op(_)))
        && matches!(
            pending_op,
            Operator::Yank | Operator::Change | Operator::Delete
        )
    {
        return Some(match pending_op {
            Operator::Yank => Token::SurroundAddPrefix,
            Operator::Change => Token::SurroundChangePrefix,
            Operator::Delete => Token::SurroundDeletePrefix,
            _ => unreachable!(),
        });
    }

    // `yss` — second `s` after `[Op(Yank), SurroundAddPrefix]` means
    // "surround the current line", same line-wise role as `dd` / `yy`.
    if matches!(code, KeyCode::Char('s')) && matches!(prev.last(), Some(Token::SurroundAddPrefix)) {
        return Some(Token::SelfDouble(pending_op));
    }

    // Operator key pressed again: SelfDouble (dd, yy, cc). Stays inline
    // because the matching key is determined by the active operator
    // rather than by a static table.
    let same_key = matches!(
        (pending_op, code),
        (Operator::Delete, KeyCode::Char('d'))
            | (Operator::Yank, KeyCode::Char('y'))
            | (Operator::Change, KeyCode::Char('c'))
            | (Operator::Indent, KeyCode::Char('>'))
            | (Operator::Dedent, KeyCode::Char('<'))
            // `gcc` / `gbc` follow vim-commentary / Comment.nvim: the
            // second `c` (not the operator's own letter `b`) is the
            // self-double key for block comments too, mirroring how
            // `gcc` reads as "comment the current line".
            | (Operator::Comment, KeyCode::Char('c'))
            | (Operator::BlockComment, KeyCode::Char('c'))
    );
    if same_key {
        return Some(Token::SelfDouble(pending_op));
    }

    OP_PENDING_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

fn ascii_digit(code: KeyCode) -> Option<char> {
    match code {
        KeyCode::Char(c) if c.is_ascii_digit() => Some(c),
        _ => None,
    }
}

fn object_token(code: KeyCode) -> Option<Token> {
    OBJECT_BINDINGS
        .iter()
        .find(|b| b.matches(code))
        .map(|b| b.token)
}

// ────────────────────────────────────────────────────────────────────────
// Count helpers
// ────────────────────────────────────────────────────────────────────────

/// Peel leading `Count(_)` tokens off the slice and combine them into one
/// number (with `1` as default when none are present).
fn take_count(tokens: &[Token]) -> (u32, &[Token]) {
    let mut count: u32 = 0;
    let mut i = 0;
    while let Some(Token::Count(d)) = tokens.get(i) {
        count = count.saturating_mul(10).saturating_add(*d);
        i += 1;
    }
    if i == 0 {
        (1, tokens)
    } else {
        (count.max(1), &tokens[i..])
    }
}

/// Encode the count for a standalone motion, special-casing `gg`/`G`.
/// For [`MotionKind::FileStart`]/[`MotionKind::FileEnd`] a *missing*
/// count becomes the sentinel `0` (handled as file-edge), while any
/// typed count — including `1` — passes through as the target line.
/// Every other motion keeps the regular `>= 1` count.
fn goto_aware_count(motion: MotionKind, outer_count: u32, count_present: bool) -> u32 {
    match motion {
        MotionKind::FileStart | MotionKind::FileEnd if !count_present => 0,
        _ => outer_count,
    }
}

// ────────────────────────────────────────────────────────────────────────
// classify + build_expr
// ────────────────────────────────────────────────────────────────────────

/// Try to interpret the current token list. Returns Complete with the
/// resulting Expr when the list is a finished command, Incomplete when
/// it's a valid prefix of one, or Invalid otherwise.
pub(in crate::app) fn classify(tokens: &[Token]) -> Parse {
    if let Some(expr) = build_expr(tokens) {
        return Parse::Complete(expr);
    }
    if is_valid_prefix(tokens) {
        return Parse::Incomplete;
    }
    Parse::Invalid
}

fn build_expr(tokens: &[Token]) -> Option<Expr> {
    use Token::*;
    // Whether the user actually typed a count. `gg`/`G` need this to
    // tell bare (`gg` → file start, `G` → file end) from an explicit
    // `1gg`/`1G`, which vim treats as a line-number jump to line 1.
    let count_present = matches!(tokens.first(), Some(Token::Count(_)));
    let (outer_count, rest) = take_count(tokens);

    match rest {
        // Direct standalone — count usually meaningless, kept for parity.
        [Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // Motion alone or with leading count (already captured). `gg`/`G`
        // (FileStart/FileEnd) carry a sentinel `0` when no count was
        // typed so `handle_motion` can pick file-edge vs line-jump.
        [Motion(m)] => Some(Expr::Motion(MotionExpr {
            motion: *m,
            count: goto_aware_count(*m, outer_count, count_present),
        })),

        // `f<c>` / `t<c>` / etc — the prefix is purely a parser
        // shaping token and disappears at the AST level.
        [FindCharPrefix { .. }, Motion(m)] => Some(Expr::Motion(MotionExpr {
            motion: *m,
            count: outer_count,
        })),

        // Leader-style: <space>f, <space>l
        [LeaderPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // `<space>c` — a leader binding may emit a SelfDouble token to
        // act as a single-key alias for an operator's line-wise form
        // (e.g. `<space>c` ≡ `gcc`). Same shape as the post-operator
        // `[SelfDouble(_)]` arm below.
        [LeaderPrefix, SelfDouble(op)] => Some(Expr::Op {
            op: *op,
            target: Target::LineWise,
            outer_count,
        }),

        // Window sub-leader: <space>w v, <space>w h, <space>w <arrow>, ...
        [LeaderPrefix, WindowPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // Bookmark sub-leader: <space>m a, <space>m d, <space>m m.
        [LeaderPrefix, BookmarkPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // Vim window-prefix chord: <C-w>h, <C-w>v, <C-w>w, ...
        [CtrlWPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // `]d` / `[d` — bracket prefix followed by a direction-baked
        // direct from `BRACKET_*_BINDINGS`. The prefix dropped at the
        // AST level; direction lives inside `d` itself.
        [BracketPrefix { .. }, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // gg → file start; with a count it's a line jump (5gg = line 5,
        // 1gg = line 1). The sentinel `0` means "no count → file start".
        [GotoPrefix, GotoPrefix] => Some(Expr::Motion(MotionExpr {
            motion: MotionKind::FileStart,
            count: if count_present { outer_count } else { 0 },
        })),

        // gd / gr — goto-prefix followed by an LSP action
        [GotoPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // g_ / ge / gE / gs / gl — goto-prefix followed by a motion.
        // Drops the prefix at the AST level.
        [GotoPrefix, Motion(m)] => Some(Expr::Motion(MotionExpr {
            motion: *m,
            count: outer_count,
        })),

        // `gc` / `gb` — goto-prefix introducing a comment operator.
        // Strip the prefix and reuse the regular operator builder so
        // motion / text-object / self-double targets all work the same
        // as for `d`, `y`, `c` (`gcc`, `gcap`, `gci{`, `gcw`, …).
        [GotoPrefix, Op(op), inner @ ..] => build_op_expr(*op, inner, outer_count),

        // zz / zt / zb — z-prefix followed by a viewport direct.
        [ZPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // `r<c>` — the prefix is purely a parser shaping token; the
        // emitted `ReplaceChar` direct carries the typed character.
        [ReplaceCharPrefix, Direct(d)] => Some(Expr::Direct {
            kind: *d,
            count: outer_count,
        }),

        // `ys{target}{ch}` — surround add. The leading `Op(Yank)` is
        // the parser anchor and disappears at the AST level; the
        // surround prefix likewise. Outer counts are ignored — vim's
        // surround doesn't multiply.
        [Op(Operator::Yank), SurroundAddPrefix, inner @ ..] => build_surround_add(inner),

        // `cs{from}{to}` — surround change. Same anchor pattern.
        [
            Op(Operator::Change),
            SurroundChangePrefix,
            SurroundChar(from),
            SurroundChar(to),
        ] => Some(Expr::SurroundChange {
            from: *from,
            to: *to,
        }),

        // `ds{ch}` — surround delete.
        [Op(Operator::Delete), SurroundDeletePrefix, SurroundChar(ch)] => {
            Some(Expr::SurroundDelete { ch: *ch })
        }

        // Operator + something
        [Op(op), inner @ ..] => build_op_expr(*op, inner, outer_count),

        _ => None,
    }
}

/// Assemble [`Expr::SurroundAdd`] from the tokens after
/// `[Op(Yank), SurroundAddPrefix]`. `inner` already had the surround
/// anchor stripped; what's left is `[…target…, SurroundChar(c)]`.
fn build_surround_add(inner: &[Token]) -> Option<Expr> {
    use Token::*;
    let last = inner.last()?;
    let SurroundChar(ch) = last else {
        return None;
    };
    let target_tokens = &inner[..inner.len() - 1];
    let (motion_count, body) = take_count(target_tokens);

    let target = match body {
        [SelfDouble(_)] => Target::LineWise,
        [Motion(m)] | [FindCharPrefix { .. }, Motion(m)] | [GotoPrefix, Motion(m)] => {
            Target::Motion(MotionExpr {
                motion: *m,
                count: motion_count,
            })
        }
        [Scope(s), Object(o)] if motion_count == 1 => Target::TextObject {
            scope: *s,
            object: *o,
        },
        _ => return None,
    };
    Some(Expr::SurroundAdd { target, ch: *ch })
}

fn build_op_expr(op: Operator, after_op: &[Token], outer_count: u32) -> Option<Expr> {
    use Token::*;
    let (motion_count, body) = take_count(after_op);

    match body {
        // dd / yy / cc
        [SelfDouble(_)] => Some(Expr::Op {
            op,
            target: Target::LineWise,
            outer_count: outer_count.saturating_mul(motion_count),
        }),

        // dw / 3dw / d3w / 3d2w — motion-based
        [Motion(m)] => Some(Expr::Op {
            op,
            target: Target::Motion(MotionExpr {
                motion: *m,
                count: motion_count,
            }),
            outer_count,
        }),

        // `df<c>` / `2dt<c>` — operator followed by a char-find motion.
        // The FindCharPrefix is a parser shaping token and is dropped
        // from the AST.
        [FindCharPrefix { .. }, Motion(m)] => Some(Expr::Op {
            op,
            target: Target::Motion(MotionExpr {
                motion: *m,
                count: motion_count,
            }),
            outer_count,
        }),

        // `dg_` / `dge` / etc — operator followed by a `g`-prefixed
        // motion. Same parser-shaping treatment as the find-char case.
        [GotoPrefix, Motion(m)] => Some(Expr::Op {
            op,
            target: Target::Motion(MotionExpr {
                motion: *m,
                count: motion_count,
            }),
            outer_count,
        }),

        // `cgn` / `dgn` / `ygn` (and the `gN` variants) — operator
        // followed by the gn target. Doesn't fit `Target::Motion`
        // because the range starts at the match (not the cursor); use
        // the dedicated `SearchMatch` target.
        [GotoPrefix, Direct(DirectKind::SearchSelectNext { reverse })] => Some(Expr::Op {
            op,
            target: Target::SearchMatch { reverse: *reverse },
            outer_count: outer_count.saturating_mul(motion_count),
        }),

        // dib / di" — text objects (motion_count must be 1; multi-count
        // on a text object isn't supported yet)
        [Scope(s), Object(o)] if motion_count == 1 => Some(Expr::Op {
            op,
            target: Target::TextObject {
                scope: *s,
                object: *o,
            },
            outer_count,
        }),

        _ => None,
    }
}

/// True if the token slice is the prefix of some buildable command.
/// Used to decide between Incomplete (keep accumulating) and Invalid
/// (clear and beep).
fn is_valid_prefix(tokens: &[Token]) -> bool {
    use Token::*;
    // Strip leading counts — they're transparent to validity.
    let (_, rest) = take_count(tokens);
    match rest {
        [] => true,                             // just counts so far
        [LeaderPrefix] => true,                 // <space> waiting for follower
        [LeaderPrefix, WindowPrefix] => true,   // <space>w waiting for v/h/c/o/arrow
        [LeaderPrefix, BookmarkPrefix] => true, // <space>m waiting for a/d/m
        [CtrlWPrefix] => true,                  // <C-w> waiting for follower
        [GotoPrefix] => true,                   // g waiting for the second g
        [BracketPrefix { .. }] => true,         // ] or [ waiting for the follower
        [ZPrefix] => true,                      // z waiting for z/t/b
        [FindCharPrefix { .. }] => true,        // f/F/t/T waiting for the literal char
        [ReplaceCharPrefix] => true,            // r waiting for the replacement
        [Op(_)] => true,                        // d / y / c waiting
        [Op(_), Scope(_)] => true,              // di waiting for an object
        [Op(_), FindCharPrefix { .. }] => true, // df / dt waiting for the char
        [Op(_), GotoPrefix] => true,            // dg waiting for the follower
        [Op(_), Count(_), ..] => {
            // After Op + inner counts the only continuations we can
            // still extend are Scope (heading for a text object) and
            // FindCharPrefix (heading for an `f<c>` style target).
            let after_op = &rest[1..];
            let (_, after_inner_count) = take_count(after_op);
            matches!(after_inner_count, [] | [Scope(_)] | [FindCharPrefix { .. }])
        }
        // `gc` / `gci` / `gc<count>...` — recurse into the post-prefix
        // tail so a `g`-introduced operator keeps the same valid-prefix
        // rules as a bare operator.
        [GotoPrefix, after @ ..] if matches!(after.first(), Some(Op(_))) => is_valid_prefix(after),
        // `ds` waiting for its single char.
        [Op(Operator::Delete), SurroundDeletePrefix] => true,
        // `cs` waiting for its first char.
        [Op(Operator::Change), SurroundChangePrefix] => true,
        // `cs<from>` waiting for its second char.
        [Op(Operator::Change), SurroundChangePrefix, SurroundChar(_)] => true,
        // `ys` and friends — anything from "just the prefix" to "prefix
        // + complete target" is a valid prefix, since the final
        // surround char is still pending.
        [Op(Operator::Yank), SurroundAddPrefix, after @ ..] => is_valid_ys_tail(after),
        _ => false,
    }
}

/// Valid `[..., SurroundAddPrefix, <tail>]` shapes — anything between
/// "just the prefix" (empty tail) and "complete target without char"
/// counts as a valid prefix. The actual char closes the parse via
/// [`build_surround_add`].
fn is_valid_ys_tail(tail: &[Token]) -> bool {
    use Token::*;
    let (_, after) = take_count(tail);
    matches!(
        after,
        [] | [Scope(_)]
            | [FindCharPrefix { .. }]
            | [GotoPrefix]
            | [Motion(_)]
            | [FindCharPrefix { .. }, Motion(_)]
            | [GotoPrefix, Motion(_)]
            | [Scope(_), Object(_)]
            | [SelfDouble(_)]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Object, Operator, Scope as ScopeKind};

    /// Sugar: build a token slice from raw `Token::…` values.
    fn complete(toks: &[Token]) -> Option<Expr> {
        match classify(toks) {
            Parse::Complete(e) => Some(e),
            _ => None,
        }
    }

    fn incomplete(toks: &[Token]) -> bool {
        matches!(classify(toks), Parse::Incomplete)
    }

    fn invalid(toks: &[Token]) -> bool {
        matches!(classify(toks), Parse::Invalid)
    }

    #[test]
    fn bookmark_subleader_parses_and_waits() {
        use crate::action::DirectKind;
        // `<space>m a` completes to the add action.
        assert_eq!(
            complete(&[
                Token::LeaderPrefix,
                Token::BookmarkPrefix,
                Token::Direct(DirectKind::BookmarkAdd),
            ]),
            Some(Expr::Direct {
                kind: DirectKind::BookmarkAdd,
                count: 1,
            })
        );
        // `<space>m d` completes to remove-here.
        assert_eq!(
            complete(&[
                Token::LeaderPrefix,
                Token::BookmarkPrefix,
                Token::Direct(DirectKind::BookmarkRemoveCurrent),
            ]),
            Some(Expr::Direct {
                kind: DirectKind::BookmarkRemoveCurrent,
                count: 1,
            })
        );
        // `<space>m` alone is a valid in-progress prefix, not an abort.
        assert!(incomplete(&[Token::LeaderPrefix, Token::BookmarkPrefix]));
    }

    #[test]
    fn goto_motions_distinguish_bare_from_explicit_count() {
        // Bare `gg` / `G` carry the `0` sentinel (file-edge); any typed
        // count — including `1` — is a line-number jump.
        let motion = |toks: &[Token]| match complete(toks) {
            Some(Expr::Motion(m)) => Some(m),
            _ => None,
        };

        assert_eq!(
            motion(&[Token::GotoPrefix, Token::GotoPrefix]),
            Some(MotionExpr {
                motion: MotionKind::FileStart,
                count: 0,
            })
        );
        assert_eq!(
            motion(&[Token::Count(1), Token::GotoPrefix, Token::GotoPrefix]),
            Some(MotionExpr {
                motion: MotionKind::FileStart,
                count: 1,
            })
        );
        assert_eq!(
            motion(&[Token::Motion(MotionKind::FileEnd)]),
            Some(MotionExpr {
                motion: MotionKind::FileEnd,
                count: 0,
            })
        );
        assert_eq!(
            motion(&[Token::Count(1), Token::Motion(MotionKind::FileEnd)]),
            Some(MotionExpr {
                motion: MotionKind::FileEnd,
                count: 1,
            })
        );
        // A regular motion still defaults to count 1, never the sentinel.
        assert_eq!(
            motion(&[Token::Motion(MotionKind::Down)]),
            Some(MotionExpr {
                motion: MotionKind::Down,
                count: 1,
            })
        );
    }

    #[test]
    fn ds_complete_with_one_char() {
        let toks = [
            Token::Op(Operator::Delete),
            Token::SurroundDeletePrefix,
            Token::SurroundChar('"'),
        ];
        assert_eq!(complete(&toks), Some(Expr::SurroundDelete { ch: '"' }));
    }

    #[test]
    fn ds_prefix_alone_is_incomplete() {
        assert!(incomplete(&[
            Token::Op(Operator::Delete),
            Token::SurroundDeletePrefix
        ]));
    }

    #[test]
    fn cs_needs_two_chars() {
        let prefix = [Token::Op(Operator::Change), Token::SurroundChangePrefix];
        assert!(incomplete(&prefix));

        let one_char = [
            Token::Op(Operator::Change),
            Token::SurroundChangePrefix,
            Token::SurroundChar('"'),
        ];
        assert!(incomplete(&one_char));

        let full = [
            Token::Op(Operator::Change),
            Token::SurroundChangePrefix,
            Token::SurroundChar('"'),
            Token::SurroundChar('\''),
        ];
        assert_eq!(
            complete(&full),
            Some(Expr::SurroundChange {
                from: '"',
                to: '\''
            })
        );
    }

    #[test]
    fn ys_with_text_object() {
        let toks = [
            Token::Op(Operator::Yank),
            Token::SurroundAddPrefix,
            Token::Scope(ScopeKind::Inner),
            Token::Object(Object::Word),
            Token::SurroundChar('"'),
        ];
        let expected = Expr::SurroundAdd {
            target: Target::TextObject {
                scope: ScopeKind::Inner,
                object: Object::Word,
            },
            ch: '"',
        };
        assert_eq!(complete(&toks), Some(expected));
    }

    #[test]
    fn ys_with_motion() {
        let toks = [
            Token::Op(Operator::Yank),
            Token::SurroundAddPrefix,
            Token::Motion(MotionKind::WordForward),
            Token::SurroundChar(')'),
        ];
        let expected = Expr::SurroundAdd {
            target: Target::Motion(MotionExpr {
                motion: MotionKind::WordForward,
                count: 1,
            }),
            ch: ')',
        };
        assert_eq!(complete(&toks), Some(expected));
    }

    #[test]
    fn yss_is_line_wise() {
        let toks = [
            Token::Op(Operator::Yank),
            Token::SurroundAddPrefix,
            Token::SelfDouble(Operator::Yank),
            Token::SurroundChar('"'),
        ];
        let expected = Expr::SurroundAdd {
            target: Target::LineWise,
            ch: '"',
        };
        assert_eq!(complete(&toks), Some(expected));
    }

    #[test]
    fn ys_intermediate_states_are_incomplete() {
        let cases: Vec<Vec<Token>> = vec![
            vec![Token::Op(Operator::Yank), Token::SurroundAddPrefix],
            vec![
                Token::Op(Operator::Yank),
                Token::SurroundAddPrefix,
                Token::Scope(ScopeKind::Inner),
            ],
            vec![
                Token::Op(Operator::Yank),
                Token::SurroundAddPrefix,
                Token::Scope(ScopeKind::Inner),
                Token::Object(Object::Word),
            ],
            vec![
                Token::Op(Operator::Yank),
                Token::SurroundAddPrefix,
                Token::Motion(MotionKind::WordForward),
            ],
        ];
        for c in cases {
            assert!(incomplete(&c), "expected incomplete: {:?}", c);
        }
    }

    #[test]
    fn plain_yank_still_parses() {
        // Make sure the surround grammar didn't break ordinary yank.
        let toks = [
            Token::Op(Operator::Yank),
            Token::Motion(MotionKind::WordForward),
        ];
        let expected = Expr::Op {
            op: Operator::Yank,
            target: Target::Motion(MotionExpr {
                motion: MotionKind::WordForward,
                count: 1,
            }),
            outer_count: 1,
        };
        assert_eq!(complete(&toks), Some(expected));
    }

    #[test]
    fn s_after_indent_op_is_invalid() {
        // `>` then `s` isn't a known prefix and isn't a motion — should
        // not silently produce a surround command.
        let toks = [
            Token::Op(Operator::Indent),
            Token::SurroundAddPrefix,
            Token::SurroundChar('"'),
        ];
        assert!(invalid(&toks));
    }
}
