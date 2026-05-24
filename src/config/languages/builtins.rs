//! Out-of-the-box LSP server and language definitions. Users overlay
//! their own `[lsp.<name>]` / `[languages.<name>]` tables onto these
//! at startup ([`super::resolve::resolve_lsp_table`] /
//! [`super::resolve::resolve`]).

use std::collections::HashMap;

use super::{FormatterToml, LanguageConfig, LspConfig};
use crate::config::editor::EditorToml;

/// Built-in `[lsp.<name>]` defaults. Users overlay onto these by
/// re-declaring `[lsp.<name>]` in their config; entirely new servers
/// can also be added.
pub fn builtin_lsp() -> HashMap<String, LspConfig> {
    let mut m = HashMap::new();
    let add = |m: &mut HashMap<String, LspConfig>,
               name: &str,
               command: &str,
               args: &[&str],
               language_id: Option<&str>,
               root_markers: &[&str]| {
        m.insert(
            name.to_string(),
            LspConfig {
                name: name.to_string(),
                command: command.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                language_id: language_id.map(|s| s.to_string()),
                root_markers: root_markers.iter().map(|s| s.to_string()).collect(),
            },
        );
    };

    add(
        &mut m,
        "rust-analyzer",
        "rust-analyzer",
        &[],
        None,
        &["Cargo.toml", "rust-project.json"],
    );
    add(
        &mut m,
        "pyright",
        "pyright-langserver",
        &["--stdio"],
        None,
        &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
    );
    add(&mut m, "taplo", "taplo", &["lsp", "stdio"], None, &[]);
    add(
        &mut m,
        "vtsls",
        "vtsls",
        &["--stdio"],
        None,
        &["package.json", "tsconfig.json"],
    );
    add(
        &mut m,
        "typescript-language-server",
        "typescript-language-server",
        &["--stdio"],
        None,
        &["package.json", "tsconfig.json", "jsconfig.json"],
    );
    add(&mut m, "gopls", "gopls", &[], None, &["go.mod", "go.work"]);
    add(
        &mut m,
        "kotlin-lsp",
        "kotlin-lsp",
        &["--stdio"],
        None,
        &[
            "settings.gradle.kts",
            "settings.gradle",
            "build.gradle.kts",
            "build.gradle",
            "pom.xml",
        ],
    );
    add(
        &mut m,
        "clangd",
        "clangd",
        &[],
        None,
        &[
            "compile_commands.json",
            ".clangd",
            "Makefile",
            "CMakeLists.txt",
        ],
    );
    add(
        &mut m,
        "jdtls",
        "jdtls",
        &[],
        None,
        &["pom.xml", "build.gradle", "build.gradle.kts", ".project"],
    );
    // bash-language-server expects `languageId: "shellscript"`; the
    // `bash` language name wouldn't match.
    add(
        &mut m,
        "bash-language-server",
        "bash-language-server",
        &["start"],
        Some("shellscript"),
        &[],
    );
    add(
        &mut m,
        "vscode-json-language-server",
        "vscode-json-language-server",
        &["--stdio"],
        None,
        &[],
    );
    add(
        &mut m,
        "yaml-language-server",
        "yaml-language-server",
        &["--stdio"],
        None,
        &[],
    );
    add(
        &mut m,
        "marksman",
        "marksman",
        &["server"],
        None,
        &[".marksman.toml"],
    );
    add(
        &mut m,
        "vscode-html-language-server",
        "vscode-html-language-server",
        &["--stdio"],
        None,
        &[],
    );
    add(
        &mut m,
        "vscode-css-language-server",
        "vscode-css-language-server",
        &["--stdio"],
        None,
        &[],
    );
    add(
        &mut m,
        "lua-language-server",
        "lua-language-server",
        &[],
        None,
        &[".luarc.json", ".luarc.jsonc", "stylua.toml"],
    );
    add(
        &mut m,
        "ruby-lsp",
        "ruby-lsp",
        &[],
        None,
        &["Gemfile", ".rubocop.yml"],
    );
    add(&mut m, "zls", "zls", &[], None, &["build.zig"]);
    // HashiCorp's official Terraform language server; works for HCL
    // dialects too via the same protocol.
    add(
        &mut m,
        "terraform-ls",
        "terraform-ls",
        &["serve"],
        None,
        &[".terraform", "main.tf"],
    );
    // Vue / Svelte language servers — both are bundled with their
    // respective official tooling and stream-aware via `--stdio`.
    add(
        &mut m,
        "vue-language-server",
        "vue-language-server",
        &["--stdio"],
        None,
        &["package.json", "vite.config.ts", "vite.config.js"],
    );
    add(
        &mut m,
        "svelteserver",
        "svelteserver",
        &["--stdio"],
        None,
        &["package.json", "svelte.config.js", "svelte.config.ts"],
    );
    // HLS ships a `-wrapper` binary that picks the right GHC-matched
    // server for the project; users override to `haskell-language-server`
    // directly if they need to pin a version.
    add(
        &mut m,
        "haskell-language-server",
        "haskell-language-server-wrapper",
        &["--lsp"],
        None,
        &[
            "*.cabal",
            "stack.yaml",
            "cabal.project",
            "package.yaml",
            "hie.yaml",
        ],
    );
    // Elixir ships with both lexical and elixir-ls registered — the
    // first one found on PATH wins (`is_command_not_found` skip in the
    // LSP spawner). Lexical is preferred for speed; users on legacy
    // setups keep elixir-ls.
    add(
        &mut m,
        "lexical",
        "lexical",
        &["server"],
        None,
        &["mix.exs"],
    );
    add(&mut m, "elixir-ls", "elixir-ls", &[], None, &["mix.exs"]);
    // Nix gets both nixd (more diagnostics, slower) and nil (lighter,
    // faster) — same precedence trick as Elixir.
    add(
        &mut m,
        "nixd",
        "nixd",
        &[],
        None,
        &["flake.nix", "default.nix", "shell.nix"],
    );
    add(
        &mut m,
        "nil",
        "nil",
        &[],
        None,
        &["flake.nix", "default.nix", "shell.nix"],
    );
    // csharp-ls is a lighter alternative to OmniSharp/Roslyn — single
    // binary, no SDK detection step. Users who want full Roslyn should
    // override `[lsp.csharp-ls]` or add their own server entry.
    add(
        &mut m,
        "csharp-ls",
        "csharp-ls",
        &[],
        Some("csharp"),
        &["global.json", "Directory.Build.props"],
    );
    // SourceKit-LSP ships with the Swift toolchain.
    add(
        &mut m,
        "sourcekit-lsp",
        "sourcekit-lsp",
        &[],
        None,
        &["Package.swift"],
    );
    add(
        &mut m,
        "intelephense",
        "intelephense",
        &["--stdio"],
        None,
        &["composer.json", ".phpactor.json"],
    );
    // Dart's CLI hosts the language server; the client metadata flags
    // are required by `dart language-server` to start.
    add(
        &mut m,
        "dart",
        "dart",
        &[
            "language-server",
            "--client-id=vorto",
            "--client-version=0.11",
        ],
        None,
        &["pubspec.yaml"],
    );
    add(
        &mut m,
        "ocamllsp",
        "ocamllsp",
        &[],
        None,
        &["dune-project", "dune"],
    );
    // graphql-lsp (from graphql-language-service-cli) needs the `server`
    // subcommand and the `--method=stream` flag to speak LSP over stdio.
    add(
        &mut m,
        "graphql-lsp",
        "graphql-lsp",
        &["server", "--method=stream"],
        Some("graphql"),
        &[
            ".graphqlrc",
            ".graphqlrc.json",
            ".graphqlrc.yml",
            "graphql.config.js",
            "package.json",
        ],
    );
    add(&mut m, "fish-lsp", "fish-lsp", &["start"], None, &[]);
    m
}

