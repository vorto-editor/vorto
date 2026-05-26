//! Terminal-grid renderer for the in-app agent pane (Phase D).
//!
//! Replaces Phase C's crude escape-stripping tail renderer with real VT
//! emulation: [`crate::agent::AgentSession`] feeds PTY output through an
//! `alacritty_terminal` parser into a grid, and this module paints that
//! grid into the pane's ratatui [`Rect`] with colors, text attributes,
//! and the terminal cursor. Full-screen TUI agents (e.g. `claude`) lay
//! out and redraw correctly because they're driving a true terminal.
//!
//! The agent owns the whole rect — no border or header, so the agent's
//! own chrome gets every cell. An `[exited]` badge overlays the top-right
//! corner once the process ends.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Clear;

use crate::agent::GridSnapshot;
use crate::app::App;

/// Draw the agent pane into `area`. Resizes the emulated terminal + PTY
/// to the rect first so the agent reflows, then paints the grid cell by
/// cell. `active` is unused for chrome (the agent owns the rect) but kept
/// for parity with sibling renderers and the focus-ring drawn elsewhere.
pub(super) fn draw_agent_pane(f: &mut Frame, app: &App, area: Rect, _active: bool) {
    f.render_widget(Clear, area);

    let Some(session) = app.agent.as_ref() else {
        // Pane marked Agent but no process — shouldn't happen, but render
        // something rather than panicking.
        f.render_widget(
            Span::styled(" agent ", Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    };

    if area.width == 0 || area.height == 0 {
        return;
    }

    // Reflow the emulator + PTY to the visible rect. Cheap (a no-op when
    // the size is unchanged), called every frame the pane is drawn.
    session.resize(area.width, area.height);

    let Some(snap) = session.grid_snapshot() else {
        return;
    };

    // Clamp to whichever is smaller — the rect we drew into or the grid
    // the snapshot came from — so a race between resize and snapshot
    // can't write outside the pane or read past the grid.
    let max_rows = (area.height as usize).min(snap.rows);
    let max_cols = (area.width as usize).min(snap.cols);
    let buf = f.buffer_mut();
    for cell in &snap.cells {
        if cell.row >= max_rows || cell.col >= max_cols {
            continue;
        }
        let x = area.x + cell.col as u16;
        let y = area.y + cell.row as u16;
        let style = cell_style(cell.fg, cell.bg, cell.flags);
        // A space with default styling is left as the cleared cell; any
        // glyph or styled cell is painted.
        let target = &mut buf[(x, y)];
        let mut s = [0u8; 4];
        target.set_symbol(cell.c.encode_utf8(&mut s));
        target.set_style(style);
    }

    // Exited badge in the top-right corner.
    if session.exited() {
        let badge = " [exited] ";
        let bw = badge.len() as u16;
        if area.width >= bw {
            let x = area.x + area.width - bw;
            f.render_widget(
                Span::styled(
                    badge,
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ),
                Rect {
                    x,
                    y: area.y,
                    width: bw,
                    height: 1,
                },
            );
        }
    }
}

/// Place the terminal cursor at the agent grid's cursor when the agent
/// pane is focused. Called from the top-level UI orchestrator after the
/// frame is painted (only the focused pane gets a hardware cursor).
pub(super) fn place_agent_cursor(f: &mut Frame, snap: &GridSnapshot, area: Rect) {
    if !snap.cursor_visible {
        return;
    }
    let max_rows = (area.height as usize).min(snap.rows);
    let max_cols = (area.width as usize).min(snap.cols);
    if snap.cursor_col >= max_cols || snap.cursor_row >= max_rows {
        return;
    }
    let x = area.x + snap.cursor_col as u16;
    let y = area.y + snap.cursor_row as u16;
    f.set_cursor_position((x, y));
}

/// Map a cell's fg/bg colors and attribute flags to a ratatui [`Style`].
/// Inverse swaps fg/bg; bold/italic/dim/underline/strikeout map to the
/// equivalent ratatui modifiers; hidden blanks the cell out.
fn cell_style(fg: AnsiColor, bg: AnsiColor, flags: Flags) -> Style {
    let mut fg = map_color(fg);
    let mut bg = map_color(bg);

    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut style = Style::default();
    // `Color::Reset` means "use the terminal default"; leaving it off the
    // style entirely lets ratatui's own default through, but setting it
    // explicitly is harmless and keeps inverse swaps coherent.
    style = style.fg(fg).bg(bg);

    let mut modifier = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    if flags.contains(Flags::DIM) {
        modifier |= Modifier::DIM;
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        modifier |= Modifier::UNDERLINED;
    }
    if flags.contains(Flags::STRIKEOUT) {
        modifier |= Modifier::CROSSED_OUT;
    }
    if flags.contains(Flags::HIDDEN) {
        modifier |= Modifier::HIDDEN;
    }
    style.add_modifier(modifier)
}

/// Map an `alacritty_terminal` cell color to a ratatui [`Color`].
///
/// - `Named` base-16 colors → the matching ANSI [`Color`] so the host
///   terminal's palette applies.
/// - `Named` foreground/background/cursor and the dim/bright synonyms →
///   `Reset` (defer to the terminal) or the nearest ANSI color.
/// - `Indexed(n)` → `Color::Indexed(n)`.
/// - `Spec(rgb)` → `Color::Rgb`.
fn map_color(c: AnsiColor) -> Color {
    match c {
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(n) => Color::Indexed(n),
        AnsiColor::Named(named) => map_named(named),
    }
}

/// Map a [`NamedColor`] to a ratatui [`Color`]. The base 16 ANSI colors
/// map one-to-one; the dim variants fold onto their base ANSI color (dim
/// is conveyed via the DIM modifier on the style); the special
/// foreground/background/cursor entries defer to the terminal default via
/// `Reset`.
fn map_named(c: NamedColor) -> Color {
    use NamedColor::*;
    match c {
        Black | DimBlack => Color::Black,
        Red | DimRed => Color::Red,
        Green | DimGreen => Color::Green,
        Yellow | DimYellow => Color::Yellow,
        Blue | DimBlue => Color::Blue,
        Magenta | DimMagenta => Color::Magenta,
        Cyan | DimCyan => Color::Cyan,
        White | DimWhite => Color::Gray,
        BrightBlack => Color::DarkGray,
        BrightRed => Color::LightRed,
        BrightGreen => Color::LightGreen,
        BrightYellow => Color::LightYellow,
        BrightBlue => Color::LightBlue,
        BrightMagenta => Color::LightMagenta,
        BrightCyan => Color::LightCyan,
        BrightWhite => Color::White,
        // Defer the special slots to the host terminal's defaults.
        Foreground | BrightForeground | DimForeground | Background | Cursor => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_base_colors_map_to_ansi() {
        assert_eq!(map_named(NamedColor::Red), Color::Red);
        assert_eq!(map_named(NamedColor::BrightBlue), Color::LightBlue);
        assert_eq!(map_named(NamedColor::White), Color::Gray);
        assert_eq!(map_named(NamedColor::BrightWhite), Color::White);
    }

    #[test]
    fn special_slots_defer_to_terminal() {
        assert_eq!(map_named(NamedColor::Foreground), Color::Reset);
        assert_eq!(map_named(NamedColor::Background), Color::Reset);
    }

    #[test]
    fn indexed_and_rgb_pass_through() {
        assert_eq!(map_color(AnsiColor::Indexed(200)), Color::Indexed(200));
        assert_eq!(
            map_color(AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
                r: 1,
                g: 2,
                b: 3
            })),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn inverse_swaps_fg_and_bg() {
        let style = cell_style(
            AnsiColor::Named(NamedColor::Red),
            AnsiColor::Named(NamedColor::Blue),
            Flags::INVERSE,
        );
        assert_eq!(style.fg, Some(Color::Blue));
        assert_eq!(style.bg, Some(Color::Red));
    }

    #[test]
    fn bold_italic_flags_map_to_modifiers() {
        let style = cell_style(
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Named(NamedColor::Background),
            Flags::BOLD | Flags::ITALIC,
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }
}
