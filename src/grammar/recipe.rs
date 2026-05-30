//! Built-in catalog of tree-sitter grammar repos.
//!
//! A "recipe" is the minimum info needed to fetch a grammar's source and
//! point `tree-sitter build` at it: the git repo URL, an optional
//! subdirectory for monorepos (e.g. tree-sitter-typescript holds both
//! `typescript/` and `tsx/`), and an optional pinned revision.
//!
//! Adding a new built-in language is a one-line addition to
//! [`builtin_recipes`]. Users who want a grammar that's not built in can
//! define their own recipe in config under `[grammars.<name>]` (see
//! [`GrammarRecipe::from_config`]) — that makes `vorto grammar install
//! <name>` work for it too. Queries for such grammars are user-supplied
//! (we only bundle queries for built-ins).

/// Static description of how to fetch and build one grammar.
#[derive(Debug, Clone)]
pub struct GrammarRecipe {
    /// Logical name — the filename stem the loader looks for
    /// (`<name>.{so,dylib,dll}`) and the symbol root
    /// (`tree_sitter_<name>`).
    pub name: &'static str,
    /// Git URL to clone.
    pub repo: &'static str,
    /// Optional subdirectory inside the cloned repo to build from. Used
    /// for monorepos like tree-sitter-typescript that ship multiple
    /// grammars side-by-side.
    pub subpath: Option<&'static str>,
    /// Optional pinned git revision (tag, branch, or commit). When
    /// `None`, the default branch is shallow-cloned. When `Some`, a full
    /// clone is performed and the rev checked out.
    pub rev: Option<&'static str>,
}