/// Per-extension LSP `languageId` overrides. The LSP spec fixes the id
/// names (e.g. `.tsx` ↔ `"typescriptreact"`, `.jsx` ↔ `"javascriptreact"`),
/// but our internal language *names* don't have to match — `.tsx` is
/// routed through the `tsx` language so it picks up the JSX-aware
/// grammar and queries. This table is the bridge: extensions listed
/// here win over the language-name fallback at `didOpen` time.
/// Extensions not listed fall through to the language name, which is
/// already the right answer for `.ts` / `.py` / `.rs` / etc.
pub fn builtin_extension_language_ids() -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut add = |ext: &str, id: &str| {
        m.insert(ext.to_string(), id.to_string());
    };
    add("tsx", "typescriptreact");
    add("jsx", "javascriptreact");
    add("mjs", "javascript");
    add("cjs", "javascript");
    add("mts", "typescript");
    add("cts", "typescript");
    add("h", "c");
    add("hpp", "cpp");
    add("hh", "cpp");
    add("hxx", "cpp");
    add("htm", "html");
    add("mdx", "markdown");
    add("yml", "yaml");
    // C#'s LSP id is `csharp` (no dash) — our language name is `c-sharp`
    // to match the grammar's repo/symbol convention, so override per
    // extension here.
    add("cs", "csharp");
    m
}

