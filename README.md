# vorto

A modal terminal text editor written in Rust. Tree-sitter syntax
highlighting, Language Server Protocol support, fuzzy pickers, and
optional GitHub Copilot — all in a single, dependency-light binary.

## Highlights

- **Vim-style modal editing** with operators, motions, tree-sitter text
  objects, multi-cursor, jump labels, and `.`-repeat.
- **Tree-sitter highlighting** with on-demand grammar install — no
  `tree-sitter-cli`, no external repos to clone, no Node toolchain.
- **First-class LSP**: diagnostics, hover, completion, signature help,
  code actions, goto definition / declaration / implementation,
  references, rename, format-on-save.
- **Optional Copilot** via `copilot-language-server` — ghost-text inline
  completions when present, silent when not.
- **Async architecture** keeps the UI responsive while language servers
  index and grammars parse in the background.
- **Git-aware** out of the box: diff gutter, picker respects
  `.gitignore`, dirty-state indicator.
- **Single static binary**, pure Rust, no runtime dependencies beyond
  a C compiler for building grammars on demand.

## Features

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
- Uses `highlights.scm` for coloring, `indents.scm` for auto-indent,
  and `textobjects.scm` for tree-sitter text objects.
- Falls back to plain text when a grammar is unavailable.

### UI

- Fuzzy pickers with live syntax-highlighted preview:
  - **Files** — `:e` (git-aware)
  - **Buffers** — `:ls` / `:buffers`
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

### AI (optional)

If `copilot-language-server` is on your `PATH`, vorto enables inline
ghost-text completions: accept with `<Tab>`, dismiss with `<Esc>`.
Missing the binary is a no-op — no errors, no prompts.

### Git

- Diff gutter: `+` added, `~` modified, `⋮` deleted marker
- File picker honors `.gitignore` and your global excludes file
- Status bar surfaces buffer dirty state

## Install

From crates.io:

```sh
cargo install vorto
```

From source:

```sh
git clone https://github.com/shka-k/vorto.git
cd vorto
make install            # installs to ~/.local/bin
# or
cargo build --release   # binary at target/release/vorto
```

Requires a Rust toolchain (edition 2024). A C compiler is needed when
building tree-sitter grammars.

## Usage

```sh
vorto [FILE]
vorto -h | --help
vorto -V | --version
```

## Configuration

Configuration lives under `$XDG_CONFIG_HOME/vorto/` (typically
`~/.config/vorto/`):

- `config.toml` — editor settings, keymap, theme, per-language LSP /
  formatter / indent overrides, and a `[finder]` table
  (`hidden_patterns`, `max_items`) controlling what the file picker
  and explorer treat as hidden
- `grammars/` — installed tree-sitter `.so` libraries
- `queries/<lang>/` — installed `highlights.scm`, `indents.scm`,
  `textobjects.scm`

See the [documentation site](https://docs.vorto-editor.dev/) for the
full configuration reference.

## License

Licensed under either of

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
