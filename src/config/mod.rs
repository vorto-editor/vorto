//! Resolved user configuration loaded from `~/.config/vorto/config.toml`.
//!
//! The public type [`Config`] is a pure data struct holding the final,
//! ready-to-use settings (`keymap`, `cursor_shapes`, `languages`,
//! `grammar_dir`, `query_dir`). [`Config::load`] consumes a TOML file
//! (when present) and produces a `Config`; everything else in this
//! module is internal plumbing.
//!
//! Schema:
//!
//! ```toml
//! [[bind]]
//! keys   = "<C-s>"      # vim-style key notation; see `keys::parse_sequence`
//! action = "save"        # named action; see `keys::action_to_token`
//!
//! [[bind]]
//! keys   = "<space>w"   # 2-key sequence — installed in the Leader context
//! action = "save"
//! ```
//!
//! Bindings either **override** an existing default (same key sequence)
//! or **add** new ones. Only single keys (Initial context) and
//! `<space>X` two-key sequences (Leader context) are supported in v1.

mod agent;
mod command;
mod cursor;
mod editor;
mod finder;
mod keymap;
mod keys;
mod languages;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::KeyCode;
use serde::Deserialize;

pub use agent::{AgentRegistry, persist_default_agent};
pub use command::{
    AGENT_SUBCOMMANDS, Args, COMMANDS, COPILOT_SUBCOMMANDS, Command, GRAMMAR_SUBCOMMANDS, Inline,
    Kind, resolve_subcommand,
};
pub use cursor::{CursorShape, CursorShapes};
pub use editor::{EditorConfig, EditorToml, IndentGuideStyle};
pub use finder::{FinderConfig, FinderToml};
pub use keymap::{
    BOOKMARK_BINDINGS, BRACKET_NEXT_BINDINGS, BRACKET_PREV_BINDINGS, CTRL_W_BINDINGS,
    GOTO_BINDINGS, KeySig, Keymap, LEADER_DEFAULTS, OBJECT_BINDINGS, OP_PENDING_BINDINGS,
    WINDOW_BINDINGS, Z_BINDINGS,
};
pub use languages::{
    FormatterConfig, Language, LanguageConfig, LanguageRegistry, LspConfig, LspToml,
};

use cursor::{CursorConfig, resolve_cursor_shapes};
use keymap::LEADER;
use keys::{action_to_token, parse_sequence};

/// Resolved configuration — the runtime state of "what settings is the
/// app currently using". Pure data: every field is filled in by
/// [`Config::load`] and never mutated afterward.
pub struct Config {
    pub keymap: Keymap,
    pub cursor_shapes: CursorShapes,
    pub languages: LanguageRegistry,
    /// Global editor settings, applied to every buffer that doesn't get
    /// a more specific override from a `[languages.<name>]` block.
    pub editor: EditorConfig,
    /// File picker / tree explorer behavior — currently just the
    /// `hidden_patterns` glob list.
    pub finder: FinderConfig,
    /// AI-agent catalog + default, driving the `:agent` command. The
    /// catalog is built-ins overlaid with `[agents.*]`; `default` names
    /// the agent a bare `:agent` launches.
    pub agents: AgentRegistry,
    /// User-defined grammar recipes from `[grammars.<name>]`. Augments
    /// the built-in catalog; an entry whose name matches a built-in
    /// overrides it. Sorted by name for stable output.
    pub grammars: Vec<GrammarSource>,
    /// Absolute path to the grammar directory (`<grammar>.{so,dylib,dll}`).
    pub grammar_dir: PathBuf,
    /// Absolute path to the query directory (`<lang>/highlights.scm`).
    pub query_dir: PathBuf,
    /// Active theme name (`theme = "..."`). Resolved against bundled +
    /// `~/.config/vorto/themes/*.toml` by [`crate::theme::load_by_name`]
    /// at startup. Defaults to `ansi` — the terminal's own palette.
    pub theme: String,
}

