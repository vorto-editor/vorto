//! Minimal raw renderer for the in-app agent pane (Phase C).
//!
//! This is deliberately crude: it strips escape sequences with a small
//! state machine and shows the tail of the agent's output as plain text,
//! with a header carrying the agent name and an `[exited]` marker when
//! the process ended. There is **no** terminal grid / VT emulation — a
//! proper terminal model replaces this entirely in Phase D.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

/// Draw the agent pane into `area`. Also resizes the agent's PTY to the
/// content rect so the agent reflows to the pane size. `active` tints
/// the border to mark focus.
pub(super) fn draw_agent_pane(f: &mut Frame, app: &App, area: Rect, active: bool) {
    f.render_widget(Clear, area);

    let Some(session) = app.agent.as_ref() else {
        // Pane marked Agent but no process — shouldn't happen, but render
        // something rather than panicking.
        let block = Block::default().borders(Borders::ALL).title(" agent ");
        f.render_widget(block, area);
        return;
    };

    let title = if session.exited() {
        format!(" agent: {} [exited] ", session.name)
    } else {
        format!(" agent: {} ", session.name)
    };
    let border_style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Resize the PTY to the content rect so the agent lays out to what's
    // visible. Cheap; called every frame the pane is drawn.
    session.resize(inner.width, inner.height);

    // Crudely strip escape sequences and turn the raw bytes into lines.
    let text = strip_escapes(session.output());
    let rows = inner.height as usize;
    let cols = inner.width as usize;
    // Show the tail: the last `rows` display lines, each truncated to
    // `cols`. Phase D's grid will track the cursor and scrollback
    // properly; this just keeps the most recent output visible.
    let all_lines: Vec<&str> = text.lines().collect();
    let start = all_lines.len().saturating_sub(rows);
    let lines: Vec<Line> = all_lines[start..]
        .iter()
        .map(|l| {
            let truncated: String = l.chars().take(cols).collect();
            Line::from(Span::raw(truncated))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

/// Strip ANSI/VT escape sequences from `bytes`, returning printable
/// text. A small state machine: CSI (`ESC [ … final`), OSC (`ESC ] …
/// BEL/ST`), and other `ESC <byte>` forms are dropped; carriage returns
/// are normalized so a `\r`-overwrite doesn't smear onto one line.
/// Lossy and approximate — good enough to read the agent's output until
/// the Phase D emulator lands.
fn strip_escapes(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                match chars.peek() {
                    Some('[') => {
                        // CSI: consume until a final byte in 0x40..=0x7e.
                        chars.next();
                        for d in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&d) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        // OSC: consume until BEL or ESC \ (ST).
                        chars.next();
                        while let Some(d) = chars.next() {
                            if d == '\x07' {
                                break;
                            }
                            if d == '\x1b' {
                                // Possible ST terminator `ESC \`.
                                if chars.peek() == Some(&'\\') {
                                    chars.next();
                                }
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        // Two-char escape (e.g. `ESC (B`); drop the next.
                        chars.next();
                    }
                    None => {}
                }
            }
            // Drop carriage return; the simple tail renderer treats `\n`
            // as the only line break (a bare `\r` cursor-return would
            // otherwise show as a control glyph).
            '\r' => {}
            // Drop other C0 control bytes except newline/tab.
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_color_sequences() {
        let raw = b"\x1b[31mred\x1b[0m text";
        assert_eq!(strip_escapes(raw), "red text");
    }

    #[test]
    fn strips_osc_title() {
        let raw = b"\x1b]0;my title\x07hello";
        assert_eq!(strip_escapes(raw), "hello");
    }

    #[test]
    fn drops_carriage_returns_keeps_newlines() {
        let raw = b"line1\r\nline2";
        assert_eq!(strip_escapes(raw), "line1\nline2");
    }
}
