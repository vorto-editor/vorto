//! `vorto grammar …` subcommand dispatcher.
//!
//! Three operations:
//!
//! * `list` — print every built-in recipe with installed/missing status.
//! * `install <name>…` (or `--all`) — fetch, build, and place the
//!   `.so`/`.dylib`/`.dll` into the configured `grammar_dir`.
//! * `remove <name>…` — delete the installed library.
//!
//! The grammar directory is read from the same `Config::load` path the
//! editor uses, so anything installed here is immediately picked up next
//! time the editor starts.
//!
//! The recipe catalog is the built-in list plus any user
//! `[grammars.<name>]` entries from config (a user entry overrides a
//! built-in of the same name). User-defined grammars install just like
//! built-ins, except their queries are user-supplied — only built-ins
//! carry bundled `.scm` files.

use std::path::Path;

use anyhow::{Result, bail};

use crate::config::{self, Config, GrammarSource, LanguageRegistry};

use super::assets;
use super::build;
use super::recipe::{GrammarRecipe, builtin_recipes};

/// Entry point invoked from `main` when `argv[1] == "grammar"`. `args`
/// is everything after the `grammar` token.
pub fn run(args: &[String]) -> Result<()> {
    let cfg = Config::load(config::default_path().as_deref())?;
    let grammar_dir = cfg.grammar_dir.as_path();
    let query_dir = cfg.query_dir.as_path();
    // Built-in catalog plus any user `[grammars.*]` recipes (a user
    // entry whose name matches a built-in overrides it).
    let recipes = merged_recipes(&cfg.grammars);

    match args.split_first() {
        None => {
            print_usage();
            Ok(())
        }
        Some((cmd, rest)) => match cmd.as_str() {
            "list" | "ls" => list(rest, &recipes, &cfg.languages, grammar_dir, query_dir),
            "install" | "add" => install(rest, &recipes, grammar_dir, query_dir),
            "install-queries" | "refresh-queries" => install_queries(rest, &recipes, query_dir),
            "remove" | "rm" | "uninstall" => remove(rest, grammar_dir),
            "help" | "-h" | "--help" => {
                print_usage();
                Ok(())
            }
            other => {
                print_usage();
                bail!("unknown grammar subcommand: `{}`", other);
            }
        },
    }
}

/// Built-in recipes overlaid with the user's `[grammars.*]` entries.
/// A user entry with the same name as a built-in replaces it (so users
/// can repoint a grammar at a fork or newer revision); new names are
/// appended.
///
/// Public so the in-editor `:grammar` command can build the same
/// config-aware catalog the CLI uses.
pub fn merged_recipes(user: &[GrammarSource]) -> Vec<GrammarRecipe> {
    let mut recipes = builtin_recipes();
    for g in user {
        let recipe =
            GrammarRecipe::from_config(&g.name, &g.source, g.rev.as_deref(), g.subpath.as_deref());
        match recipes.iter_mut().find(|r| r.name == g.name) {
            Some(slot) => *slot = recipe,
            None => recipes.push(recipe),
        }
    }
    recipes
}

fn find<'a>(recipes: &'a [GrammarRecipe], name: &str) -> Option<&'a GrammarRecipe> {
    recipes.iter().find(|r| r.name == name)
}

fn print_usage() {
    eprintln!("usage: vorto grammar <command> [args]");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  list [-v]                         show install status (grouped; -v for detail)");
    eprintln!("  install <name>... | --all         build and install one or more grammars");
    eprintln!("  install-queries <name>... | --all overwrite installed .scm files from the");
    eprintln!("                                    vendored bundle (no library rebuild)");
    eprintln!("  remove <name>...                  delete installed grammar libraries");
    eprintln!();
    eprintln!("examples:");
    eprintln!("  vorto grammar install rust python");
    eprintln!("  vorto grammar install --all");
    eprintln!("  vorto grammar install-queries python");
    eprintln!("  vorto grammar list");
    eprintln!();
    eprintln!("define non-built-in grammars in config under [grammars.<name>]:");
    eprintln!("  [grammars.nim]");
    eprintln!("  source  = \"https://github.com/alaviss/tree-sitter-nim\"");
    eprintln!("  rev     = \"abc123\"   # optional; omit to build the latest default branch");
    eprintln!("  subpath = \"sub\"      # optional; for monorepos");
}

