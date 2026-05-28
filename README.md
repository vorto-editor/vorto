# vorto

A Vim-flavored modal terminal editor, written in Rust.

## Vorto is…

a modal terminal text editor with batteries included. Tree-sitter
highlighting, Language Server Protocol support, fuzzy pickers, an
in-editor AI agent pane, and optional Copilot inline completions — all
in a single, dependency-light binary that starts instantly and needs no
Node toolchain, plugin manager, or external repos to clone.

## Why vorto

- **Batteries included, not assembled.** LSP, tree-sitter, fuzzy
  pickers, a file explorer, git gutter, themes, multi-cursor, and
  bookmarks all work out of the box — there is no plugin manager to
  learn and nothing to wire together.
- **A single static binary.** Pure Rust, no runtime dependencies beyond
  a C compiler for building grammars on demand.
- **No Node, no clones.** Tree-sitter grammars install on demand — no
  `tree-sitter-cli`, no external repos, no Node toolchain.
- **Responsive by design.** An async architecture keeps the UI snappy
  while language servers index and grammars parse in the background.
- **Familiar.** Vim-style modal editing with operators, motions,
  tree-sitter text objects, multi-cursor, jump labels, bookmarks, and
  `.`-repeat.
- **Themable.** Helix-compatible TOML themes — most Helix theme files
  drop in unchanged.
- **Optional AI.** Ghost-text inline completions via
  `copilot-language-server` and an `:agent` pane that runs an AI coding
  agent inside the editor — both silent when not configured.

## Installation

From crates.io:

```sh
cargo install vorto
```

From source:

```sh
git clone https://github.com/vorto-editor/vorto.git
cd vorto
make install            # installs to ~/.local/bin
# or
cargo build --release   # binary at target/release/vorto
```

Requires a Rust toolchain (edition 2024). A C compiler is needed when
building tree-sitter grammars.

```sh
vorto [FILE]
vorto -h | --help
vorto -V | --version
```

## Features

