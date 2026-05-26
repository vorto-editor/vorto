//! Helix-compatible theme TOML → [`Theme`].
//!
//! The format mirrors Helix's themes so existing `.toml` theme files
//! drop in with little or no change:
//!
//! ```toml
//! # An optional named-color palette. Values are hex or ANSI names.
//! [palette]
//! bg    = "#1e1e2e"
//! mauve = "#cba6f7"
//!
//! # Every other top-level key is a scope. A bare string sets the
//! # foreground; a table sets fg/bg/modifiers. Color values are a
//! # palette name, a `#rrggbb` hex literal, or an ANSI color name.
//! keyword           = "mauve"
//! "function.macro"  = { fg = "mauve", modifiers = ["bold"] }
//! comment           = { fg = "#6c7086", modifiers = ["italic"] }
//! "ui.selection"    = { bg = "#313244" }
//! ```
//!
//! Unknown color names and unknown modifier names are skipped (the rest
//! of the entry still applies) rather than failing the whole load — a
//! theme with one typo'd color shouldn't blank the editor. A malformed
//! *document* (not valid TOML) is a hard error.

use std::collections::HashMap;

use anyhow::{Context, Result};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

use super::Theme;

/// One scope's value: either a bare color string (foreground only) or a
/// `{ fg, bg, modifiers }` table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Entry {
    Color(String),
    Style {
        #[serde(default)]
        fg: Option<String>,
        #[serde(default)]
        bg: Option<String>,
        #[serde(default)]
        modifiers: Vec<String>,
    },
}

/// Parse a Helix-style theme document. `name` becomes [`Theme::name`]
/// (derived from the file stem by the caller, not read from the body).
pub fn parse(name: &str, text: &str) -> Result<Theme> {
    // Parse into a generic table first: the `[palette]` sub-table needs
    // to be pulled out and resolved before the scope entries can name
    // its colors.
    let mut table: HashMap<String, toml::Value> =
        toml::from_str(text).with_context(|| format!("parsing theme `{name}`"))?;

    let palette = match table.remove("palette") {
        Some(v) => v
            .try_into::<HashMap<String, String>>()
            .with_context(|| format!("theme `{name}`: [palette] must be name → color"))?,
        None => HashMap::new(),
    };

    let mut scopes = HashMap::with_capacity(table.len());
    for (scope, value) in table {
        let entry: Entry = value
            .try_into()
            .with_context(|| format!("theme `{name}`: scope `{scope}`"))?;
        if let Some(style) = entry.into_style(&palette) {
            scopes.insert(scope, style);
        }
    }

    Ok(Theme {
        name: name.to_string(),
        scopes,
    })
}

impl Entry {
    /// Resolve to a [`Style`], or `None` when the entry yields nothing
    /// usable (e.g. a single unresolvable color string).
    fn into_style(self, palette: &HashMap<String, String>) -> Option<Style> {
        let mut style = Style::default();
        match self {
            Entry::Color(c) => {
                style = style.fg(resolve_color(&c, palette)?);
            }
            Entry::Style { fg, bg, modifiers } => {
                if let Some(c) = fg.as_deref().and_then(|c| resolve_color(c, palette)) {
                    style = style.fg(c);
                }
                if let Some(c) = bg.as_deref().and_then(|c| resolve_color(c, palette)) {
                    style = style.bg(c);
                }
                for m in &modifiers {
                    if let Some(modifier) = parse_modifier(m) {
                        style = style.add_modifier(modifier);
                    }
                }
                // An entry with no resolvable fg/bg/modifier carries no
                // information — drop it so `lookup` keeps falling back to
                // a shorter prefix instead of stopping at an empty style.
                if style == Style::default() {
                    return None;
                }
            }
        }
        Some(style)
    }
}

/// Resolve a color token: a `[palette]` name, a `#rrggbb`/`#rgb` hex
/// literal, or an ANSI color name. Unknown → `None`.
fn resolve_color(token: &str, palette: &HashMap<String, String>) -> Option<Color> {
    // Palette names take priority so a theme can shadow an ANSI name.
    if let Some(hex) = palette.get(token) {
        return parse_literal(hex);
    }
    parse_literal(token)
}

/// A non-palette color literal: hex or ANSI name.
fn parse_literal(s: &str) -> Option<Color> {
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    parse_ansi(s)
}

