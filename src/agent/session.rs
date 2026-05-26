//! The single in-app agent process, backed by a PTY and emulated with a
//! real terminal grid.
//!
//! Phase D scope: spawn the agent under a pseudo-terminal, stream its
//! output to the main loop as [`crate::event::AppEvent::AgentOutput`]
//! events, and feed those bytes through an `alacritty_terminal` VT parser
//! into a [`Term`] grid that the renderer paints with colors + cursor.
//! Keystrokes are forwarded by writing to the PTY. Device-query replies
//! the emulator emits ([`Event::PtyWrite`]) are written back to the PTY
//! from the [`EventListener`].

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape as TermCursorShape, Processor,
};
use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::agent::AgentSpec;
use crate::event::AppEvent;

/// `TERM` advertised to the agent. 256-color xterm is the safe default
/// every modern CLI agent understands; truecolor is opted into via
/// `COLORTERM` below.
const TERM_ENV: &str = "xterm-256color";

/// Shared PTY writer handle: behind a mutex so input forwarding,
/// device-query replies (from the emulator's event listener), and the
/// reader thread can all touch it through shared references.
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Dimensions of the emulated terminal. Implements `alacritty_terminal`'s
/// [`Dimensions`] trait so it can size a [`Term`] and drive `resize`.
/// `total_lines` includes the scrollback history; `screen_lines` is the
/// visible viewport.
#[derive(Debug, Clone, Copy)]
struct TermSize {
    cols: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Minimal [`EventListener`] for the emulated terminal. Device-status /
/// cursor-position queries (DA, DSR, …) ask the terminal to *reply* on
/// the PTY input; the emulator surfaces those as [`TermEvent::PtyWrite`],
/// which we write straight back to the agent. The most recent window
/// title is captured so the status line can show it. Everything else
/// (bell, clipboard, …) is ignored for now.
#[derive(Clone)]
struct AgentEventListener {
    writer: SharedWriter,
    title: Arc<Mutex<Option<String>>>,
}

impl EventListener for AgentEventListener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => {
                if let Ok(mut w) = self.writer.lock() {
                    let _ = w.write_all(text.as_bytes());
                    let _ = w.flush();
                }
            }
            TermEvent::Title(title) => {
                if let Ok(mut t) = self.title.lock() {
                    *t = Some(title);
                }
            }
            TermEvent::ResetTitle => {
                if let Ok(mut t) = self.title.lock() {
                    *t = None;
                }
            }
            _ => {}
        }
    }
}

/// The VT emulator: a parser plus the terminal grid it drives. Held
/// behind a mutex inside [`AgentSession`] so the parse path (`&mut App`),
/// the renderer (`&App`), and resize (`&App`) can all reach it through a
/// shared reference.
struct Emu {
    processor: Processor,
    term: Term<AgentEventListener>,
    size: TermSize,
}

/// A snapshot of one visible grid cell, decoupled from
/// `alacritty_terminal` types so the renderer doesn't hold the emulator
/// lock while building ratatui spans.
pub struct GridCell {
    pub row: usize,
    pub col: usize,
    pub c: char,
    pub fg: AnsiColor,
    pub bg: AnsiColor,
    pub flags: Flags,
}

/// What the renderer needs to paint a frame of the agent terminal: the
/// visible cells plus cursor position / visibility. (The cursor *shape*
/// is read separately by the main loop via
/// [`AgentSession::cursor_shape`] for DECSCUSR.)
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<GridCell>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
}

/// A live agent process attached to a PTY. One per `App`; the pane that
/// displays it can come and go (`:agent` re-attaches a pane) without the
/// process dying. Killed only when vorto exits.
pub struct AgentSession {
    /// Display name of the agent (for the pane header / status line).
    pub name: String,
    /// PTY master, kept so we can resize it as the pane rect changes.
    master: Box<dyn MasterPty + Send>,
    /// Writer half of the PTY, behind a mutex so `write(&self, …)`
    /// works through a shared reference (the input path holds `&App`
    /// fields disjointly). Also shared with the emulator's event
    /// listener so device-query replies go back out the same channel.
    writer: SharedWriter,
    /// The spawned child. Killed on [`Drop`] / [`Self::kill`].
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// The VT emulator (parser + grid). Behind a mutex for interior
    /// mutability: parse mutates it, render reads it, resize mutates it,
    /// all reachable through `&self`.
    emu: Mutex<Emu>,
    /// The agent's most recently set window title (OSC 0/2), shared with
    /// the event listener. `None` until the agent sets one.
    title: Arc<Mutex<Option<String>>>,
}