/// The built-in catalog. Names here line up with the language entries in
/// [`crate::config::languages::builtin_languages`] so `vorto grammar
/// install <lang>` "just works" for the languages that ship out of the
/// box.
pub fn builtin_recipes() -> Vec<GrammarRecipe> {
    vec![
        GrammarRecipe {
            name: "rust",
            repo: "https://github.com/tree-sitter/tree-sitter-rust",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "python",
            repo: "https://github.com/tree-sitter/tree-sitter-python",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "go",
            repo: "https://github.com/tree-sitter/tree-sitter-go",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "javascript",
            repo: "https://github.com/tree-sitter/tree-sitter-javascript",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "typescript",
            repo: "https://github.com/tree-sitter/tree-sitter-typescript",
            subpath: Some("typescript"),
            rev: None,
        },
        GrammarRecipe {
            name: "tsx",
            repo: "https://github.com/tree-sitter/tree-sitter-typescript",
            subpath: Some("tsx"),
            rev: None,
        },
        GrammarRecipe {
            name: "toml",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-toml",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "kotlin",
            repo: "https://github.com/fwcd/tree-sitter-kotlin",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "c",
            repo: "https://github.com/tree-sitter/tree-sitter-c",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "cpp",
            repo: "https://github.com/tree-sitter/tree-sitter-cpp",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "java",
            repo: "https://github.com/tree-sitter/tree-sitter-java",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "bash",
            repo: "https://github.com/tree-sitter/tree-sitter-bash",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "json",
            repo: "https://github.com/tree-sitter/tree-sitter-json",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "yaml",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-yaml",
            subpath: None,
            rev: None,
        },
        // `tree-sitter-grammars/tree-sitter-markdown` ships two grammars in
        // one repo: a block-level one (`tree-sitter-markdown/`) and an
        // inline one (`tree-sitter-markdown-inline/`) intended to be used
        // via injection. We install only the block grammar here — the
        // inline grammar requires editor-side injection plumbing that
        // doesn't exist yet, and installing it standalone would just be
        // dead weight.
        GrammarRecipe {
            name: "markdown",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-markdown",
            subpath: Some("tree-sitter-markdown"),
            rev: None,
        },
        GrammarRecipe {
            name: "html",
            repo: "https://github.com/tree-sitter/tree-sitter-html",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "css",
            repo: "https://github.com/tree-sitter/tree-sitter-css",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "lua",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-lua",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "ruby",
            repo: "https://github.com/tree-sitter/tree-sitter-ruby",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "zig",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-zig",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "sql",
            repo: "https://github.com/DerekStride/tree-sitter-sql",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "dockerfile",
            repo: "https://github.com/camdencheek/tree-sitter-dockerfile",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "make",
            repo: "https://github.com/alemuller/tree-sitter-make",
            subpath: None,
            rev: None,
        },
        // `tree-sitter-hcl` covers both Terraform (`.tf`) and plain
        // HCL (`.hcl`, `.tfvars`); we route all three through the
        // single `hcl` language entry.
        GrammarRecipe {
            name: "hcl",
            repo: "https://github.com/MichaHoffmann/tree-sitter-hcl",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "diff",
            repo: "https://github.com/the-mikedavis/tree-sitter-diff",
            subpath: None,
            rev: None,
        },
        // Vue and Svelte single-file components. The template layer
        // (tags, directives, attributes) highlights via these grammars
        // directly; embedded `<script>` / `<style>` blocks render as
        // plain text until language injection is wired up.
        GrammarRecipe {
            name: "vue",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-vue",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "svelte",
            repo: "https://github.com/Himujjal/tree-sitter-svelte",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "haskell",
            repo: "https://github.com/tree-sitter/tree-sitter-haskell",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "elixir",
            repo: "https://github.com/elixir-lang/tree-sitter-elixir",
            subpath: None,
            rev: None,
        },
        GrammarRecipe {
            name: "nix",
            repo: "https://github.com/nix-community/tree-sitter-nix",
            subpath: None,
            rev: None,
        },
        // For the seven below, revisions are pinned to match
        // nvim-treesitter's lockfile at the same commit we vendored
        // queries from (`cf12346…`). Leaving these unpinned drifts the
        // grammar AST away from the captured node names — eg. the Swift
        // grammar at HEAD no longer exposes `"#available"` as a literal
        // token, which makes our `highlights.scm` fail to compile.

        // tree-sitter-c-sharp's library symbol is `tree_sitter_c_sharp`,
        // which the loader derives by replacing `-` with `_` in the
        // grammar name — so the `c-sharp` recipe name lines up.
        GrammarRecipe {
            name: "c-sharp",
            repo: "https://github.com/tree-sitter/tree-sitter-c-sharp",
            subpath: None,
            rev: Some("b5eb5742f6a7e9438bee22ce8026d6b927be2cd7"),
        },
        GrammarRecipe {
            name: "swift",
            repo: "https://github.com/alex-pinkus/tree-sitter-swift",
            subpath: None,
            rev: Some("aca5a52aa3cab858944d3c02701ccf5b2d8fd0f9"),
        },
        // tree-sitter-php is a monorepo; the `php/` subdir is the full
        // grammar (parses both `<?php` blocks and surrounding HTML),
        // which is what most PHP files in the wild are.
        GrammarRecipe {
            name: "php",
            repo: "https://github.com/tree-sitter/tree-sitter-php",
            subpath: Some("php"),
            rev: Some("576a56fa7f8b68c91524cdd211eb2ffc43e7bb11"),
        },
        GrammarRecipe {
            name: "dart",
            repo: "https://github.com/UserNobody14/tree-sitter-dart",
            subpath: None,
            rev: Some("80e23c07b64494f7e21090bb3450223ef0b192f4"),
        },
        // tree-sitter-ocaml is a monorepo; `grammars/ocaml/` is the
        // implementation grammar (covers `.ml`). `.mli` interface files
        // share the entry — the grammar tolerates them well enough that
        // a separate `ocaml-interface` grammar isn't worth the size.
        GrammarRecipe {
            name: "ocaml",
            repo: "https://github.com/tree-sitter/tree-sitter-ocaml",
            subpath: Some("grammars/ocaml"),
            rev: Some("91708deb10cb4fe68ab3c50891426b9967dbf35a"),
        },
        GrammarRecipe {
            name: "graphql",
            repo: "https://github.com/bkegley/tree-sitter-graphql",
            subpath: None,
            rev: Some("5e66e961eee421786bdda8495ed1db045e06b5fe"),
        },
        GrammarRecipe {
            name: "fish",
            repo: "https://github.com/ram02z/tree-sitter-fish",
            subpath: None,
            rev: Some("70640c0696abde32622afc43291a385681afbd32"),
        },
        GrammarRecipe {
            name: "julia",
            repo: "https://github.com/tree-sitter/tree-sitter-julia",
            subpath: None,
            rev: Some("12a3aede757bc7fbdfb1909507c7a6fddd31df37"),
        },
        // The six below are likewise pinned to nvim-treesitter's lockfile
        // at `cf12346…` (the commit we vendored their queries from), same
        // rationale as the block above.
        GrammarRecipe {
            name: "gleam",
            repo: "https://github.com/gleam-lang/tree-sitter-gleam",
            subpath: None,
            rev: Some("99ec4101504452c488b7c835fb65cfef75b090b7"),
        },
        GrammarRecipe {
            name: "scala",
            repo: "https://github.com/tree-sitter/tree-sitter-scala",
            subpath: None,
            rev: Some("c1189954df854977c3a52003ca8a247c5f4729ba"),
        },
        GrammarRecipe {
            name: "odin",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-odin",
            subpath: None,
            rev: Some("d2ca8efb4487e156a60d5bd6db2598b872629403"),
        },
        GrammarRecipe {
            name: "proto",
            repo: "https://github.com/treywood/tree-sitter-proto",
            subpath: None,
            rev: Some("e9f6b43f6844bd2189b50a422d4e2094313f6aa3"),
        },
        GrammarRecipe {
            name: "cmake",
            repo: "https://github.com/uyha/tree-sitter-cmake",
            subpath: None,
            rev: Some("fe48221d4d9842d916d66b5e71ab3c6307ec28b3"),
        },
        // tree-sitter-xml is a monorepo; `xml/` is the XML grammar (the
        // sibling `dtd/` covers DTDs, which we don't route yet). The
        // built library's symbol is `tree_sitter_xml`, matching the name.
        GrammarRecipe {
            name: "xml",
            repo: "https://github.com/tree-sitter-grammars/tree-sitter-xml",
            subpath: Some("xml"),
            rev: Some("0d9a8099c963ed53e183425c1b47fa2622c8eaf7"),
        },
    ]
}

