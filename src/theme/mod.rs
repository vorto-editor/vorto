//! Themes: scope-name → terminal style, loaded from Helix-compatible
//! TOML.
//!
//! A [`Theme`] is a flat map from scope name to [`Style`]. Two families
//! of scope share the one namespace and the one longest-prefix-wins
//! lookup:
//!
//! - **syntax** captures, e.g. `function.method`, `keyword.return` —
//!   the tree-sitter highlight names produced by `highlights.scm`. These
//!   resolve through [`Theme::style_for`], which the buffer renderer
//!   calls per capture.
//! - **UI** scopes, e.g. `ui.selection`, `ui.statusline.insert` — chrome
//!   roles modeled on Helix's `ui.*` family. These resolve through the
//!   typed `ui_*` accessors, each of which falls back to a hard-coded
//!   default so a syntax-only theme still renders the UI as before.
//!
//! There is one **active** theme at a time, held in a process global and
//! read at render time on the main thread (workers only ever produce
//! capture *names* — styles are resolved later). The `:theme` picker
//! swaps the active theme for live preview via [`set_active`]; the free
//! function [`style_for`] reads it.

mod builtins;
mod parse;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use ratatui::style::{Color, Modifier, Style};

pub use builtins::{ANSI, available, load_by_name};
pub use parse::parse;

/// A resolved theme: scope name → style. Built by [`parse`] from a
/// Helix-compatible TOML document; never mutated after construction.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Display name — the picker handle and what gets persisted to
    /// `theme = "<name>"`. Derived from the file stem, not the file body.
    pub name: String,
    /// Flat scope → style map. Both `function.method` (syntax) and
    /// `ui.selection` (chrome) live here.
    scopes: HashMap<String, Style>,
}

impl Theme {
    /// Resolve a tree-sitter capture name (e.g. `function.method`) into a
    /// style, trying progressively shorter dotted prefixes so
    /// `function.method` falls back to `function`. Unknown captures
    /// return the default (uncolored) style so the buffer degrades to
    /// plain text. Same contract the old hard-coded `style_for` had.
    pub fn style_for(&self, capture: &str) -> Style {
        self.lookup(capture).unwrap_or_default()
    }

    /// Longest-prefix lookup shared by [`Self::style_for`] and the UI
    /// accessors: try the full scope, then drop the last `.segment` until
    /// something matches or there's nothing left.
    fn lookup(&self, scope: &str) -> Option<Style> {
        let mut candidate = scope;
        loop {
            if let Some(style) = self.scopes.get(candidate) {
                return Some(*style);
            }
            match candidate.rfind('.') {
                Some(i) => candidate = &candidate[..i],
                None => return None,
            }
        }
    }

    /// A UI scope's style, or `fallback` when the theme doesn't define it
    /// (or any shorter prefix). The fallback is the editor's pre-theme
    /// hard-coded look, so a theme that only sets syntax colors keeps the
    /// familiar chrome instead of going blank.
    fn ui(&self, scope: &str, fallback: Style) -> Style {
        self.lookup(scope).unwrap_or(fallback)
    }

    /// Exact-scope lookup with no prefix fallback. For chrome roles where
    /// inheriting a parent scope would be wrong — e.g. a popup *border*
    /// must not pick up the popup *fill*'s bg as its fg.
    fn ui_exact(&self, scope: &str, fallback: Style) -> Style {
        self.scopes.get(scope).copied().unwrap_or(fallback)
    }
}

// ────────────────────────────────────────────────────────────────────────
// UI-scope accessors
//
// Each returns the theme's `ui.*` entry or the editor's historical
// hard-coded style. Centralizing them here makes this the registry of
// "which chrome roles exist and what they look like by default"; call
// sites in `ui/` read these instead of naming `Color::` directly.
// ────────────────────────────────────────────────────────────────────────

impl Theme {
    /// Editor background style (`ui.background`) — `Some` only when the
    /// theme sets a `bg`, which is the signal to paint a fill. Returns
    /// the whole style: the `bg` fills the viewport and the `fg` becomes
    /// the default text color for un-captured text, so plain text stays
    /// readable against a themed background regardless of the terminal's
    /// own fg. `None` (the `ansi` theme, any syntax-only theme) leaves the
    /// terminal's own background and foreground showing.
    pub fn ui_background(&self) -> Option<Style> {
        let style = self.lookup("ui.background")?;
        style.bg.is_some().then_some(style)
    }

