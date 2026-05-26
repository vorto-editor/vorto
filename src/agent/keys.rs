//! Encode a crossterm [`KeyEvent`] into the byte sequence a terminal
//! application expects on its PTY input. Ported from evelyn's
//! `input.rs` (which encodes winit key events) and adapted to
//! crossterm's [`KeyCode`] / [`KeyModifiers`].
//!
//! The encoding is xterm-flavoured: arrows / Home / End as CSI (`ESC [
//! X`) or SS3 when modified, PageUp/Down/Insert/Delete as `ESC [ N ~`,
//! F-keys as SS3 / `ESC [ 1 ; m P`, and Ctrl/Alt folded into control
//! bytes / an ESC prefix. Bare cursor / Home / End keys switch between
//! CSI (`ESC [ A`) and SS3 (`ESC O A`) form depending on the terminal's
//! DECCKM (application-cursor) mode, read from the live emulator at the
//! call site.

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Translate a key press into bytes to write to the agent's PTY. `mode`
/// is the emulated terminal's current mode (DECCKM etc.), used to pick
/// the CSI-vs-SS3 form of bare cursor keys. Returns `None` when the key
/// carries nothing to forward (e.g. a bare modifier press, or a key
/// release under the kitty protocol).
pub fn encode_key(event: &KeyEvent, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    use crossterm::event::KeyEventKind;
    // Under the kitty keyboard protocol crossterm also reports Release /
    // Repeat events. Only forward presses (and repeats, which are real
    // input); a release would double every keystroke.
    if matches!(event.kind, KeyEventKind::Release) {
        return None;
    }
    let m = event.modifiers;
    let shift = m.contains(KeyModifiers::SHIFT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);

    match event.code {
        KeyCode::Char(c) => encode_char(c, ctrl, alt),
        KeyCode::Up => Some(cursor_letter(b'A', shift, alt, ctrl, app_cursor)),
        KeyCode::Down => Some(cursor_letter(b'B', shift, alt, ctrl, app_cursor)),
        KeyCode::Right => Some(cursor_letter(b'C', shift, alt, ctrl, app_cursor)),
        KeyCode::Left => Some(cursor_letter(b'D', shift, alt, ctrl, app_cursor)),
        KeyCode::Home => Some(cursor_letter(b'H', shift, alt, ctrl, app_cursor)),
        KeyCode::End => Some(cursor_letter(b'F', shift, alt, ctrl, app_cursor)),
        KeyCode::PageUp => Some(csi_tilde(5, shift, alt, ctrl)),
        KeyCode::PageDown => Some(csi_tilde(6, shift, alt, ctrl)),
        KeyCode::Insert => Some(csi_tilde(2, shift, alt, ctrl)),
        KeyCode::Delete => Some(csi_tilde(3, shift, alt, ctrl)),
        KeyCode::F(1) => Some(csi_fkey(b'P', shift, alt, ctrl)),
        KeyCode::F(2) => Some(csi_fkey(b'Q', shift, alt, ctrl)),
        KeyCode::F(3) => Some(csi_fkey(b'R', shift, alt, ctrl)),
        KeyCode::F(4) => Some(csi_fkey(b'S', shift, alt, ctrl)),
        // F5-F12: `ESC [ N ~`. The terminfo `kf5`…`kf12` numbers skip a
        // few values (15, 17-21, 23, 24) for historical reasons.
        KeyCode::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            Some(csi_tilde(code, shift, alt, ctrl))
        }
        KeyCode::Enter => Some(esc_prefix(b"\r", alt)),
        KeyCode::Backspace => Some(esc_prefix(b"\x7f", alt)),
        // Shift+Tab → CSI Z (xterm "backtab" / terminfo kcbt).
        KeyCode::BackTab => Some(esc_prefix(b"\x1b[Z", alt)),
        KeyCode::Tab => Some(esc_prefix(b"\t", alt)),
        KeyCode::Esc => Some(esc_prefix(b"\x1b", alt)),
        _ => None,
    }
}

/// xterm-style modifier code: 1 + shift + 2*alt + 4*ctrl. Used inside
/// `CSI 1; <m> X` / `CSI N; <m> ~` for cursor / nav / function keys.
fn modifier_code(shift: bool, alt: bool, ctrl: bool) -> u8 {
    1u8 + (shift as u8) + 2 * (alt as u8) + 4 * (ctrl as u8)
}