impl GrammarRecipe {
    /// Build a recipe from a user's `[grammars.<name>]` config entry.
    ///
    /// The owned strings are leaked to `&'static str`. That's deliberate:
    /// recipes are resolved once at startup and live for the whole
    /// process (the `grammar` subcommand is short-lived, and the editor
    /// keeps its config for its entire run), so a handful of never-freed
    /// small strings costs nothing and lets user recipes share the exact
    /// same type as the built-ins.
    pub fn from_config(name: &str, source: &str, rev: Option<&str>, subpath: Option<&str>) -> Self {
        fn leak(s: &str) -> &'static str {
            Box::leak(s.to_owned().into_boxed_str())
        }
        GrammarRecipe {
            name: leak(name),
            repo: leak(source),
            subpath: subpath.map(leak),
            rev: rev.map(leak),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(name: &str) -> Option<GrammarRecipe> {
        builtin_recipes().into_iter().find(|r| r.name == name)
    }

    #[test]
    fn builtin_catalog_contains_rust() {
        assert!(find("rust").is_some());
    }

    #[test]
    fn typescript_and_tsx_share_repo_with_subpaths() {
        let ts = find("typescript").unwrap();
        let tsx = find("tsx").unwrap();
        assert_eq!(ts.repo, tsx.repo);
        assert_eq!(ts.subpath, Some("typescript"));
        assert_eq!(tsx.subpath, Some("tsx"));
    }

    #[test]
    fn unknown_recipe_is_none() {
        assert!(find("does-not-exist").is_none());
    }

    #[test]
    fn from_config_builds_owned_recipe() {
        let r = GrammarRecipe::from_config(
            "foolang",
            "https://example.com/tree-sitter-foolang",
            Some("abc123"),
            Some("sub"),
        );
        assert_eq!(r.name, "foolang");
        assert_eq!(r.repo, "https://example.com/tree-sitter-foolang");
        assert_eq!(r.rev, Some("abc123"));
        assert_eq!(r.subpath, Some("sub"));
    }
}
