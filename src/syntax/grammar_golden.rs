//! Golden tests for the per-language `indents.scm` / `textobjects.scm`
//! queries, run against the *real* tree-sitter grammars.
//!
//! For every file under `tests/inputs/` (a committed corpus — copies of
//! the gitignored `assets/samples/` scratch files), the engine is built
//! from the prebuilt grammar library for the host target and parsed.
//! Two snapshots are rendered and compared against committed goldens
//! under `tests/golden/`:
//!
//! - `indents/<sample>.txt` — every indent scope as `start..end` (1-based
//!   line numbers), nested by containment. This is what verifies that the
//!   indent block's *start line and end line* are correct.
//! - `textobjects/<sample>.txt` — every `function.*` / `class.*` /
//!   `parameter.*` / `type.*` text object as `startRow:col-endRow:col`.
//!
//! These tests are `#[ignore]`d by default: they need a compiled grammar
//! library for the host target, which only exists after
//! `vorto grammar bundle` has populated
//! `assets/grammars-prebuilt/<host-target>/`. That is exactly what the
//! release workflow does before building, so the tests run there (per
//! target). To run locally on a host that already has prebuilt grammars:
//!
//! ```text
//! cargo test --locked grammar_golden -- --include-ignored
//! ```
//!
//! To (re)generate the goldens after an intentional query change:
//!
//! ```text
//! UPDATE_EXPECT=1 cargo test --locked grammar_golden -- --include-ignored
//! ```
//!
//! When no prebuilt grammars exist for the host (e.g. plain CI without a
//! prior bundle), the tests skip cleanly rather than fail.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use flate2::read::GzDecoder;

use super::{Engine, Loader};
use crate::config::{Language, LanguageRegistry};

/// Host target triple, stashed by `build.rs` as a `rustc-env`.
const HOST_TARGET: &str = env!("BUILD_TARGET");

/// Text-object capture names we snapshot, in a stable render order.
const TEXT_OBJECT_TARGETS: &[&str] = &[
    "function.outer",
    "function.inner",
    "class.outer",
    "class.inner",
    "parameter.outer",
    "parameter.inner",
    "type.outer",
    "type.inner",
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn query_dir() -> PathBuf {
    manifest_dir().join("assets").join("queries")
}

/// Committed corpus of source files the harness parses. NOT
/// `assets/samples/` — that dir is gitignored (local scratch), so it's
/// absent in CI and other checkouts. These are a committed copy.
fn samples_dir() -> PathBuf {
    manifest_dir().join("tests").join("inputs")
}

/// Directory of gzip-compressed prebuilt grammar libraries for the host
/// target, or `None` when this host has not been bundled.
fn prebuilt_dir() -> Option<PathBuf> {
    let dir = manifest_dir()
        .join("assets")
        .join("grammars-prebuilt")
        .join(HOST_TARGET);
    dir.is_dir().then_some(dir)
}

/// Extract every `<name>.<ext>.gz` from the host's prebuilt directory into
/// a shared temp dir, once per test process, and return that dir (usable
/// as a [`Loader`] `grammar_dir`). `None` when there is nothing to extract
/// for this host.
fn grammar_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        // The ONLY legitimate skip is "this host has no prebuilt grammars".
        // Past that point every IO step is `expect`/`panic` so a corrupt
        // `.gz`, a permission error, or a partial extraction fails loudly
        // on a release runner instead of masquerading as a clean skip.
        let src = prebuilt_dir()?;
        let out = std::env::temp_dir().join(format!("vorto-grammar-golden-{}", std::process::id()));
        // Clear any stale extraction from a previous run that reused this
        // PID (different grammar set / layout would otherwise linger).
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).expect("create temp grammar dir");
        for entry in fs::read_dir(&src).expect("read prebuilt grammar dir") {
            let entry = entry.expect("read prebuilt dir entry");
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // "<grammar>.dylib.gz" -> "<grammar>.dylib" (what library_path wants).
            let Some(lib_name) = name.strip_suffix(".gz") else {
                continue;
            };
            let gz = fs::read(entry.path()).expect("read prebuilt .gz");
            let mut decoder = GzDecoder::new(&gz[..]);
            let mut bytes = Vec::new();
            decoder
                .read_to_end(&mut bytes)
                .unwrap_or_else(|e| panic!("decompress {name}: {e}"));
            fs::write(out.join(lib_name), bytes).expect("write extracted grammar lib");
        }
        Some(out)
    })
    .as_deref()
}

fn registry() -> &'static LanguageRegistry {
    static R: OnceLock<LanguageRegistry> = OnceLock::new();
    R.get_or_init(|| LanguageRegistry::build(HashMap::new(), HashMap::new()).unwrap())
}

/// One process-wide [`Loader`]. The loader owns the grammar libraries and
/// MUST outlive every [`Engine`] it produces — an `Engine` holds a raw
/// `Language` pointer into the library, so a dropped `Loader` (dlclose)
/// would dangle it. A `OnceLock` keeps it alive for the whole test
/// process; the `Mutex` allows the parallel tests to share it (and reuse
/// the cached libraries). Caller must ensure [`grammar_dir`] is `Some`.
fn loader() -> &'static Mutex<Loader> {
    static L: OnceLock<Mutex<Loader>> = OnceLock::new();
    L.get_or_init(|| {
        let dir = grammar_dir().expect("loader() called without prebuilt grammars");
        Mutex::new(Loader::new(dir.to_path_buf(), query_dir()))
    })
}

