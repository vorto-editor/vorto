//! Theme discovery: bundled themes vendored into the binary plus the
//! user's `~/.config/vorto/themes/*.toml` (and workspace `.vorto/themes`).
//!
//! Names are the file stems. A user theme whose stem matches a bundled
//! one **wins** — the same "user config shadows built-in" rule the
//! grammar and language catalogs use — so dropping
//! `~/.config/vorto/themes/default.toml` replaces the bundled `default`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use include_dir::{Dir, include_dir};
use ratatui::style::{Color, Modifier, Style};

use super::{Theme, parse};

/// The name of the special terminal-palette theme. Resolving it yields
/// styles built from the 16 ANSI color *names* (not RGB), so the
/// terminal renders them with the user's own scheme — `:theme ansi`
/// follows whatever colors their terminal is configured with. It's
/// synthesized in code (see [`ansi_theme`]) rather than read from a
/// file, so it's always available and a `themes/ansi.toml` can't shadow
/// it.
pub const ANSI: &str = "ansi";

/// Bundled themes, vendored at compile time. Mirrors the `assets/queries`
/// `include_dir!` pattern so a built-in theme ships in the binary with no
/// install step.
static BUILTINS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/themes");

/// Every available theme name — bundled ∪ user — sorted and de-duped.
/// A name present in both is listed once (the user file is what
/// [`load_by_name`] will actually read).
pub fn available() -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    // The terminal-palette theme is synthesized, not a file.
    names.insert(ANSI.to_string());
    for file in BUILTINS.files() {
        if let Some(stem) = theme_stem(file.path()) {
            names.insert(stem);
        }
    }
    for dir in crate::config::theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(stem) = theme_stem(&entry.path()) {
                names.insert(stem);
            }
        }
    }
    names.into_iter().collect()
}

/// Load a theme by name. User directories are checked first (override),
/// then the bundled set. Unknown name → error.
pub fn load_by_name(name: &str) -> Result<Theme> {
    // `ansi` is the terminal-palette theme: synthesized, never a file.
    if name == ANSI {
        return Ok(ansi_theme());
    }
    for dir in crate::config::theme_dirs() {
        let path = dir.join(format!("{name}.toml"));
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading theme {}", path.display()))?;
            return parse(name, &text);
        }
    }
    let file = BUILTINS
        .get_file(format!("{name}.toml"))
        .ok_or_else(|| anyhow!("unknown theme: {name}"))?;
    let text = file
        .contents_utf8()
        .ok_or_else(|| anyhow!("bundled theme {name}.toml is not UTF-8"))?;
    parse(name, text)
}

/// The terminal-palette theme, built in code from the 16 ANSI color
/// *names*. This is the editor's historical hard-coded highlight table —
/// keeping it as named colors (rather than RGB) means the terminal
/// renders each with the user's own palette, so the look tracks their
/// terminal theme. Also the startup seed, so the default look is
/// unchanged from before themes existed.
pub fn ansi_theme() -> Theme {
    use Color::*;
    let bold = Modifier::BOLD;
    let italic = Modifier::ITALIC;
    let underline = Modifier::UNDERLINED;

    let mut s: HashMap<String, Style> = HashMap::new();
    let mut put = |scope: &str, style: Style| {
        s.insert(scope.to_string(), style);
    };
    let fg = |c: Color| Style::default().fg(c);

    // Keywords and nvim-treesitter aliases some grammars emit as
    // top-level captures (`include`, `conditional`, …).
    for k in ["keyword", "include", "conditional", "repeat", "exception"] {
        put(k, fg(Magenta));
    }
    put("namespace", fg(Yellow));
    put("parameter", fg(White));
    put("constructor", fg(Yellow));
    put("string", fg(Green));
    put("string.escape", fg(LightGreen));
    put("character", fg(Green));
    put("number", fg(LightRed));
    put("boolean", fg(LightRed));
    put("constant", fg(LightRed));
    put("constant.builtin", fg(LightRed).add_modifier(bold));
    put("comment", fg(DarkGray).add_modifier(italic));
    put("function", fg(LightBlue));
    put("function.macro", fg(LightMagenta));
    put("function.builtin", fg(LightBlue));
    put("method", fg(LightBlue));
    put("type", fg(Yellow));
    put("type.builtin", fg(Magenta).add_modifier(bold));
    // `variable` stays the default fg — left out so lookups return the
    // uncolored style.
    put("variable.parameter", fg(White));
    put("variable.builtin", fg(Cyan).add_modifier(bold));
    put("property", fg(White));
    put("field", fg(White));
    put("label", fg(Yellow));
    put("operator", fg(White));
    put("punctuation.bracket", fg(Gray));
    put("punctuation.delimiter", fg(Gray));
    put("punctuation.special", fg(Yellow));
    put("attribute", fg(LightMagenta));
    put("tag", fg(LightBlue));
    put("markup.heading", fg(LightBlue).add_modifier(bold));
    put("markup.heading.marker", fg(Magenta));
    put("markup.list", fg(DarkGray));
    put("markup.raw", fg(Green));
    put("markup.link.url", fg(Cyan).add_modifier(underline));
    put("markup.link.label", fg(LightBlue));
    put("diff.plus", fg(Green));
    put("diff.minus", fg(Red));
    for (scope, style) in conflict_defaults() {
        put(scope, style);
    }

    Theme {
        name: ANSI.to_string(),
        scopes: s,
    }
}