/// Built-in `[languages.<name>]` defaults. To support a new language
/// out-of-the-box, add it here. Users can override every field via
/// `[languages.<name>]` in their config, and they can add entirely new
/// languages with the same syntax.
pub fn builtin_languages() -> HashMap<String, LanguageConfig> {
    let mut m = HashMap::new();
    let lsp = |names: &[&str]| Some(names.iter().map(|s| s.to_string()).collect());

    // rustfmt with no path argument reads stdin and writes stdout —
    // the shape `run_external_formatter` expects.
    m.insert(
        "rust".into(),
        LanguageConfig {
            extensions: Some(vec!["rs".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                ..Default::default()
            },
            lsp: lsp(&["rust-analyzer"]),
            formatter: Some(FormatterToml {
                command: Some("rustfmt".into()),
                args: None,
            }),
            ..Default::default()
        },
    );
    m.insert(
        "python".into(),
        LanguageConfig {
            extensions: Some(vec!["py".into()]),
            comment_token: Some("#".into()),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                ..Default::default()
            },
            lsp: lsp(&["pyright"]),
            ..Default::default()
        },
    );
    m.insert(
        "toml".into(),
        LanguageConfig {
            extensions: Some(vec!["toml".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["taplo"]),
            ..Default::default()
        },
    );
    // TypeScript ships with both vtsls and typescript-language-server
    // — whichever is installed will spawn, the other is silently
    // skipped (`is_command_not_found`). Users who want only one can
    // re-declare `lsp = [...]` in their config.
    m.insert(
        "typescript".into(),
        LanguageConfig {
            extensions: Some(vec!["ts".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["vtsls", "typescript-language-server"]),
            ..Default::default()
        },
    );
    // `.tsx` gets its own language entry (grammar `tsx`, query dir
    // `tsx/`) so JSX-aware indents.scm / highlights.scm fire — the
    // plain `typescript` grammar doesn't parse JSX nodes.
    m.insert(
        "tsx".into(),
        LanguageConfig {
            extensions: Some(vec!["tsx".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["vtsls", "typescript-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "javascript".into(),
        LanguageConfig {
            extensions: Some(vec!["js".into(), "jsx".into(), "mjs".into(), "cjs".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["typescript-language-server"]),
            ..Default::default()
        },
    );
    // Go is canonically tab-indented (gofmt enforces it).
    m.insert(
        "go".into(),
        LanguageConfig {
            extensions: Some(vec!["go".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                use_tabs: Some(true),
                ..Default::default()
            },
            lsp: lsp(&["gopls"]),
            formatter: Some(FormatterToml {
                command: Some("gofmt".into()),
                args: None,
            }),
            ..Default::default()
        },
    );
    m.insert(
        "kotlin".into(),
        LanguageConfig {
            extensions: Some(vec!["kt".into(), "kts".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["kotlin-lsp"]),
            ..Default::default()
        },
    );
    // `.h` is ambiguous (C or C++); routed to C by default. C++-specific
    // headers (`.hpp`, `.hh`, `.hxx`) go to C++.
    m.insert(
        "c".into(),
        LanguageConfig {
            extensions: Some(vec!["c".into(), "h".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["clangd"]),
            ..Default::default()
        },
    );
    m.insert(
        "cpp".into(),
        LanguageConfig {
            extensions: Some(vec![
                "cpp".into(),
                "cc".into(),
                "cxx".into(),
                "hpp".into(),
                "hh".into(),
                "hxx".into(),
            ]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["clangd"]),
            ..Default::default()
        },
    );
    m.insert(
        "java".into(),
        LanguageConfig {
            extensions: Some(vec!["java".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["jdtls"]),
            ..Default::default()
        },
    );
    m.insert(
        "bash".into(),
        LanguageConfig {
            extensions: Some(vec!["sh".into(), "bash".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["bash-language-server"]),
            ..Default::default()
        },
    );
    // JSON has no native single-line comment; leaving `comment_token`
    // unset disables the `<space>c` toggle (correct).
    m.insert(
        "json".into(),
        LanguageConfig {
            extensions: Some(vec!["json".into()]),
            comment_token: None,
            lsp: lsp(&["vscode-json-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "yaml".into(),
        LanguageConfig {
            extensions: Some(vec!["yaml".into(), "yml".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["yaml-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "markdown".into(),
        LanguageConfig {
            extensions: Some(vec!["md".into(), "markdown".into()]),
            comment_token: None,
            lsp: lsp(&["marksman"]),
            ..Default::default()
        },
    );
    m.insert(
        "html".into(),
        LanguageConfig {
            extensions: Some(vec!["html".into(), "htm".into()]),
            comment_token: None,
            block_comment_token: Some(("<!--".into(), "-->".into())),
            lsp: lsp(&["vscode-html-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "css".into(),
        LanguageConfig {
            extensions: Some(vec!["css".into()]),
            comment_token: None,
            block_comment_token: Some(("/*".into(), "*/".into())),
            lsp: lsp(&["vscode-css-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "lua".into(),
        LanguageConfig {
            extensions: Some(vec!["lua".into()]),
            comment_token: Some("--".into()),
            lsp: lsp(&["lua-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "ruby".into(),
        LanguageConfig {
            extensions: Some(vec!["rb".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["ruby-lsp"]),
            ..Default::default()
        },
    );
    m.insert(
        "sql".into(),
        LanguageConfig {
            extensions: Some(vec!["sql".into()]),
            comment_token: Some("--".into()),
            ..Default::default()
        },
    );
    m.insert(
        "zig".into(),
        LanguageConfig {
            extensions: Some(vec!["zig".into(), "zon".into()]),
            comment_token: Some("//".into()),
            lsp: lsp(&["zls"]),
            formatter: Some(FormatterToml {
                command: Some("zig".into()),
                args: Some(vec!["fmt".into(), "--stdin".into()]),
            }),
            ..Default::default()
        },
    );
    // Dockerfile is usually a bare filename; `.dockerfile` (and the
    // Podman-flavored `Containerfile`) also exist in the wild.
    m.insert(
        "dockerfile".into(),
        LanguageConfig {
            extensions: Some(vec!["dockerfile".into()]),
            filenames: Some(vec!["Dockerfile".into(), "Containerfile".into()]),
            comment_token: Some("#".into()),
            ..Default::default()
        },
    );
    // GNU Make recognizes `Makefile`, `makefile`, and `GNUmakefile`
    // out of the box; `.mk` / `.make` are common for included
    // fragments. Tab-indented recipes are load-bearing, so use_tabs is
    // forced on.
    m.insert(
        "make".into(),
        LanguageConfig {
            extensions: Some(vec!["mk".into(), "make".into()]),
            filenames: Some(vec![
                "Makefile".into(),
                "makefile".into(),
                "GNUmakefile".into(),
            ]),
            comment_token: Some("#".into()),
            editor: EditorToml {
                use_tabs: Some(true),
                tab_width: Some(8),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    // HCL covers Terraform (`.tf`, `.tfvars`) and plain HCL configs
    // (`.hcl`). All three share the same grammar.
    m.insert(
        "hcl".into(),
        LanguageConfig {
            extensions: Some(vec!["hcl".into(), "tf".into(), "tfvars".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["terraform-ls"]),
            ..Default::default()
        },
    );
    // Diff/patch files have no comment syntax — leaving the toggle
    // unset disables `<space>c` for them, which is correct.
    m.insert(
        "diff".into(),
        LanguageConfig {
            extensions: Some(vec!["diff".into(), "patch".into()]),
            comment_token: None,
            ..Default::default()
        },
    );
    // Vue / Svelte single-file components. Comment token is the HTML
    // form since the outermost layer is template; users editing inside
    // a `<script>` block will get the wrong comment briefly until
    // injection is wired up.
    m.insert(
        "vue".into(),
        LanguageConfig {
            extensions: Some(vec!["vue".into()]),
            comment_token: Some("<!--".into()),
            block_comment_token: Some(("<!--".into(), "-->".into())),
            lsp: lsp(&["vue-language-server"]),
            ..Default::default()
        },
    );
    m.insert(
        "svelte".into(),
        LanguageConfig {
            extensions: Some(vec!["svelte".into()]),
            comment_token: Some("<!--".into()),
            block_comment_token: Some(("<!--".into(), "-->".into())),
            lsp: lsp(&["svelteserver"]),
            ..Default::default()
        },
    );
    // `.lhs` (Literate Haskell) shares the entry; the haskell grammar
    // doesn't parse the bird-track wrapping, so highlights inside `>`
    // blocks degrade gracefully to plain text.
    m.insert(
        "haskell".into(),
        LanguageConfig {
            extensions: Some(vec!["hs".into(), "lhs".into()]),
            comment_token: Some("--".into()),
            block_comment_token: Some(("{-".into(), "-}".into())),
            lsp: lsp(&["haskell-language-server"]),
            ..Default::default()
        },
    );
    // Elixir: `.ex` for compiled modules, `.exs` for scripts (incl.
    // `mix.exs`). Lexical preferred, elixir-ls fallback.
    m.insert(
        "elixir".into(),
        LanguageConfig {
            extensions: Some(vec!["ex".into(), "exs".into()]),
            filenames: Some(vec!["mix.lock".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["lexical", "elixir-ls"]),
            ..Default::default()
        },
    );
    // Nix uses 2-space indent by convention (nixpkgs / nixfmt agree).
    m.insert(
        "nix".into(),
        LanguageConfig {
            extensions: Some(vec!["nix".into()]),
            comment_token: Some("#".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["nixd", "nil"]),
            ..Default::default()
        },
    );
    // C#: dotnet convention is 4-space indent. Language name uses a
    // dash to keep parity with the grammar (`tree-sitter-c-sharp`); the
    // LSP id `csharp` is set both per-extension and on the LSP entry
    // so it stays right regardless of route.
    m.insert(
        "c-sharp".into(),
        LanguageConfig {
            extensions: Some(vec!["cs".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                ..Default::default()
            },
            lsp: lsp(&["csharp-ls"]),
            ..Default::default()
        },
    );
    m.insert(
        "swift".into(),
        LanguageConfig {
            extensions: Some(vec!["swift".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                ..Default::default()
            },
            lsp: lsp(&["sourcekit-lsp"]),
            ..Default::default()
        },
    );
    // PHP: `<?php` blocks plus surrounding HTML are both parsed by the
    // `php/` subgrammar. PSR-12 mandates 4-space indent.
    m.insert(
        "php".into(),
        LanguageConfig {
            extensions: Some(vec!["php".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(4),
                tab_width: Some(4),
                ..Default::default()
            },
            lsp: lsp(&["intelephense"]),
            ..Default::default()
        },
    );
    // Dart / Flutter convention is 2-space indent (`dart format`
    // enforces it).
    m.insert(
        "dart".into(),
        LanguageConfig {
            extensions: Some(vec!["dart".into()]),
            comment_token: Some("//".into()),
            block_comment_token: Some(("/*".into(), "*/".into())),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["dart"]),
            formatter: Some(FormatterToml {
                command: Some("dart".into()),
                args: Some(vec!["format".into(), "--output=show".into()]),
            }),
            ..Default::default()
        },
    );
    // OCaml: `.ml` implementation, `.mli` interface — both share the
    // single `ocaml` grammar / queries. No single-line comment syntax,
    // so the `<space>c` toggle uses the block form via
    // `block_comment_token`. ocamlformat is the canonical formatter.
    m.insert(
        "ocaml".into(),
        LanguageConfig {
            extensions: Some(vec!["ml".into(), "mli".into()]),
            comment_token: None,
            block_comment_token: Some(("(*".into(), "*)".into())),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["ocamllsp"]),
            formatter: Some(FormatterToml {
                command: Some("ocamlformat".into()),
                args: Some(vec![
                    "--enable-outside-detected-project".into(),
                    "--name".into(),
                    "stdin.ml".into(),
                    "-".into(),
                ]),
            }),
            ..Default::default()
        },
    );
    // GraphQL uses 2-space indent in every common style guide; `#` is
    // the only comment syntax (no block form).
    m.insert(
        "graphql".into(),
        LanguageConfig {
            extensions: Some(vec!["graphql".into(), "gql".into()]),
            comment_token: Some("#".into()),
            editor: EditorToml {
                indent_width: Some(2),
                tab_width: Some(2),
                ..Default::default()
            },
            lsp: lsp(&["graphql-lsp"]),
            ..Default::default()
        },
    );
    // Fish shell scripts. `fish_indent` is the canonical formatter,
    // shipped with fish itself.
    m.insert(
        "fish".into(),
        LanguageConfig {
            extensions: Some(vec!["fish".into()]),
            comment_token: Some("#".into()),
            lsp: lsp(&["fish-lsp"]),
            formatter: Some(FormatterToml {
                command: Some("fish_indent".into()),
                args: None,
            }),
            ..Default::default()
        },
    );
    m
}