impl AgentSession {
    /// Spawn `spec.command` + `spec.args` under a fresh PTY rooted at
    /// `cwd`. The reader thread streams output to the main loop via
    /// `event_tx`; on EOF it sends [`AppEvent::AgentExited`].
    pub fn spawn(
        spec: &AgentSpec,
        cwd: &Path,
        event_tx: std::sync::mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        // Start at a reasonable default; the renderer resizes us to the
        // real pane rect on the first frame.
        const INIT_COLS: u16 = 80;
        const INIT_ROWS: u16 = 24;

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: INIT_ROWS,
            cols: INIT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(&spec.command);
        for a in &spec.args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", TERM_ENV);
        // Agents gate 24-bit color on COLORTERM rather than terminfo.
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd)?;
        // Drop the slave handle so the only thing keeping the PTY open is
        // the child; when it exits the reader sees EOF.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if event_tx
                            .send(AppEvent::AgentOutput(buf[..n].to_vec()))
                            .is_err()
                        {
                            // Main loop is gone — nothing left to feed.
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = event_tx.send(AppEvent::AgentExited);
        });

        let title = Arc::new(Mutex::new(None));
        let listener = AgentEventListener {
            writer: Arc::clone(&writer),
            title: Arc::clone(&title),
        };
        let size = TermSize {
            cols: INIT_COLS as usize,
            screen_lines: INIT_ROWS as usize,
        };
        let term = Term::new(TermConfig::default(), &size, listener);
        let emu = Mutex::new(Emu {
            processor: Processor::new(),
            term,
            size,
        });

        Ok(Self {
            name: spec.name.clone(),
            master: pair.master,
            writer,
            child,
            emu,
            title,
        })
    }

    /// Write `bytes` to the agent's PTY input. Takes `&self` so the
    /// input dispatcher can forward keys while holding other `&mut App`
    /// field borrows; the writer is internally synchronised. Errors are
    /// swallowed — a dead PTY just drops the keystroke (the reader
    /// thread will surface the exit separately).
    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    /// Feed a chunk read from the PTY through the VT parser into the
    /// terminal grid. Called by the main loop on each
    /// [`AppEvent::AgentOutput`]. Device-query replies are written back
    /// to the PTY from the event listener as a side effect.
    pub fn push_output(&self, chunk: &[u8]) {
        if let Ok(mut emu) = self.emu.lock() {
            let Emu {
                processor, term, ..
            } = &mut *emu;
            processor.advance(term, chunk);
        }
    }

    /// The current terminal keyboard mode (DECCKM / app-keypad / …).
    /// Read at the input call site so `encode_key` can pick CSI vs SS3
    /// cursor-key forms.
    pub fn term_mode(&self) -> TermMode {
        self.emu
            .lock()
            .map(|e| *e.term.mode())
            .unwrap_or_else(|_| TermMode::empty())
    }

    /// Build a render snapshot of the visible grid: every visible cell
    /// plus the cursor. Returns `None` only if the lock is poisoned.
    pub fn grid_snapshot(&self) -> Option<GridSnapshot> {
        let emu = self.emu.lock().ok()?;
        let content = emu.term.renderable_content();
        let cols = emu.size.cols;
        let rows = emu.size.screen_lines;

        let mut cells: Vec<GridCell> = Vec::new();
        for indexed in content.display_iter {
            let point: Point = indexed.point;
            // `display_iter` yields only the visible viewport, but guard
            // against any out-of-range row/col so indexing the ratatui
            // buffer downstream stays in bounds.
            if point.line.0 < 0 {
                continue;
            }
            let row = point.line.0 as usize;
            let col = point.column.0;
            if row >= rows || col >= cols {
                continue;
            }
            let cell: &Cell = indexed.cell;
            // Skip the trailing half of a wide character and empty cells
            // with no styling — they'd just paint default-on-default.
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            cells.push(GridCell {
                row,
                col,
                c: cell.c,
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
            });
        }

        let cursor = content.cursor;
        let cursor_row = cursor.point.line.0.max(0) as usize;
        let cursor_col = cursor.point.column.0;
        let cursor_visible = !matches!(cursor.shape, TermCursorShape::Hidden)
            && cursor_row < rows
            && cursor_col < cols;

        Some(GridSnapshot {
            cols,
            rows,
            cells,
            cursor_row,
            cursor_col,
            cursor_visible,
        })
    }

    /// The agent's window title, if it has set one (OSC 0/2).
    pub fn title(&self) -> Option<String> {
        self.title.lock().ok().and_then(|t| t.clone())
    }

    /// The emulated terminal's cursor shape, mapped to vorto's
    /// [`crate::config::CursorShape`]. Used by the main loop to set the
    /// host terminal's cursor (DECSCUSR) while the agent pane is focused.
    /// A hidden grid cursor / poisoned lock falls back to `Block`.
    pub fn cursor_shape(&self) -> crate::config::CursorShape {
        use crate::config::CursorShape;
        let shape = self
            .emu
            .lock()
            .ok()
            .map(|e| e.term.cursor_style().shape)
            .unwrap_or(TermCursorShape::Block);
        match shape {
            TermCursorShape::Beam => CursorShape::Bar,
            TermCursorShape::Underline => CursorShape::Underbar,
            // Block / HollowBlock / Hidden → a solid block; Hidden is
            // handled separately by cursor visibility in the renderer.
            _ => CursorShape::Block,
        }
    }

    /// Resize the emulated terminal *and* the PTY to `cols` x `rows`.
    /// Called by the renderer when the agent pane's rectangle changes so
    /// both the grid and the agent reflow. A no-op when the size is
    /// unchanged so it's cheap to call every frame.
    pub fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if let Ok(mut emu) = self.emu.lock() {
            if emu.size.cols == cols as usize && emu.size.screen_lines == rows as usize {
                return;
            }
            let size = TermSize {
                cols: cols as usize,
                screen_lines: rows as usize,
            };
            emu.term.resize(size);
            emu.size = size;
        }
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Kill the child process. Idempotent and best-effort — used on the
    /// quit path and by [`Drop`].
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentSpec;
    use std::time::Duration;

    fn cat_spec() -> AgentSpec {
        // `cat` stays alive on the PTY reading stdin — a stable stand-in
        // for an interactive agent. A PTY echoes input back, so writing
        // produces readable output without the program doing anything.
        AgentSpec {
            name: "cat".into(),
            command: "cat".into(),
            args: vec![],
            prompt_args: vec![],
        }
    }

    #[cfg(unix)]
    #[test]
    fn spawn_streams_pty_echo_and_kills_cleanly() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap();
        let mut session =
            AgentSession::spawn(&cat_spec(), &cwd, tx).expect("cat spawns under a pty");

        // The PTY echoes what we write; feed output events through the
        // emulator until our marker lands in the grid or we time out.
        session.write(b"vorto-marker\n");
        let mut saw_marker = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(AppEvent::AgentOutput(bytes)) => {
                    session.push_output(&bytes);
                    let snap = session.grid_snapshot().unwrap();
                    let text: String = grid_text(&snap);
                    if text.contains("vorto-marker") {
                        saw_marker = true;
                        break;
                    }
                }
                Ok(AppEvent::AgentExited) => break,
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
        assert!(
            saw_marker,
            "expected the PTY to echo our marker into the grid"
        );

        // Killing reaps the child; a subsequent kill is harmless.
        session.kill();
        session.kill();
    }

    /// Flatten a grid snapshot into a row-major string (for assertions).
    fn grid_text(snap: &GridSnapshot) -> String {
        let mut rows = vec![vec![' '; snap.cols]; snap.rows];
        for cell in &snap.cells {
            if cell.row < snap.rows && cell.col < snap.cols {
                rows[cell.row][cell.col] = cell.c;
            }
        }
        rows.into_iter()
            .map(|r| r.into_iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parser_writes_text_into_the_grid() {
        // Drive the emulator directly without a process: feed plain text
        // plus a CSI cursor move and assert it lands in the right cell.
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap();
        let Ok(mut session) = AgentSession::spawn(&cat_spec(), &cwd, tx) else {
            // `cat` not available — skip rather than fail.
            return;
        };
        session.resize(20, 5);
        // "hi" at the origin, then move to row 2 col 3 (CSI 2;3 H) "X".
        session.push_output(b"hi\x1b[2;3HX");
        let snap = session.grid_snapshot().unwrap();
        let text = grid_text(&snap);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("hi"), "row 0 = {:?}", lines[0]);
        // CSI row/col are 1-based: row 2 col 3 → grid row 1, col 2.
        assert_eq!(lines[1].as_bytes()[2], b'X', "row 1 = {:?}", lines[1]);
        session.kill();
    }

    #[test]
    fn sgr_red_foreground_lands_on_the_cell() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap();
        let Ok(mut session) = AgentSession::spawn(&cat_spec(), &cwd, tx) else {
            return;
        };
        session.resize(20, 3);
        // SGR 31 = red foreground; "R"; SGR 0 reset.
        session.push_output(b"\x1b[31mR\x1b[0m");
        let snap = session.grid_snapshot().unwrap();
        let r = snap
            .cells
            .iter()
            .find(|c| c.c == 'R')
            .expect("R cell present");
        assert_eq!(
            r.fg,
            AnsiColor::Named(alacritty_terminal::vte::ansi::NamedColor::Red)
        );
        session.kill();
    }

    #[test]
    fn decckm_mode_tracks_app_cursor() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap();
        let Ok(mut session) = AgentSession::spawn(&cat_spec(), &cwd, tx) else {
            return;
        };
        assert!(!session.term_mode().contains(TermMode::APP_CURSOR));
        // DECCKM set: CSI ? 1 h.
        session.push_output(b"\x1b[?1h");
        assert!(session.term_mode().contains(TermMode::APP_CURSOR));
        // DECCKM reset: CSI ? 1 l.
        session.push_output(b"\x1b[?1l");
        assert!(!session.term_mode().contains(TermMode::APP_CURSOR));
        session.kill();
    }
}