/// Raw `[grammars.<name>]` entry: where to fetch a grammar's source.
/// Mirrors the built-in recipe shape. The git URL is `source` (alias
/// `src`); `rev` pins a tag/branch/commit (omit it to build the latest
/// default branch); `subpath` selects a grammar inside a monorepo.
#[derive(Debug, Deserialize, Clone)]
struct GrammarToml {
    #[serde(alias = "src")]
    source: String,
    #[serde(default)]
    rev: Option<String>,
    #[serde(default)]
    subpath: Option<String>,
}

/// A resolved user grammar recipe, with the table-key name folded in.
#[derive(Debug, Clone)]
pub struct GrammarSource {
    pub name: String,
    pub source: String,
    pub rev: Option<String>,
    pub subpath: Option<String>,
}

impl Config {
    /// Load and resolve the user config from `path` (if it exists).
    /// Missing file or `None` path yields a Config seeded entirely from
    /// built-in defaults.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let toml = Toml::load(path)?;
        Self::resolve(toml)
    }

    fn resolve(toml: Toml) -> Result<Self> {
        let mut keymap = Keymap::vim_default();
        for (i, b) in toml.bind.iter().enumerate() {
            install_binding(&mut keymap, &b.keys, &b.action)
                .with_context(|| format!("bind[{}] ({} → {})", i, b.keys, b.action))?;
        }

        let cursor_shapes = resolve_cursor_shapes(&toml.cursor)?;
        let editor = EditorConfig::default().overlay(&toml.editor);
        let finder = FinderConfig::default().overlay(&toml.finder);
        let agents = AgentRegistry::build(toml.agents, toml.agent);
        let languages = LanguageRegistry::build(toml.languages, toml.lsp)?;
        let mut grammars: Vec<GrammarSource> = toml
            .grammars
            .into_iter()
            .map(|(name, g)| GrammarSource {
                name,
                source: g.source,
                rev: g.rev,
                subpath: g.subpath,
            })
            .collect();
        grammars.sort_by(|a, b| a.name.cmp(&b.name));
        let grammar_dir = toml
            .grammar_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| default_subdir("grammars"));
        let query_dir = toml
            .query_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| default_subdir("queries"));
        let theme = toml.theme.unwrap_or_else(|| crate::theme::ANSI.to_string());

        Ok(Self {
            keymap,
            cursor_shapes,
            languages,
            editor,
            finder,
            agents,
            grammars,
            grammar_dir,
            query_dir,
            theme,
        })
    }
}

// ────────────────────────────────────────────────────────────────────────
// TOML schema (private — implementation detail of `Config::load`).
// ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct Toml {
    #[serde(default)]
    bind: Vec<BindEntry>,
    #[serde(default)]
    cursor: CursorConfig,
    /// Global `[editor]` table. Per-language overrides flatten the same
    /// fields directly into each `[languages.<name>]` table.
    #[serde(default)]
    editor: EditorToml,
    /// `[finder]` table — file picker / explorer behavior.
    #[serde(default)]
    finder: FinderToml,
    /// `[agent]` settings table (currently just `default`).
    #[serde(default)]
    agent: agent::AgentSettingsToml,
    /// `[agents.<name>]` blocks — per-agent command lines that overlay
    /// the built-in catalog.
    #[serde(default)]
    agents: std::collections::HashMap<String, agent::AgentToml>,
    /// `[languages.<name>]` blocks. Resolved against built-in defaults
    /// by [`LanguageRegistry::build`].
    #[serde(default)]
    languages: std::collections::HashMap<String, LanguageConfig>,
    /// `[lsp.<server-name>]` blocks. Built-in servers can be partially
    /// overlaid (e.g. just `args`); entirely new servers must include
    /// `command`. Referenced from `[languages.<lang>].lsp = ["<name>"]`.
    #[serde(default)]
    lsp: std::collections::HashMap<String, LspToml>,
    /// `[grammars.<name>]` blocks — user-defined grammar recipes that
    /// make `vorto grammar install <name>` work for grammars not in the
    /// built-in catalog. Resolved into [`Config::grammars`].
    #[serde(default)]
    grammars: std::collections::HashMap<String, GrammarToml>,
    /// Directory holding `<grammar>.{so,dylib,dll}`. Defaults to
    /// `<config>/grammars`.
    grammar_dir: Option<String>,
    /// Directory holding `<lang>/highlights.scm`. Defaults to
    /// `<config>/queries`.
    query_dir: Option<String>,
    /// Active theme name. Resolved by [`crate::theme::load_by_name`] at
    /// startup; defaults to `ansi` (terminal palette).
    theme: Option<String>,
}