/// Built-in styles for the synthetic `conflict.*` scopes that
/// `ui::buffer::conflict_captures` emits for git conflict markers. The
/// marker lines get a bold bar; each side gets a dim background tint so
/// the regions read at a glance. Dark RGB fills (like the jump-label
/// colors) so they sit quietly under code on a dark terminal.
///
/// Seeded into *every* theme (see [`super::parse`]), not just `ansi`,
/// because missing a conflict marker is a correctness hazard — a theme
/// can still override any `conflict.*` scope, but never silently drops
/// the highlight by omitting it.
pub(super) fn conflict_defaults() -> [(&'static str, Style); 6] {
    let bar = |bg: Color| {
        Style::default()
            .fg(Color::Rgb(230, 230, 230))
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    };
    [
        ("conflict.marker", bar(Color::Rgb(90, 40, 50))),
        ("conflict.marker.ours", bar(Color::Rgb(36, 72, 46))),
        ("conflict.marker.theirs", bar(Color::Rgb(40, 56, 92))),
        ("conflict.ours", Style::default().bg(Color::Rgb(22, 42, 28))),
        (
            "conflict.theirs",
            Style::default().bg(Color::Rgb(24, 32, 54)),
        ),
        ("conflict.base", Style::default().bg(Color::Rgb(44, 44, 30))),
    ]
}

/// The theme name for a path, if it's a `*.toml`. `themes/foo.toml` → `foo`.
fn theme_stem(path: &Path) -> Option<String> {
    if path.extension()?.to_str()? != "toml" {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_parses() {
        for file in BUILTINS.files() {
            let Some(stem) = theme_stem(file.path()) else {
                continue;
            };
            let text = file.contents_utf8().expect("bundled theme is UTF-8");
            parse(&stem, text).unwrap_or_else(|e| panic!("bundled theme {stem}: {e}"));
        }
    }

    /// Catch palette-reference typos: an unresolvable color is silently
    /// dropped at parse time, so a misspelled palette name wouldn't fail
    /// `parse` — it'd just leave that scope uncolored. Assert the core
    /// scopes every bundled theme defines actually resolved to a color.
    #[test]
    fn bundled_themes_resolve_core_scopes() {
        for file in BUILTINS.files() {
            let Some(stem) = theme_stem(file.path()) else {
                continue;
            };
            let text = file.contents_utf8().unwrap();
            let theme = parse(&stem, text).unwrap();
            for scope in ["keyword", "string", "comment", "function", "type"] {
                assert!(
                    theme.style_for(scope).fg.is_some(),
                    "theme {stem}: scope `{scope}` resolved to no color \
                     (palette typo?)"
                );
            }
            assert!(
                theme.ui_background().is_some(),
                "theme {stem}: ui.background has no bg (palette typo?)"
            );
        }
    }

    #[test]
    fn ansi_is_synthesized_not_a_file() {
        // The terminal-palette theme has no backing file but always loads.
        assert!(BUILTINS.get_file("ansi.toml").is_none());
        assert!(load_by_name(ANSI).is_ok());
        assert!(available().iter().any(|n| n == ANSI));
    }

    #[test]
    fn rgb_theme_is_bundled() {
        assert!(BUILTINS.get_file("catppuccin-mocha.toml").is_some());
        let t = load_by_name("catppuccin-mocha").unwrap();
        // Palette reference resolved to RGB.
        assert!(matches!(
            t.style_for("keyword").fg,
            Some(ratatui::style::Color::Rgb(..))
        ));
    }

    #[test]
    fn unknown_theme_errors() {
        assert!(load_by_name("no-such-theme").is_err());
    }
}