fn list(
    args: &[String],
    recipes: &[GrammarRecipe],
    languages: &LanguageRegistry,
    grammar_dir: &Path,
    query_dir: &Path,
) -> Result<()> {
    if args.iter().any(|a| a == "-v" || a == "--verbose") {
        return list_verbose(recipes, languages, grammar_dir, query_dir);
    }

    println!("grammar dir: {}", grammar_dir.display());
    println!("query dir:   {}", query_dir.display());
    println!();

    // Three states: a grammar is "installed" once its library plus every
    // bundled query is on disk; "lib only" means the library built but
    // some queries are missing (rare — usually manual tinkering); the
    // rest are "missing". Empty groups are skipped so the common case is
    // just installed/missing.
    let (mut installed, mut lib_only, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for r in recipes {
        if build::installed_path(r.name, grammar_dir).is_none() {
            missing.push(r.name);
        } else if build::is_fully_installed(r.name, grammar_dir, query_dir) {
            installed.push(r.name);
        } else {
            lib_only.push(r.name);
        }
    }
    let total = installed.len() + lib_only.len() + missing.len();

    print_group("installed", &installed);
    print_group("lib only, queries missing", &lib_only);
    print_group("missing", &missing);

    println!(
        "{} total · {} installed · {} missing",
        total,
        installed.len() + lib_only.len(),
        missing.len()
    );

    // Grammars referenced by `[languages.*]` config that have no recipe
    // at all (built-in or user `[grammars.*]`). These can't be installed
    // via `grammar install` — the user supplies the `.so` — so they get
    // their own section with per-entry status.
    let custom = custom_grammars(recipes, languages, grammar_dir);
    if !custom.is_empty() {
        println!();
        println!("custom (from config) ({}):", custom.len());
        for c in &custom {
            let glyph = if c.installed { "✓" } else { "✗" };
            println!("  {} {}", glyph, c.grammar);
        }
        if custom.iter().any(|c| !c.installed) {
            println!(
                "  (✗ = no library found and no recipe — add a `[grammars.<name>]` entry to install it, or drop the compiled `.so` in yourself)"
            );
        }
    }

    println!("(-v for repo URLs and per-grammar query detail)");
    Ok(())
}

/// A grammar referenced by a `[languages.*]` config entry that has no
/// recipe at all — the user is responsible for supplying its library.
struct CustomGrammar {
    grammar: String,
    installed: bool,
}

/// Collect grammars referenced by config languages that have no recipe
/// (neither built-in nor user `[grammars.*]`), deduped by grammar stem
/// and sorted by name. Each language may override the global grammar
/// dir, so install status is checked against the language's effective
/// dir.
fn custom_grammars(
    recipes: &[GrammarRecipe],
    languages: &LanguageRegistry,
    grammar_dir: &Path,
) -> Vec<CustomGrammar> {
    use std::collections::HashSet;
    let has_recipe: HashSet<&str> = recipes.iter().map(|r| r.name).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for lang in languages.iter() {
        if has_recipe.contains(lang.grammar.as_str()) || !seen.insert(lang.grammar.clone()) {
            continue;
        }
        let dir = lang.grammar_dir.as_deref().unwrap_or(grammar_dir);
        out.push(CustomGrammar {
            grammar: lang.grammar.clone(),
            installed: build::installed_path(&lang.grammar, dir).is_some(),
        });
    }
    out.sort_by(|a, b| a.grammar.cmp(&b.grammar));
    out
}

/// Print `<label> (<n>):` followed by the names laid out in aligned,
/// terminal-width-aware columns. No-op for an empty group.
fn print_group(label: &str, names: &[&str]) {
    if names.is_empty() {
        return;
    }
    println!("{} ({}):", label, names.len());

    const INDENT: usize = 2;
    let term_w = crossterm::terminal::size()
        .map(|(c, _)| c as usize)
        .unwrap_or(80);
    let col_w = names.iter().map(|n| n.len()).max().unwrap_or(1) + 2;
    let cols = (term_w.saturating_sub(INDENT) / col_w).max(1);

    for row in names.chunks(cols) {
        let mut line = " ".repeat(INDENT);
        for name in row {
            line.push_str(&format!("{:<width$}", name, width = col_w));
        }
        println!("{}", line.trim_end());
    }
    println!();
}

/// The detailed, one-block-per-grammar view (repo URL, subpath, library
/// and query status). Reachable via `list -v`.
fn list_verbose(
    recipes: &[GrammarRecipe],
    languages: &LanguageRegistry,
    grammar_dir: &Path,
    query_dir: &Path,
) -> Result<()> {
    println!("grammar dir: {}", grammar_dir.display());
    println!("query dir:   {}", query_dir.display());
    println!();
    for r in recipes {
        let lib_status = match build::installed_path(r.name, grammar_dir) {
            Some(_) => "lib ✓",
            None => "lib ✗",
        };
        let installed = build::installed_queries(r.name, query_dir);
        let bundled = assets::bundled_query_names(r.name);
        let query_status = match (installed.is_empty(), bundled.is_empty()) {
            (false, _) => format!("queries: {} (installed)", installed.join(",")),
            (true, false) => format!("queries: {} (bundled, not installed)", bundled.join(",")),
            (true, true) => "queries: none bundled".to_string(),
        };
        let subpath = r.subpath.map(|s| format!(" [{}]", s)).unwrap_or_default();
        println!(
            "  {:<12} {}{}\n               {} | {}",
            r.name, r.repo, subpath, lib_status, query_status
        );
    }

    let custom = custom_grammars(recipes, languages, grammar_dir);
    if !custom.is_empty() {
        println!();
        println!("custom (from config):");
        for c in &custom {
            let lib_status = if c.installed { "lib ✓" } else { "lib ✗" };
            println!(
                "  {:<12} (no recipe — user-supplied)\n               {}",
                c.grammar, lib_status
            );
        }
    }
    Ok(())
}

fn install(
    args: &[String],
    recipes: &[GrammarRecipe],
    grammar_dir: &Path,
    query_dir: &Path,
) -> Result<()> {
    let recipes = match args.first().map(String::as_str) {
        None => {
            bail!("install: need at least one grammar name (or `--all`)");
        }
        Some("--all") => recipes.to_vec(),
        _ => {
            let mut out = Vec::new();
            for name in args {
                match find(recipes, name) {
                    Some(r) => out.push(r.clone()),
                    None => bail!(
                        "unknown grammar `{}`. Run `vorto grammar list` for the catalog, or define it under `[grammars.{}]` in your config.",
                        name,
                        name
                    ),
                }
            }
            out
        }
    };

    // Skip already-installed up front so the parallel worker pool
    // only sees real work. Sequential and noted inline so users see
    // the skip reason next to the name.
    let mut pending = Vec::new();
    for r in &recipes {
        if build::is_fully_installed(r.name, grammar_dir, query_dir) {
            eprintln!("==> {} already installed, skipping", r.name);
        } else {
            pending.push(r.clone());
        }
    }
    if pending.is_empty() {
        return Ok(());
    }

    // Worker pool: cap at CPU count and the job count. The per-job
    // work is mostly cc-bound (the parser is a single ~600k-line
    // translation unit), so more workers than physical cores just
    // thrashes the scheduler. Stderr is locked per-recipe so each
    // grammar's "==> installing"/"lib"/"queries" lines stay grouped.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(pending.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let stderr_lock = std::sync::Mutex::new(());
    let failures = std::sync::Mutex::new(Vec::<&str>::new());

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= pending.len() {
                        break;
                    }
                    let r = &pending[i];
                    let result = build::install(r, grammar_dir, query_dir);
                    let mut buf = String::new();
                    use std::fmt::Write;
                    writeln!(buf, "==> installing {} ({})", r.name, r.repo).ok();
                    match result {
                        Ok(report) => {
                            writeln!(buf, "    lib: {}", report.library.display()).ok();
                            if report.queries.is_empty() {
                                writeln!(
                                    buf,
                                    "    queries: none bundled — drop your own .scm into {}/{}/",
                                    query_dir.display(),
                                    r.name
                                )
                                .ok();
                            } else {
                                let names: Vec<String> = report
                                    .queries
                                    .iter()
                                    .filter_map(|p| {
                                        p.file_name().map(|n| n.to_string_lossy().into_owned())
                                    })
                                    .collect();
                                writeln!(
                                    buf,
                                    "    queries: {} ({} files)",
                                    names.join(", "),
                                    report.queries.len()
                                )
                                .ok();
                            }
                        }
                        Err(e) => {
                            writeln!(buf, "    failed: {:#}", e).ok();
                            failures.lock().unwrap().push(r.name);
                        }
                    }
                    let _g = stderr_lock.lock().unwrap();
                    eprint!("{}", buf);
                }
            });
        }
    });

    let failures = failures.into_inner().unwrap();
    if !failures.is_empty() {
        bail!("failed to install: {}", failures.join(", "));
    }
    Ok(())
}

