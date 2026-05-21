//! Grammar-library loading and `.scm` query reading.
//!
//! [`Loader`] owns the `.so`/`.dylib`/`.dll` grammar libraries and their
//! resolved `tree_sitter::Language` handles. It also reads the
//! `highlights.scm` / `textobjects.scm` / `indents.scm` query files from
//! disk, honoring `; inherits: <lang>` headers. Libraries are cached
//! indefinitely — `Loader` is meant to live for the whole program so
//! the loaded `Language` pointers stay valid.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use libloading::{Library, Symbol};
use tree_sitter::{Language, Query};

use crate::config::Language as LangSpec;

use super::engine::Engine;
use super::injection::SubHighlighter;

/// Owns `tree-sitter` grammar libraries and resolves them to
/// `tree_sitter::Language` handles on demand. Holds the [`Library`]
/// instances for the lifetime of the program so that the `Language`
/// pointer-into-library remains valid.
pub struct Loader {
    grammar_dir: PathBuf,
    query_dir: PathBuf,
    libs: HashMap<String, Library>,
    languages: HashMap<String, Language>,
}

impl Loader {
    pub fn new(grammar_dir: PathBuf, query_dir: PathBuf) -> Self {
        Self {
            grammar_dir,
            query_dir,
            libs: HashMap::new(),
            languages: HashMap::new(),
        }
    }

    /// Try to build a fresh [`Highlighter`] for `spec`. Loads the
    /// grammar (cached), compiles its highlights query, and — if a
    /// `textobjects.scm` / `indents.scm` is also present — their
    /// queries too. Both are optional; missing files are not an error.
    pub fn engine_for(&mut self, spec: &LangSpec) -> Result<Engine> {
        let lang = self.load_language(spec)?;
        let highlights_src = self.read_query(spec, "highlights")?;
        let textobjects_src = self.read_query(spec, "textobjects").ok();
        let indents_src = self.read_query(spec, "indents").ok();
        let injections_src = self.read_query(spec, "injections").ok();
        let injection = match injections_src.as_deref() {
            Some(src) => super::injection::InjectionEngine::build(&lang, src, self)?,
            None => None,
        };
        Engine::new(
            lang,
            &highlights_src,
            textobjects_src.as_deref(),
            indents_src.as_deref(),
            injection,
        )
    }

    /// Build a sub-highlighter (grammar handle + compiled `highlights`
    /// query + capture names) for `lang_name`. Used by the injection
    /// engine to set up child parsers for languages embedded inside
    /// host files (e.g. `<script lang="ts">` inside Vue). Returns an
    /// error when the grammar or query isn't installed — the injection
    /// engine treats that as "skip this language" rather than fatal.
    pub fn sub_highlighter_for(&mut self, lang_name: &str) -> Result<SubHighlighter> {
        let language = self.load_language_by_name(lang_name)?;
        let highlights_src = self.read_query_by_name(lang_name, "highlights")?;
        let query = Query::new(&language, &highlights_src)
            .with_context(|| format!("compiling sub-language `{}` highlights query", lang_name))?;
        let capture_names = query
            .capture_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        // indents.scm is optional — a sub-language without one still
        // highlights fine, it just won't contribute deep indent scopes
        // to the host's guide computation.
        let (indents, indent_capture_names) =
            match self.read_query_by_name(lang_name, "indents").ok() {
                Some(src) => match Query::new(&language, &src) {
                    Ok(q) => {
                        let names = q.capture_names().iter().map(|s| s.to_string()).collect();
                        (Some(q), names)
                    }
                    Err(_) => (None, Vec::new()),
                },
                None => (None, Vec::new()),
            };
        Ok(SubHighlighter::new(
            language,
            query,
            capture_names,
            indents,
            indent_capture_names,
        ))
    }