- [Editing](#editing)
- [Language Server Protocol](#language-server-protocol)
- [Tree-sitter](#tree-sitter)
- [UI](#ui)
- [Theming](#theming)
- [AI (optional)](#ai-optional)
- [Git](#git)
- [Auto-reload](#auto-reload)

### Editing

- Normal / Insert / Visual (char, line, block) / Command modes
- Operators: `d` `y` `c` `>` `<`
- Motions: `h j k l`, `w b e W B E ge gE`, `f F t T ; ,`, `gg G H M L`,
  `{ }`, `%`, `<C-f> <C-b> <C-d> <C-u>`
- Search: `/ ?`, `n N`, `* #`, `gn gN`
- Text objects: `iw aw`, `i" a"`, `i( a(`, `i{ a{`, `i[ a[`, `ip ap`,
  plus tree-sitter-aware `if af` (function), `ic ac` (class),
  `i, a,` (parameter)
- Multi-cursor: `+` add next match, `Shift-Down` add below, `-` pop,
  `<space>,` clear
- Jump labels: `gw` (easymotion-style 2-char word jumps)
- Jump history: `<C-o>` / `<C-i>` (or `Tab`) walk the per-pane jumplist;
  `:jumps` / `<space>j` open a fuzzy picker over it
- Bookmarks (harpoon-style): `<space>ma` add, `<space>md` remove,
  `<space>mm` open the picker; `:bookmarks` (alias `:bm`) takes
  `add` / `delete` / `list`. Marked lines show a `●` in the gutter and
  persist per-project across restarts
- Per-buffer cursor memory: switching back to a buffer restores the
  cursor where you left it (tracked independently per pane)
- Auto-pair brackets and quotes, language-aware comment toggle
  (`<space>c`), case toggle, line join, repeat with `.`
- Undo / redo (`u` / `<C-r>`), system clipboard yank/paste via
  `arboard`

### Language Server Protocol

| Feature                | Binding / Trigger                   |
| ---------------------- | ----------------------------------- |
| Diagnostics            | inline, navigate with `]d` / `[d`   |
| Hover                  | `K` (scrollable markdown popup)     |
| Completion             | `<C-n>` / `<C-p>` + trigger chars   |
| Signature help         | auto on `(` / `,`                   |
| Code actions           | `<space>a`                          |
| Goto definition        | `gd`                                |
| Goto declaration       | `gD`                                |
| Goto implementation    | `gi`                                |
| References             | `gr` (preview picker)               |
| Rename                 | `<space>r`                          |
| Format on save         | `:w` runs configured formatter      |

Servers and formatters ship with built-in defaults for the common
languages (rust-analyzer, pyright, gopls, clangd, vtsls, and ~30
more). Override or add to them per-language in `config.toml`.

### Tree-sitter

- Built-in grammar recipes for 37 languages: **rust, python, go,
  javascript, typescript, tsx, toml, kotlin, c, cpp, java, bash, json,
  yaml, markdown, html, css, lua, ruby, zig, sql, dockerfile, make,
  hcl, diff, vue, svelte, haskell, elixir, nix, c-sharp, swift, php,
  dart, ocaml, graphql, fish**.
- Install on demand — only the grammars you use are built and cached:
  ```sh
  vorto grammar list
  vorto grammar install rust python
  vorto grammar install --all
  vorto grammar install-queries rust
  vorto grammar remove rust
  ```
- Or from inside the editor with `:grammar` — opens a modal listing
  every grammar and its install status; `j`/`k` to move, `Enter` to
  install the selected grammar (asynchronously, in the background),
  `d` to remove. The buffer re-highlights the moment an install
  finishes — no restart. `:grammar install <name>…` and
  `:grammar remove <name>…` work inline too, mirroring the CLI.
- Uses `highlights.scm` for coloring, `indents.scm` for auto-indent,
  and `textobjects.scm` for tree-sitter text objects.
- Falls back to plain text when a grammar is unavailable.

### UI

- Fuzzy pickers with live syntax-highlighted preview:
  - **Files** — `:e` (git-aware)
  - **Buffers** — `:ls` / `:buffers`
  - **Changed files** — `<space>g` (files differing from HEAD)
  - **Document symbols** — `<space><space>` (LSP)
- **File explorer** tree on `<space>e` — navigate with `j k l` /
  arrows, toggle hidden files with `.`, toggle `.gitignore`d files
  with `h`, filter with `/`, and create / delete / rename / move with
  `a` / `d` / `r` / `m`
- **Splits**: `:split` / `:vsplit`, `<space>w h` / `<space>w v`,
  cycle with `<C-w w>`
- Status bar with mode badge, filename, cursor position, dirty flag
- Configurable indentation guides (line / dot, skip levels)
- Mode-specific cursor shapes (block / bar / underline)
- Mouse support: click to move cursor, scroll to scroll
- Non-blocking toast notifications

### Theming

- `:theme` opens a filterable picker that previews each theme live on
  the current buffer as you move the cursor; Enter applies and persists
  it to `config.toml`, Esc reverts.
- Helix-compatible TOML themes — a flat scope → color map with an
  optional `[palette]`, so most Helix theme files drop in unchanged.
- Dozens of built-ins (Catppuccin, tokyonight, nord, dracula, gruvbox,
  rose-pine, kanagawa, ayu, …) plus an `ansi` theme that uses your
  terminal's own palette. See [Themes](#themes) for the full reference.

### AI (optional)

- **Inline completions** — if `copilot-language-server` is on your
  `PATH`, vorto enables ghost-text inline completions: accept with
  `<Tab>`, dismiss with `<Esc>`. Missing the binary is a no-op — no
  errors, no prompts.
- **Agent launcher** — `:agent` opens an AI coding agent in a built-in
  pane inside vorto, running under a pseudo-terminal alongside your
  buffers (no external multiplexer required). A bare `:agent` launches
  the configured default, or opens a picker the first time and remembers
  your choice. Re-running `:agent` focuses the existing pane instead of
  spawning another; the agent process keeps running even if you close its
  pane. While the agent pane is focused, every key goes to the agent
  except `Ctrl-W`, which is reserved for window navigation
  (`Ctrl-W h/j/k/l/w`) so you can move focus back to a buffer. `:agent
  explain @file` / `:agent chat @file` go further: they build a prompt
  about the active buffer and send it to the agent. Use `@selection`
  instead of `@file` to scope it to the visually-selected text: select in
  visual mode, press `:`, and the selection is captured for `:agent
  explain @selection` and embedded in the prompt as a code block. An
  unsaved or scratch buffer used as `@file` is snapshotted to a temp file
  first so the agent has something on disk to read. Built-in catalog:
  **claude, codex, gemini, aider** — override commands/args or the
  default in `config.toml` (see below).

### Git

- Diff gutter: `+` added, `~` modified, `⋮` deleted marker
- File picker honors `.gitignore` and your global excludes file
- Status bar surfaces buffer dirty state
- **Conflict markers**: `<<<<<<<` / `=======` / `>>>>>>>` blocks (and the
  diff3 `|||||||` base) are highlighted — a colored bar on each marker
  line and a dim tint on the "ours" / "theirs" sides, layered under
  syntax. Jump between conflicts with `]c` / `[c`, and resolve the one at
  the cursor with `:conflict ours` / `theirs` / `both` / `none` (undoable).
  Works on both `git` conflicts and the `autoreload = "merge"` markers
  below.

### Auto-reload

When the active buffer's file changes on disk, vorto reacts according to
the `autoreload` setting (`[editor]`, overridable per language):

- `"replace"` (default) — prompt to reload; if the buffer is dirty it
  warns that unsaved edits will be replaced (undo with `u`). Declining
  suppresses re-prompts until the file changes again.
- `"merge"` — follow the external edit via a three-way merge instead of
  prompting; conflicts are written inline with `<<<<<<<` / `=======` /
  `>>>>>>>` markers (`local (your edits)` vs `disk`) and the result is
  undoable.
- `"none"` — disable the watcher; reload manually with `:reload`.

## Configuration

Configuration lives under `$XDG_CONFIG_HOME/vorto/` (typically
`~/.config/vorto/`):

- `config.toml` — editor settings, keymap, theme, per-language LSP /
  formatter / indent overrides, a `[finder]` table (`hidden_patterns`,
  `max_items`) controlling what the file picker and explorer treat as
  hidden, and an `[agent]` / `[agents.*]` section for the `:agent`
  launcher:

  ```toml
  [agent]
  default = "claude"      # bare :agent launches this; unset → picker

  [agents.claude]         # override a built-in or add a new agent
  command = "claude"
  args = ["--model", "opus"]
  # How `:agent explain @file` passes its prompt at launch. `{prompt}` is
  # substituted with the text. Defaults to ["{prompt}"] (positional, as
  # claude/codex accept); aider needs ["--message", "{prompt}"]; set [] to
  # opt an agent out of launch-time seeding.
  prompt_args = ["{prompt}"]
  ```

  A workspace-local `.vorto/config.toml` takes precedence over the
  global file when present.
- `grammars/` — installed tree-sitter `.so` libraries
- `queries/<lang>/` — installed `highlights.scm`, `indents.scm`,
  `textobjects.scm`
- `themes/<name>.toml` — color themes (see below)

### Themes

`:theme` opens a filterable picker (`/` to filter, `j`/`k` to move) that
previews each theme live on the current buffer as you move the cursor;
Enter applies and saves `theme = "<name>"` to `config.toml`, Esc reverts.

Themes are Helix-compatible TOML — a flat scope → color map with an
optional `[palette]`, so most Helix theme files drop in unchanged:

```toml
# ~/.config/vorto/themes/mytheme.toml
keyword           = "mauve"
"function.macro"  = { fg = "mauve", modifiers = ["bold"] }
comment           = { fg = "#6c7086", modifiers = ["italic"] }
"ui.selection"    = { bg = "#313244" }

[palette]            # must come last (a TOML table header captures the
mauve = "#cba6f7"    # keys after it)
```

Color values are a `[palette]` name, a `#rrggbb` (or `#rgb`) hex literal,
or an ANSI color name. Scopes cover tree-sitter highlight captures
(`keyword`, `function.method`, …) and editor chrome — `ui.background`
(paints the whole editor; a theme that sets it recolors the background,
one that omits it keeps the terminal's), `ui.selection`, `ui.linenr`,
`ui.statusline`, `ui.popup`, ….

Built-in themes: `ansi` (terminal palette), the Catppuccin flavors
(`catppuccin-latte`, `-frappe`, `-macchiato`, `-mocha`), `tokyonight`,
`nord`, `dracula`, `onedark`/`onelight`, `rose-pine`, `gruvbox-dark`/
`-light`, `everforest-dark`/`-light`, `kanagawa`, `solarized-dark`/
`-light`, `ayu-dark`/`-mirage`/`-light`, and `monokai-pro`.
A file in `themes/` shadows a built-in of the same name. The special
theme `ansi` uses your terminal's own 16-color palette and is always
available. Set the startup theme with `theme = "<name>"` in `config.toml`.

See the [documentation site](https://docs.vorto-editor.dev/) for the
full configuration reference.

## License

Licensed under either of

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