impl Toml {
    fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
    }
}

/// Single `[[bind]]` row from the TOML schema.
#[derive(Debug, Deserialize)]
struct BindEntry {
    keys: String,
    action: String,
}

// ────────────────────────────────────────────────────────────────────────
// Path resolution
// ────────────────────────────────────────────────────────────────────────

/// Resolve the config-file path. Workspace-local `.vorto/config.toml`
/// (or a plain `.vorto` file) in the current directory wins; otherwise
/// honors `$XDG_CONFIG_HOME` if set, then falls back to
/// `$HOME/.config/vorto/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(p) = workspace_path(&cwd)
    {
        return Some(p);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg).join("vorto/config.toml");
        if p.exists() {
            return Some(p);
        }
    }
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".config/vorto/config.toml");
    Some(p)
}

/// Look for a workspace-local config rooted at `cwd`. Supports two
/// layouts: `<cwd>/.vorto/config.toml` (directory form, mirroring the
/// global `~/.config/vorto/` layout) and `<cwd>/.vorto` (single TOML
/// file). Returns `None` if neither exists.
fn workspace_path(cwd: &Path) -> Option<PathBuf> {
    let vorto = cwd.join(".vorto");
    let meta = std::fs::metadata(&vorto).ok()?;
    if meta.is_dir() {
        let p = vorto.join("config.toml");
        return p.exists().then_some(p);
    }
    if meta.is_file() {
        return Some(vorto);
    }
    None
}

// ────────────────────────────────────────────────────────────────────────
// Theme persistence
// ────────────────────────────────────────────────────────────────────────

/// Write `theme = "<name>"` to the user's config file, preserving the
/// rest of the file verbatim, and return the path written. Used by the
/// `:theme` picker when the user commits a choice. Mirrors
/// [`persist_default_agent`]'s text-preserving, non-destructive approach.
pub fn persist_theme(name: &str) -> Result<PathBuf> {
    let path = default_path().ok_or_else(|| anyhow!("no config path (is $HOME set?)"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let updated = upsert_theme(&existing, name);
    std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Set the top-level `theme` key in a config file's text. Replaces an
/// existing top-level `theme = ...` assignment if one appears before the
/// first `[table]` header; otherwise prepends the line (a bare key must
/// precede any table header, since a key written *after* `[foo]` would
/// belong to that table). All other lines are kept verbatim; line
/// endings are normalized to `\n` (the file is rejoined with `\n`), and
/// a trailing newline is preserved when the original had one.
fn upsert_theme(existing: &str, name: &str) -> String {
    let line = format!("theme = {}", agent::toml_basic_string(name));
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();

    // Find an existing top-level `theme = ...`, scanning only until the
    // first `[table]` header — a `theme` key inside a table isn't the
    // top-level one we own.
    let mut target = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with('[') {
            break;
        }
        if is_theme_assignment(t) {
            target = Some(i);
            break;
        }
    }

    match target {
        Some(i) => lines[i] = line,
        None => lines.insert(0, line),
    }

    let mut s = lines.join("\n");
    if existing.is_empty() || existing.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// True for a `theme = ...` top-level assignment line (key left of the
/// first `=`, trimmed, equals `theme`). Comments (`# theme = ...`) and
/// other keys (`theme_dir = ...`) don't match.
fn is_theme_assignment(line: &str) -> bool {
    line.split_once('=')
        .is_some_and(|(k, _)| k.trim() == "theme")
}

fn default_subdir(name: &str) -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("vorto").join(name);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/vorto").join(name);
    }
    PathBuf::from(name)
}

/// Directories scanned for user theme files (`<name>.toml`), in
/// descending priority: a workspace-local `.vorto/themes/` (when the cwd
/// has a `.vorto` *directory*) wins over the global
/// `~/.config/vorto/themes/`. A theme present in an earlier dir shadows
/// the same name later — and any user theme shadows a bundled one,
/// matching the grammar/language "user overrides built-in" rule.
pub fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        let vorto = cwd.join(".vorto");
        if vorto.is_dir() {
            dirs.push(vorto.join("themes"));
        }
    }
    dirs.push(default_subdir("themes"));
    dirs
}

