# ADR 0001: Document/Editor separation and an in-app agent pane

- Status: Accepted (Phases A & B implemented; Phases C & D planned)
- Date: 2026-05-26

## Context

We want to host an AI agent (e.g. a coding-agent CLI) in a pane *inside* vorto,
rather than shelling out to an external tmux/zellij split (the current `:agent`
behavior). That goal exposed a structural problem: the `Buffer` struct was a
"god struct" fusing three unrelated concerns:

1. **Document** — `lines`, undo/redo, file backing (`path`/`dirty`/`disk_meta`),
   `version`, VCS base, syntax highlighter cache.
2. **Editing session** — `cursor`, `extra_cursors`, `mode`, and the in-flight
   command token stream.
3. **View** — scroll/viewport cells (left on the document for now).

Two consequences fell out of that fusion:

- A non-document pane (an agent terminal) had no clean place in a model where
  "a pane shows a `Buffer`". The active buffer also lived in a single `App.buffer`
  field that ~329 call sites assumed *was* the focused thing.
- The same buffer could not be shown in two panes with independent cursors
  (cursor lived on the one `Buffer`), so `:split` shared a cursor — not vim-like.

## Decision

### 1. Split `Buffer` (document) from `Editor` (session)

- `Buffer` becomes a **pure document**: `lines`, undo/redo, file state, version,
  VCS, highlighter. It does not know about cursors, modes, or the command grammar
  beyond the *operation descriptors* (`MotionKind`/`Scope`/`Object`) it already
  consumed.
- `Editor` is the **per-pane editing session**: `{ doc: BufferRef, cursor,
  extra_cursors, mode, tokens }`. The cursor-aware editing operations (motions,
  text objects, operators, insert) live on `impl Editor` and take the resolved
  document as a `&Buffer`/`&mut Buffer` parameter. Pure-document primitives
  (load/save/`bump_version`, position-parameterized edits) stay on `impl Buffer`.

### 2. Pool documents; reference them by `BufferRef`

- Documents live in `App.documents: HashMap<BufferRef, Buffer>`. An `Editor`
  *references* its document via `doc: BufferRef` rather than owning it.
- Per-pane sessions are keyed by pane: `App.pane_editors: HashMap<PaneId, Editor>`
  for inactive panes; `App.editor` is the active session (hot-path fast field).
  This replaces the old `parked_buffers` (keyed by `BufferRef`) + `pane_refs`.
- Because sessions are keyed by `PaneId` and documents by `BufferRef`, **two panes
  can reference one pooled document with independent cursors** — `:split` now
  shows the same buffer in both panes; edits reflect in both, cursors stay
  separate.
- A document with no referencing pane is frozen into `sleeping` (compressed).

The active document is reached through `App::active_doc()` / `active_doc_mut()`.
Editing-operation call sites need `&mut App.editor` *and* `&mut App.documents`
simultaneously; since those are disjoint fields this is sound, and the
resolve-ref → `documents.get_mut` → `editor.op(doc)` dance is encapsulated in the
`ed_op!` / `ed_op_ref!` macros.

### 3. Panes hold content by reference; the agent is a single shared resource

- `PaneContent = Editor(Editor) | Agent` (planned, Phase C). A pane is otherwise
  just a `PaneId` in the layout tree.
- The agent is **one** App-level process: `App.agent: Option<AgentSession>`,
  launched lazily on first `:agent`. Closing an agent pane does not kill it;
  it is killed when the editor exits.

### 4. Input dispatch forks on pane kind, above the editor machinery

- `handle_key` branches on the active `PaneContent` (after the existing prompt /
  jump overlays): a buffer pane runs the existing mode → tokenize/classify →
  `evaluate` pipeline; an agent pane encodes the key to bytes and writes them to
  the agent's PTY.
- The vim command FSM (the pending `tokens`) stays per-`Editor`. A single key
  cannot be interpreted in isolation (multi-key commands like `dap`, counts like
  `2dd`), so token accumulation remains stateful inside `apply`/`evaluate`; the
  agent path, by contrast, forwards each key as bytes (the agent process does its
  own sequence interpretation).
- `Ctrl-W` is reserved in an agent pane as the escape hatch (the existing
  window-prefix), so focus can leave the pane.

## Alternatives considered and rejected

- **Full "pure eval" refactor** (turn every buffer mutation into an `EditOp`
  enum applied by a single `App::apply`): rejected. vorto already has a pure
  parse (`KeyEvent` → `Expr`) → effectful apply split at the `Expr` boundary, and
  already has undo (snapshots) and dot-repeat (`Expr` replay). Decomposing ~45
  buffer mutation primitives into a serializable enum would balloon the surface
  for little gain, and the agent does not need it.
- **`Editor` owning its `Buffer`** (no pool): used as an *intermediate* step
  (Phase A) but rejected as the end state because it cannot share one document
  across two panes.
- **Storing `&mut Buffer` inside a persistent `Editor`**: impossible in safe Rust
  (self-referential — `App` would own both the documents and editors borrowing
  into them). Hence the `BufferRef` handle + transient borrow.
- **Routing agent input through the eval/`Effect` pipeline**: unnecessary. Agent
  input is stateless byte-forwarding; it forks at `handle_key` and never enters
  the buffer command machinery.

## Consequences

Positive:

- Clear responsibilities: `Buffer` = document, `Editor` = session, `App` = owner
  of pooled documents + sessions. The agent slots in as another `PaneContent`
  without touching the buffer-editing path.
- `:split` gains independent cursors over a shared document.
- `App.mode`/`App.tokens` globals are gone (moved onto the session).

Negative / costs:

- Large mechanical migration: cursor + editing operations moved off `Buffer`
  (~270 internal sites) and ~476 `.buffer` call sites updated across two phases.
- The disjoint-field borrow of `editor` + `documents` is mediated by the
  `ed_op!`/`ed_op_ref!` macros — a deliberate ergonomic trade-off over threading
  `&mut Buffer` by hand at every call site.
- A document that sleeps loses its (per-pane) cursor; reopening mints a fresh one.

## Implementation status

- **Phase A — done** (`refactor(editor): split Buffer into document + Editor
  session`): introduced `Editor` owning a `Buffer`; moved cursor/mode/tokens and
  cursor-aware ops off `Buffer`.
- **Phase B — done** (`refactor(editor): pool documents behind BufferRef`):
  documents pooled; `Editor` references its doc; per-pane sessions keyed by
  `PaneId`; shared `:split` enabled.
- **Phase C — planned**: `PaneContent { Editor | Agent }`, single `App.agent`,
  `handle_key` dispatch, `Ctrl-W` escape hatch.
- **Phase D — planned**: `AgentSession` — `portable-pty` for the process,
  `alacritty_terminal` for VT parsing/grid, a lifted xterm key encoder, render
  the grid into the pane's rect, kill on editor exit.