fn parse_hex(hex: &str) -> Option<Color> {
    let full = match hex.len() {
        // `#rgb` shorthand → expand each nibble (`f0a` → `ff00aa`).
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_string(),
        _ => return None,
    };
    let r = u8::from_str_radix(&full[0..2], 16).ok()?;
    let g = u8::from_str_radix(&full[2..4], 16).ok()?;
    let b = u8::from_str_radix(&full[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// ANSI / named colors. Accepts the 16 standard names in the spellings
/// both Helix and ratatui use (`gray`/`grey`, `light*`/`bright *`).
fn parse_ansi(name: &str) -> Option<Color> {
    let c = match name.to_ascii_lowercase().replace([' ', '-', '_'], "") {
        s if s == "black" => Color::Black,
        s if s == "red" => Color::Red,
        s if s == "green" => Color::Green,
        s if s == "yellow" => Color::Yellow,
        s if s == "blue" => Color::Blue,
        s if s == "magenta" || s == "purple" => Color::Magenta,
        s if s == "cyan" => Color::Cyan,
        s if s == "gray" || s == "grey" || s == "white" => return named_gray(&s),
        s if s == "darkgray" || s == "darkgrey" || s == "brightblack" => Color::DarkGray,
        s if s == "lightred" || s == "brightred" => Color::LightRed,
        s if s == "lightgreen" || s == "brightgreen" => Color::LightGreen,
        s if s == "lightyellow" || s == "brightyellow" => Color::LightYellow,
        s if s == "lightblue" || s == "brightblue" => Color::LightBlue,
        s if s == "lightmagenta" || s == "brightmagenta" => Color::LightMagenta,
        s if s == "lightcyan" || s == "brightcyan" => Color::LightCyan,
        s if s == "lightgray" || s == "lightgrey" || s == "brightwhite" => Color::White,
        _ => return None,
    };
    Some(c)
}

/// `gray`/`grey` map to ratatui's `Gray`; `white` to `White`. Split out
/// because the `match` arm above shares a guard.
fn named_gray(s: &str) -> Option<Color> {
    Some(if s == "white" {
        Color::White
    } else {
        Color::Gray
    })
}

fn parse_modifier(name: &str) -> Option<Modifier> {
    let m = match name.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "bold" => Modifier::BOLD,
        "dim" => Modifier::DIM,
        "italic" => Modifier::ITALIC,
        "underlined" | "underline" => Modifier::UNDERLINED,
        "slow_blink" | "blink" => Modifier::SLOW_BLINK,
        "rapid_blink" => Modifier::RAPID_BLINK,
        "reversed" | "reverse" => Modifier::REVERSED,
        "hidden" => Modifier::HIDDEN,
        "crossed_out" | "strikethrough" => Modifier::CROSSED_OUT,
        _ => return None,
    };
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_shorthand() {
        assert_eq!(parse_literal("#ff00aa"), Some(Color::Rgb(255, 0, 170)));
        assert_eq!(parse_literal("#f0a"), Some(Color::Rgb(255, 0, 170)));
        assert_eq!(parse_literal("#zzz"), None);
    }

    #[test]
    fn palette_reference_resolves() {
        let mut pal = HashMap::new();
        pal.insert("mauve".to_string(), "#cba6f7".to_string());
        assert_eq!(
            resolve_color("mauve", &pal),
            Some(Color::Rgb(0xcb, 0xa6, 0xf7))
        );
        // Non-palette token still parses as a literal.
        assert_eq!(resolve_color("red", &pal), Some(Color::Red));
    }

    #[test]
    fn bare_string_sets_fg() {
        let t = parse("t", r##"keyword = "#ff0000""##).unwrap();
        assert_eq!(t.style_for("keyword").fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn table_with_modifiers() {
        let text = r##"comment = { fg = "#808080", modifiers = ["italic", "bold"] }"##;
        let t = parse("t", text).unwrap();
        let s = t.style_for("comment");
        assert_eq!(s.fg, Some(Color::Rgb(0x80, 0x80, 0x80)));
        assert!(s.add_modifier.contains(Modifier::ITALIC));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn palette_block_is_not_a_scope() {
        // Scope keys must precede the `[palette]` header — a TOML table
        // header captures every key after it (this is why theme files put
        // the palette last).
        let text = r##"
keyword = "fg"

[palette]
fg = "#abcdef"
"##;
        let t = parse("t", text).unwrap();
        // `palette` itself is not a scope.
        assert_eq!(t.style_for("palette"), Style::default());
        assert_eq!(
            t.style_for("keyword").fg,
            Some(Color::Rgb(0xab, 0xcd, 0xef))
        );
    }

    #[test]
    fn unknown_color_is_skipped_not_fatal() {
        let t = parse("t", r#"keyword = "chartreuse""#).unwrap();
        // Unresolvable → no entry, falls through to default.
        assert_eq!(t.style_for("keyword"), Style::default());
    }

    #[test]
    fn malformed_toml_errors() {
        assert!(parse("t", "this is = = not toml").is_err());
    }
}