    /// Buffer visual-mode selection span. Patched over the cell's style,
    /// so a bg-only entry tints the selection without dropping syntax fg
    /// (matching the pre-theme `SEL_BG`). Reads `ui.selection.primary`,
    /// prefix-falling-back to `ui.selection`.
    pub fn ui_selection(&self) -> Style {
        self.ui("ui.selection.primary", Style::default().bg(Color::DarkGray))
    }

    /// Buffer search-hit background (`hlsearch`).
    pub fn ui_search(&self) -> Style {
        self.ui("ui.search", Style::default().bg(Color::DarkGray))
    }

    /// Gutter line numbers (non-cursor rows).
    pub fn ui_linenr(&self) -> Style {
        self.ui("ui.linenr", Style::default().fg(Color::DarkGray))
    }

    /// Gutter line number for the cursor's row. Defaults to the terminal
    /// foreground (`Reset`) so it tracks the cursor's own color.
    pub fn ui_linenr_selected(&self) -> Style {
        self.ui("ui.linenr.selected", Style::default().fg(Color::Reset))
    }

    /// Statusline fill. Today only the bg is consumed (the mode badge and
    /// position segments set their own fg); a theme may also set fg.
    pub fn ui_statusline(&self) -> Style {
        self.ui("ui.statusline", Style::default().bg(Color::DarkGray))
    }

    /// Modal / popup panel fill. Defaults to the terminal bg (`Reset`).
    pub fn ui_popup(&self) -> Style {
        self.ui("ui.popup", Style::default().bg(Color::Reset))
    }

    /// Modal / popup border. Exact lookup so it never inherits the popup
    /// fill's bg as its fg.
    pub fn ui_popup_border(&self) -> Style {
        self.ui_exact("ui.popup.border", Style::default().fg(Color::Gray))
    }

    /// Selected row in a picker / menu / completion list. Reversed video
    /// by default (terminal-relative), matching the pre-theme look.
    pub fn ui_menu_selected(&self) -> Style {
        self.ui(
            "ui.menu.selected",
            Style::default().add_modifier(Modifier::REVERSED),
        )
    }
}

// ────────────────────────────────────────────────────────────────────────
// Active theme (process global)
// ────────────────────────────────────────────────────────────────────────

fn cell() -> &'static RwLock<Arc<Theme>> {
    static ACTIVE: OnceLock<RwLock<Arc<Theme>>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(Arc::new(builtins::ansi_theme())))
}

/// The currently active theme. Cheap (an `Arc` clone); call it once per
/// render pass and reuse the handle for the whole frame.
pub fn active() -> Arc<Theme> {
    cell().read().unwrap().clone()
}

/// Replace the active theme. Used at startup (apply the configured
/// theme) and by the `:theme` picker for live preview.
pub fn set_active(theme: Theme) {
    *cell().write().unwrap() = Arc::new(theme);
}

/// The active theme's name.
pub fn active_name() -> String {
    cell().read().unwrap().name.clone()
}

/// Resolve a capture name against the active theme. Kept as a free
/// function for back-compat with `syntax::style_for`; render-hot loops
/// should instead hold an [`active`] handle and call
/// [`Theme::style_for`] to avoid re-locking per capture.
pub fn style_for(capture: &str) -> Style {
    active().style_for(capture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_with(pairs: &[(&str, Style)]) -> Theme {
        Theme {
            name: "t".into(),
            scopes: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    #[test]
    fn exact_and_prefix_lookup() {
        let t = theme_with(&[("function", Style::default().fg(Color::Blue))]);
        assert_eq!(t.style_for("function").fg, Some(Color::Blue));
        // falls back to `function`
        assert_eq!(t.style_for("function.method").fg, Some(Color::Blue));
    }

    #[test]
    fn unknown_capture_is_default() {
        let t = theme_with(&[]);
        assert_eq!(t.style_for("nope"), Style::default());
    }

    #[test]
    fn ui_accessor_falls_back_when_absent() {
        let t = theme_with(&[]);
        // No `ui.selection` defined → the hard-coded fallback.
        assert_eq!(t.ui_selection().bg, Some(Color::DarkGray));
    }

    #[test]
    fn ui_accessor_prefers_theme_entry() {
        let t = theme_with(&[("ui.selection", Style::default().bg(Color::Red))]);
        assert_eq!(t.ui_selection().bg, Some(Color::Red));
    }
}