/// Cursor / Home / End. A modified key always uses the CSI form with the
/// xterm modifier code (`ESC [ 1 ; m X`). A bare key uses SS3 (`ESC O X`)
/// when the terminal is in DECCKM application-cursor mode and CSI (`ESC [
/// X`) otherwise — this is what applications like readline / full-screen
/// TUIs expect once they set DECCKM.
fn cursor_letter(letter: u8, shift: bool, alt: bool, ctrl: bool, app_cursor: bool) -> Vec<u8> {
    let m_code = modifier_code(shift, alt, ctrl);
    if m_code > 1 {
        format!("\x1b[1;{}{}", m_code, letter as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', letter]
    } else {
        vec![0x1b, b'[', letter]
    }
}

/// PageUp/Down, Insert, Delete, F5+: `ESC [ N ~` bare, `ESC [ N ; m ~`
/// modded.
fn csi_tilde(n: u8, shift: bool, alt: bool, ctrl: bool) -> Vec<u8> {
    let m_code = modifier_code(shift, alt, ctrl);
    if m_code > 1 {
        format!("\x1b[{};{}~", n, m_code).into_bytes()
    } else {
        format!("\x1b[{}~", n).into_bytes()
    }
}

/// F1-F4: SS3 form (`ESC O P`…) bare, `ESC [ 1 ; m P`… modded.
fn csi_fkey(letter: u8, shift: bool, alt: bool, ctrl: bool) -> Vec<u8> {
    let m_code = modifier_code(shift, alt, ctrl);
    if m_code > 1 {
        format!("\x1b[1;{}{}", m_code, letter as char).into_bytes()
    } else {
        vec![0x1b, b'O', letter]
    }
}

/// Prefix `bytes` with ESC for an Alt/Meta-modified key.
fn esc_prefix(bytes: &[u8], alt: bool) -> Vec<u8> {
    if !alt {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(0x1b);
    out.extend_from_slice(bytes);
    out
}

/// Encode a character key, folding Ctrl into a control byte and Alt
/// into an ESC prefix.
fn encode_char(c: char, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    if ctrl {
        let lower = c.to_ascii_lowercase();
        let byte = match lower {
            'a'..='z' => Some((lower as u8) - b'a' + 1),
            ' ' | '@' => Some(0x00),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' | '?' => Some(0x1f),
            _ => None,
        };
        if let Some(b) = byte {
            out.push(b);
            return Some(out);
        }
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    if out.is_empty() { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// Encode under DECCKM normal mode (the common case).
    fn enc(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
        encode_key(&key(code, mods), TermMode::empty())
    }

    #[test]
    fn plain_char_is_its_utf8() {
        assert_eq!(
            enc(KeyCode::Char('a'), KeyModifiers::NONE),
            Some(b"a".to_vec())
        );
    }

    #[test]
    fn ctrl_letter_maps_to_control_byte() {
        // Ctrl-C = 0x03.
        assert_eq!(
            enc(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(vec![0x03])
        );
    }

    #[test]
    fn alt_char_gets_esc_prefix() {
        assert_eq!(
            enc(KeyCode::Char('b'), KeyModifiers::ALT),
            Some(vec![0x1b, b'b'])
        );
    }

    #[test]
    fn bare_arrows_are_csi() {
        assert_eq!(
            enc(KeyCode::Up, KeyModifiers::NONE),
            Some(vec![0x1b, b'[', b'A'])
        );
        assert_eq!(
            enc(KeyCode::Left, KeyModifiers::NONE),
            Some(vec![0x1b, b'[', b'D'])
        );
    }

    #[test]
    fn bare_arrows_under_app_cursor_are_ss3() {
        // DECCKM set → bare cursor keys use SS3 (`ESC O X`).
        let m = TermMode::APP_CURSOR;
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE), m),
            Some(vec![0x1b, b'O', b'A'])
        );
        assert_eq!(
            encode_key(&key(KeyCode::Left, KeyModifiers::NONE), m),
            Some(vec![0x1b, b'O', b'D'])
        );
        assert_eq!(
            encode_key(&key(KeyCode::Home, KeyModifiers::NONE), m),
            Some(vec![0x1b, b'O', b'H'])
        );
        assert_eq!(
            encode_key(&key(KeyCode::End, KeyModifiers::NONE), m),
            Some(vec![0x1b, b'O', b'F'])
        );
    }

    #[test]
    fn modified_arrow_carries_xterm_modifier_code_regardless_of_mode() {
        // Ctrl+Right → ESC [ 1 ; 5 C, in both normal and app-cursor mode.
        assert_eq!(
            enc(KeyCode::Right, KeyModifiers::CONTROL),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(
            encode_key(
                &key(KeyCode::Right, KeyModifiers::CONTROL),
                TermMode::APP_CURSOR
            ),
            Some(b"\x1b[1;5C".to_vec())
        );
    }

    #[test]
    fn enter_backspace_tab_escape() {
        assert_eq!(
            enc(KeyCode::Enter, KeyModifiers::NONE),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            enc(KeyCode::Backspace, KeyModifiers::NONE),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(enc(KeyCode::Tab, KeyModifiers::NONE), Some(b"\t".to_vec()));
        assert_eq!(
            enc(KeyCode::Esc, KeyModifiers::NONE),
            Some(b"\x1b".to_vec())
        );
    }

    #[test]
    fn backtab_is_csi_z() {
        assert_eq!(
            enc(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn f1_is_ss3() {
        assert_eq!(
            enc(KeyCode::F(1), KeyModifiers::NONE),
            Some(vec![0x1b, b'O', b'P'])
        );
    }

    #[test]
    fn pageup_is_csi_tilde() {
        assert_eq!(
            enc(KeyCode::PageUp, KeyModifiers::NONE),
            Some(b"\x1b[5~".to_vec())
        );
    }
}