// ────────────────────────────────────────────────────────────────────────
// Binding application
// ────────────────────────────────────────────────────────────────────────

fn install_binding(keymap: &mut Keymap, keys: &str, action: &str) -> Result<()> {
    let sequence = parse_sequence(keys)?;
    let token = action_to_token(action).ok_or_else(|| anyhow!("unknown action: {}", action))?;
    match sequence.as_slice() {
        [k] => {
            keymap.bind_initial(*k, token);
        }
        [first, second] if first.code == KeyCode::Char(LEADER) && first.modifiers.is_empty() => {
            keymap.bind_leader(*second, token);
        }
        [_, _] => bail!(
            "only `<space>X` two-key sequences are supported; got: {}",
            keys
        ),
        _ => bail!(
            "sequences of more than 2 keys aren't supported yet; got: {}",
            keys
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{DirectKind, Token};
    use crossterm::event::KeyModifiers;

    #[test]
    fn install_leader_binding() {
        let mut km = Keymap::vim_default();
        install_binding(&mut km, "<space>w", "save").unwrap();
        let sig = KeySig::new(KeyCode::Char('w'), KeyModifiers::NONE);
        assert_eq!(km.leader.get(&sig), Some(&Token::Direct(DirectKind::Save)));
    }

    #[test]
    fn install_overrides_existing() {
        let mut km = Keymap::vim_default();
        install_binding(&mut km, "u", "quit").unwrap();
        let sig = KeySig::new(KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(km.initial.get(&sig), Some(&Token::Direct(DirectKind::Quit)));
    }

    #[test]
    fn parse_inline_array_form() {
        let text = r#"
bind = [
  { keys = "<C-s>", action = "save" },
  { keys = "<space>w", action = "save" },
]
"#;
        let toml: Toml = toml::from_str(text).unwrap();
        assert_eq!(toml.bind.len(), 2);
        assert_eq!(toml.bind[0].keys, "<C-s>");
        assert_eq!(toml.bind[1].action, "save");
    }

    #[test]
    fn cursor_defaults_when_unset() {
        let toml: Toml = toml::from_str("").unwrap();
        let shapes = resolve_cursor_shapes(&toml.cursor).unwrap();
        assert!(matches!(shapes.normal, CursorShape::Block));
        assert!(matches!(shapes.insert, CursorShape::Bar));
        assert!(matches!(shapes.visual, CursorShape::Underbar));
    }

    #[test]
    fn cursor_overrides() {
        let text = r#"
[cursor]
normal = "bar"
insert = "underbar"
visual = "block"
"#;
        let toml: Toml = toml::from_str(text).unwrap();
        let shapes = resolve_cursor_shapes(&toml.cursor).unwrap();
        assert!(matches!(shapes.normal, CursorShape::Bar));
        assert!(matches!(shapes.insert, CursorShape::Underbar));
        assert!(matches!(shapes.visual, CursorShape::Block));
    }

    #[test]
    fn cursor_unknown_shape() {
        let text = r#"
[cursor]
normal = "diamond"
"#;
        let toml: Toml = toml::from_str(text).unwrap();
        assert!(resolve_cursor_shapes(&toml.cursor).is_err());
    }

    #[test]
    fn parse_languages_table() {
        let text = r#"
[languages.rust]
extensions = ["rs", "rlib"]

[languages.fish]
extensions = ["fish"]
grammar = "fish-shell"
"#;
        let toml: Toml = toml::from_str(text).unwrap();
        assert_eq!(toml.languages.len(), 2);
        assert_eq!(
            toml.languages["rust"].extensions.as_deref(),
            Some(&["rs".to_string(), "rlib".to_string()][..])
        );
        assert_eq!(
            toml.languages["fish"].grammar.as_deref(),
            Some("fish-shell")
        );
    }

    #[test]
    fn workspace_path_prefers_directory_config() {
        let root = std::env::temp_dir().join(format!(
            "vorto-test-ws-dir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let dot_vorto = root.join(".vorto");
        std::fs::create_dir_all(&dot_vorto).unwrap();
        let cfg = dot_vorto.join("config.toml");
        std::fs::write(&cfg, "").unwrap();

        assert_eq!(workspace_path(&root), Some(cfg));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn workspace_path_accepts_single_file() {
        let root = std::env::temp_dir().join(format!(
            "vorto-test-ws-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&root).unwrap();
        let dot_vorto = root.join(".vorto");
        std::fs::write(&dot_vorto, "").unwrap();

        assert_eq!(workspace_path(&root), Some(dot_vorto));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn workspace_path_absent_returns_none() {
        let root = std::env::temp_dir().join(format!(
            "vorto-test-ws-none-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&root).unwrap();

        assert_eq!(workspace_path(&root), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn upsert_theme_into_empty_file() {
        assert_eq!(
            upsert_theme("", "gruvbox-dark"),
            "theme = \"gruvbox-dark\"\n"
        );
    }

    #[test]
    fn upsert_theme_replaces_existing_top_level() {
        let existing = "theme = \"ansi\"\n\n[editor]\ntab_width = 4\n";
        let got = upsert_theme(existing, "catppuccin-mocha");
        assert_eq!(
            got,
            "theme = \"catppuccin-mocha\"\n\n[editor]\ntab_width = 4\n"
        );
    }

    #[test]
    fn upsert_theme_prepends_when_absent() {
        let existing = "[editor]\ntab_width = 4\n";
        let got = upsert_theme(existing, "gruvbox-dark");
        assert_eq!(got, "theme = \"gruvbox-dark\"\n[editor]\ntab_width = 4\n");
    }

    #[test]
    fn upsert_theme_ignores_key_inside_table() {
        // A `theme = ` line that lives inside a table isn't the top-level
        // key — the new one is prepended and the in-table line untouched.
        let existing = "[sometable]\ntheme = \"x\"\n";
        let got = upsert_theme(existing, "gruvbox-dark");
        assert_eq!(
            got,
            "theme = \"gruvbox-dark\"\n[sometable]\ntheme = \"x\"\n"
        );
    }

    #[test]
    fn upsert_theme_result_reparses() {
        let existing = "theme = \"ansi\"\n[editor]\ntab_width = 2\n";
        let got = upsert_theme(existing, "gruvbox-dark");
        let toml: Toml = toml::from_str(&got).unwrap();
        assert_eq!(toml.theme.as_deref(), Some("gruvbox-dark"));
    }

    #[test]
    fn parse_table_array_form() {
        let text = r#"
[[bind]]
keys = "<C-s>"
action = "save"

[[bind]]
keys = "<space>w"
action = "save"
"#;
        let toml: Toml = toml::from_str(text).unwrap();
        assert_eq!(toml.bind.len(), 2);
        assert_eq!(toml.bind[0].keys, "<C-s>");
    }
}