    /// Variant of [`Self::load_language`] keyed by a bare language name
    /// rather than a [`LangSpec`]. The grammar name is used both as the
    /// `.so` / `.dylib` stem and the `tree_sitter_<name>` symbol root,
    /// matching the convention `grammar install <name>` uses.
    fn load_language_by_name(&mut self, name: &str) -> Result<Language> {
        if let Some(lang) = self.languages.get(name) {
            return Ok(lang.clone());
        }
        let path = library_path(&self.grammar_dir, name)
            .with_context(|| format!("locating grammar `{}`", name))?;
        let lib = unsafe { Library::new(&path) }
            .with_context(|| format!("loading grammar library {}", path.display()))?;
        let symbol_name = format!("tree_sitter_{}", name.replace('-', "_"));
        let language = unsafe {
            let sym: Symbol<unsafe extern "C" fn() -> Language> =
                lib.get(symbol_name.as_bytes()).with_context(|| {
                    format!("symbol `{}` missing in {}", symbol_name, path.display())
                })?;
            sym()
        };
        self.libs.insert(name.to_string(), lib);
        self.languages.insert(name.to_string(), language.clone());
        Ok(language)
    }

    /// Variant of [`Self::read_query`] keyed by a bare language name.
    /// Same `; inherits: …` honoring as the spec-driven path.
    fn read_query_by_name(&self, name: &str, kind: &str) -> Result<String> {
        let mut visited = HashSet::new();
        read_query_recursive(&self.query_dir, name, kind, &mut visited)
    }

    /// Resolve `spec.grammar` to a `tree_sitter::Language`, loading the
    /// underlying shared library on first request.
    fn load_language(&mut self, spec: &LangSpec) -> Result<Language> {
        if let Some(lang) = self.languages.get(&spec.grammar) {
            return Ok(lang.clone());
        }
        let dir = spec.grammar_dir.as_ref().unwrap_or(&self.grammar_dir);
        let path = library_path(dir, &spec.grammar)
            .with_context(|| format!("locating grammar `{}`", spec.grammar))?;
        let lib = unsafe { Library::new(&path) }
            .with_context(|| format!("loading grammar library {}", path.display()))?;
        let symbol_name = format!("tree_sitter_{}", spec.grammar.replace('-', "_"));
        let language = unsafe {
            let sym: Symbol<unsafe extern "C" fn() -> Language> =
                lib.get(symbol_name.as_bytes()).with_context(|| {
                    format!("symbol `{}` missing in {}", symbol_name, path.display())
                })?;
            sym()
        };
        // Library must outlive `language` (its pointer points into the
        // library). Stash it before we return.
        self.libs.insert(spec.grammar.clone(), lib);
        self.languages
            .insert(spec.grammar.clone(), language.clone());
        Ok(language)
    }

    /// Read `<query_dir>/<name>/<kind>.scm`, honoring a per-language
    /// override on `spec.query_dir` for the directory. Resolves
    /// `; inherits: <lang>[,<lang>…]` headers by recursively loading and
    /// prepending the inherited languages' same-kind files (helix's
    /// convention), so e.g. a 35-line `typescript/highlights.scm` that
    /// inherits `javascript` ends up with the full ecma rule set in
    /// front of the TS-specific delta.
    fn read_query(&self, spec: &LangSpec, kind: &str) -> Result<String> {
        let base = spec.query_dir.as_ref().unwrap_or(&self.query_dir);
        let mut visited = HashSet::new();
        read_query_recursive(base, &spec.name, kind, &mut visited)
    }
}

/// Read `<base>/<lang>/<kind>.scm` and, if its first comment lines
/// contain `; inherits: a,b,…`, prepend each inherited language's
/// same-kind file. Inherited content goes first so the requesting
/// language's patterns appear *later* in the combined source — the
/// query engine and the UI's overlay both treat later patterns as
/// higher-priority, which lets the more-specific language win on
/// conflicts.
///
/// `visited` short-circuits cycles. A missing inherited file is logged
/// and skipped rather than failing the whole load — losing one parent's
/// highlights is better than losing all of them.
fn read_query_recursive(
    base: &Path,
    lang_name: &str,
    kind: &str,
    visited: &mut HashSet<String>,
) -> Result<String> {
    if !visited.insert(lang_name.to_string()) {
        return Ok(String::new());
    }
    let path = base.join(lang_name).join(format!("{}.scm", kind));
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading query {}", path.display()))?;

    let inherits = parse_inherits(&content);
    if inherits.is_empty() {
        return Ok(content);
    }

    let mut combined = String::new();
    for inh in inherits {
        match read_query_recursive(base, &inh, kind, visited) {
            Ok(s) if !s.is_empty() => {
                combined.push_str(&s);
                if !combined.ends_with('\n') {
                    combined.push('\n');
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "inherited query `{}/{}.scm` (from `{}`) skipped: {:#}",
                    inh, kind, lang_name, e
                );
            }
        }
    }
    combined.push_str(&content);
    Ok(combined)
}