/// All files directly under `assets/samples/`, sorted for stable test
/// ordering. Subdirectories are skipped.
fn samples() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(samples_dir())
        .expect("tests/inputs must exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

/// Build a parsed [`Engine`] for `path` using the prebuilt grammar for its
/// language. Returns `None` (with a logged reason) when the language is
/// unknown, the grammar library is missing, or the engine fails to build —
/// none of which should fail the suite, they just aren't exercised.
fn build_engine(path: &Path) -> Option<(Language, Engine, String)> {
    let grammar_dir = grammar_dir()?;
    let lang = registry().by_path(path)?.clone();
    let have_lib = ["so", "dylib", "dll"]
        .iter()
        .any(|ext| grammar_dir.join(format!("{}.{ext}", lang.grammar)).exists());
    if !have_lib {
        return None;
    }
    let source = fs::read_to_string(path).ok()?;
    let mut loader = loader().lock().unwrap();
    match loader.engine_for(&lang) {
        Ok(mut engine) => {
            engine.refresh(&source, 0);
            Some((lang, engine, source))
        }
        Err(e) => {
            eprintln!("grammar_golden: skip {} ({e})", path.display());
            None
        }
    }
}

/// Render every indent scope as `start..end` on 1-based line numbers,
/// indented by containment depth and annotated with the opening line's
/// trimmed text. This is the artifact that pins down each block's start
/// and end line.
fn render_indents(engine: &Engine, source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let last = lines.len().saturating_sub(1);
    let mut scopes = engine.indent_scopes_in_rows(0, last);
    // Outer-first: smaller start, then larger end (wider scope) first.
    scopes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    scopes.dedup();

    let mut out = String::new();
    for &(s, e) in &scopes {
        let depth = scopes
            .iter()
            .filter(|&&(s2, e2)| s2 <= s && e2 >= e && (s2 < s || e2 > e))
            .count();
        let text = lines.get(s).map(|l| l.trim()).unwrap_or("");
        out.push_str(&format!(
            "{}{}..{}  {text}\n",
            "  ".repeat(depth),
            s + 1,
            e + 1
        ));
    }
    out
}

/// Render every text object of each tracked kind as
/// `startRow:startCol-endRow:endCol` (1-based rows, 0-based char cols),
/// annotated with the start line's trimmed text.
fn render_text_objects(engine: &Engine, source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::new();
    for &target in TEXT_OBJECT_TARGETS {
        for (sr, sc, er, ec) in engine.all_text_objects(target) {
            let text = lines.get(sr).map(|l| l.trim()).unwrap_or("");
            out.push_str(&format!(
                "{target:<16} {}:{sc}-{}:{ec}  {text}\n",
                sr + 1,
                er + 1
            ));
        }
    }
    out
}

fn golden_path(kind: &str, sample: &str) -> PathBuf {
    manifest_dir()
        .join("tests")
        .join("golden")
        .join(kind)
        .join(format!("{sample}.txt"))
}

/// Compare `actual` against the committed golden for `kind`/`sample`.
/// With `UPDATE_EXPECT=1` set, (re)write the golden instead (and remove a
/// stale one when the language now produces nothing). Without a golden on
/// disk, nothing is asserted — a missing golden means "not recorded yet",
/// not "expected empty".
fn check_golden(kind: &str, sample: &str, actual: &str) {
    let path = golden_path(kind, sample);
    let existing = fs::read_to_string(&path).ok();

    if std::env::var_os("UPDATE_EXPECT").is_some() {
        if actual.trim().is_empty() {
            let _ = fs::remove_file(&path);
        } else if existing.as_deref() != Some(actual) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, actual).unwrap();
        }
        return;
    }

    if let Some(expected) = existing {
        assert_eq!(
            actual, expected,
            "golden mismatch for {kind}/{sample}; rerun with `UPDATE_EXPECT=1 cargo test grammar_golden -- --include-ignored` to refresh"
        );
    }
}

/// Run `render` over every sample for one `kind` and check against the
/// goldens. A sample that fails to build but has a committed golden is a
/// hard failure — that's how a single-language grammar/loader regression
/// (e.g. Rust alone stops loading) is caught, rather than being silently
/// downgraded to a skip. Samples with no golden may skip (the language
/// genuinely has no `{indents,textobjects}.scm`).
fn run_golden(kind: &str, render: impl Fn(&Engine, &str) -> String) {
    if grammar_dir().is_none() {
        eprintln!("grammar_golden: no prebuilt grammars for {HOST_TARGET}; skipping");
        return;
    }
    let mut checked = 0usize;
    for path in samples() {
        let name = path.file_name().unwrap().to_str().unwrap();
        match build_engine(&path) {
            Some((_, engine, source)) => {
                check_golden(kind, name, &render(&engine, &source));
                checked += 1;
            }
            None => assert!(
                !golden_path(kind, name).exists(),
                "{name} has a committed {kind} golden but failed to build an engine — \
                 grammar/loader regression (see the eprintln above for the reason)"
            ),
        }
    }
    assert!(
        checked > 0,
        "prebuilt grammars present but no sample loaded — grammar/loader regression?"
    );
}

#[test]
#[ignore = "needs prebuilt grammars for the host target (run after `vorto grammar bundle`): cargo test -- --include-ignored"]
fn indents_golden() {
    run_golden("indents", render_indents);
}

#[test]
#[ignore = "needs prebuilt grammars for the host target (run after `vorto grammar bundle`): cargo test -- --include-ignored"]
fn text_objects_golden() {
    run_golden("textobjects", render_text_objects);
}