/// Queries-only refresh. Rewrites `<query_dir>/<name>/*.scm` from the
/// compile-time-embedded bundle for each named grammar (or every
/// built-in recipe under `--all`), without touching the loaded `.so`.
/// `install` skips when a grammar is already fully installed, so this
/// is the way to pick up an in-repo edit to `assets/queries/*.scm`
/// without uninstalling and rebuilding the grammar.
fn install_queries(args: &[String], recipes: &[GrammarRecipe], query_dir: &Path) -> Result<()> {
    let recipes = match args.first().map(String::as_str) {
        None => {
            bail!("install-queries: need at least one grammar name (or `--all`)");
        }
        Some("--all") => recipes.to_vec(),
        _ => {
            let mut out = Vec::new();
            for name in args {
                match find(recipes, name) {
                    Some(r) => out.push(r.clone()),
                    None => bail!(
                        "unknown grammar `{}`. Run `vorto grammar list` for the catalog.",
                        name
                    ),
                }
            }
            out
        }
    };

    let mut failures = Vec::new();
    for r in &recipes {
        eprintln!("==> refreshing queries for {}", r.name);
        match build::write_vendored_queries(query_dir, r.name) {
            Ok(written) if written.is_empty() => {
                eprintln!("    queries: none bundled for `{}`", r.name);
            }
            Ok(written) => {
                let names: Vec<String> = written
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                eprintln!(
                    "    queries: {} ({} files)",
                    names.join(", "),
                    written.len()
                );
            }
            Err(e) => {
                eprintln!("    failed: {:#}", e);
                failures.push(r.name);
            }
        }
    }
    if !failures.is_empty() {
        bail!("failed to refresh: {}", failures.join(", "));
    }
    Ok(())
}

fn remove(args: &[String], grammar_dir: &Path) -> Result<()> {
    if args.is_empty() {
        bail!("remove: need at least one grammar name");
    }
    for name in args {
        let removed = build::remove(name, grammar_dir)?;
        if removed {
            eprintln!("removed: {}", name);
        } else {
            eprintln!("not installed: {}", name);
        }
    }
    Ok(())
}