/// Scan leading comment lines for `; inherits: a,b,c` (single or
/// double semicolons, any whitespace). Returns the parsed language
/// names in declaration order. Stops at the first non-comment / non-
/// blank line — the header has to live at the top of the file.
fn parse_inherits(content: &str) -> Vec<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with(';') {
            return Vec::new();
        }
        let stripped = trimmed.trim_start_matches(';').trim();
        if let Some(rest) = stripped.strip_prefix("inherits:") {
            return rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// Try each platform-appropriate extension under `dir/<name>.*` and
/// return the first hit. Without this, we'd force users to spell out
/// the extension on every platform.
fn library_path(dir: &Path, name: &str) -> Result<PathBuf> {
    let candidates = [
        format!("{}.so", name),
        format!("{}.dylib", name),
        format!("{}.dll", name),
        format!("lib{}.so", name),
        format!("lib{}.dylib", name),
    ];
    for c in &candidates {
        let p = dir.join(c);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "no grammar library for `{}` in {}",
        name,
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_path_reports_missing() {
        let dir = std::env::temp_dir();
        let result = library_path(&dir, "definitely-not-a-real-grammar");
        assert!(result.is_err());
    }

    #[test]
    fn parse_inherits_picks_up_single_lang() {
        let src = "; inherits: javascript\n\n(identifier) @variable\n";
        assert_eq!(parse_inherits(src), vec!["javascript".to_string()]);
    }

    #[test]
    fn parse_inherits_handles_double_semicolon_and_multiple_langs() {
        let src = ";; inherits: ecma, jsx\n";
        assert_eq!(
            parse_inherits(src),
            vec!["ecma".to_string(), "jsx".to_string()]
        );
    }

    #[test]
    fn parse_inherits_skips_blank_leading_lines() {
        let src = "\n\n; inherits: rust\n";
        assert_eq!(parse_inherits(src), vec!["rust".to_string()]);
    }

    #[test]
    fn parse_inherits_returns_empty_when_no_header() {
        let src = "; Types\n(identifier) @variable\n";
        assert!(parse_inherits(src).is_empty());
    }

    #[test]
    fn parse_inherits_stops_at_first_non_comment() {
        // A code line before the header means there's no header.
        let src = "(identifier) @variable\n; inherits: foo\n";
        assert!(parse_inherits(src).is_empty());
    }

    #[test]
    fn read_query_recursive_prepends_inherited() {
        let dir = std::env::temp_dir().join("vorto_inherits_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("parent")).unwrap();
        std::fs::create_dir_all(dir.join("child")).unwrap();
        std::fs::write(dir.join("parent/highlights.scm"), "PARENT\n").unwrap();
        std::fs::write(
            dir.join("child/highlights.scm"),
            "; inherits: parent\nCHILD\n",
        )
        .unwrap();

        let mut visited = HashSet::new();
        let out = read_query_recursive(&dir, "child", "highlights", &mut visited).unwrap();
        assert!(out.contains("PARENT"));
        assert!(out.contains("CHILD"));
        // Parent appears before child so child patterns win.
        assert!(out.find("PARENT").unwrap() < out.find("CHILD").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_query_recursive_breaks_cycles() {
        let dir = std::env::temp_dir().join("vorto_inherits_cycle");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/highlights.scm"), "; inherits: b\nA\n").unwrap();
        std::fs::write(dir.join("b/highlights.scm"), "; inherits: a\nB\n").unwrap();

        let mut visited = HashSet::new();
        let out = read_query_recursive(&dir, "a", "highlights", &mut visited).unwrap();
        // Both files load once, the second visit short-circuits.
        assert!(out.contains("A"));
        assert!(out.contains("B"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
