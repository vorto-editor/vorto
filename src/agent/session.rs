//! The single in-app agent process, backed by a PTY.
//!
//! Phase C scope: spawn the agent under a pseudo-terminal, stream its
//! output to the main loop as [`crate::event::AppEvent::AgentOutput`]
//! events, forward keystrokes by writing to the PTY, and keep a raw
//! byte buffer the renderer shows crudely. There is **no** VT / escape
//! sequence emulation or grid yet — that is Phase D, which will replace
//! [`AgentSession::output`] / the renderer with a proper terminal model.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::agent::AgentSpec;
use crate::event::AppEvent;

/// `TERM` advertised to the agent. 256-color xterm is the safe default
/// every modern CLI agent understands; truecolor is opted into via
/// `COLORTERM` below.
const TERM_ENV: &str = "xterm-256color";

/// Cap on the retained raw-output buffer. The Phase C renderer only
/// shows the tail, so there's no reason to grow unbounded under a chatty
/// agent — keep the most recent chunk and drop the front. Phase D's grid
/// will supersede this entirely.
const OUTPUT_CAP: usize = 256 * 1024;

/// A live agent process attached to a PTY. One per `App`; the pane that
/// displays it can come and go (`:agent` re-attaches a pane) without the
/// process dying. Killed only when vorto exits.
pub struct AgentSession {
    /// Display name of the agent (for the pane header).
    pub name: String,
    /// PTY master, kept so we can resize it as the pane rect changes.
    master: Box<dyn MasterPty + Send>,
    /// Writer half of the PTY, behind a mutex so `write(&self, …)`
    /// works through a shared reference (the input path holds `&App`
    /// fields disjointly).
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// The spawned child. Killed on [`Drop`] / [`Self::kill`].
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Raw bytes received from the PTY so far (tail-capped). Appended to
    /// by the main loop on each [`AppEvent::AgentOutput`]; read by the
    /// renderer. Lossy by design for Phase C.
    output: Vec<u8>,
    /// Set once the reader thread saw EOF (process exited / PTY closed).
    exited: bool,
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
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
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
        let writer = pair.master.take_writer()?;

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

        Ok(Self {
            name: spec.name.clone(),
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            child,
            output: Vec::new(),
            exited: false,
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

    /// Append a chunk read from the PTY into the retained output buffer,
    /// trimming the front when it would exceed [`OUTPUT_CAP`].
    pub fn push_output(&mut self, chunk: &[u8]) {
        self.output.extend_from_slice(chunk);
        if self.output.len() > OUTPUT_CAP {
            let overflow = self.output.len() - OUTPUT_CAP;
            self.output.drain(..overflow);
        }
    }

    /// The retained raw output. Lossy for Phase C — the renderer strips
    /// escape sequences crudely.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Mark the process as exited (reader thread saw EOF).
    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    /// Whether the agent process has exited.
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Resize the PTY to `cols` x `rows`. Called by the renderer when
    /// the agent pane's rectangle changes so the agent reflows.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
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

        // The PTY echoes what we write; collect output events until we
        // see our marker or time out.
        session.write(b"vorto-marker\n");
        let mut saw_marker = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(AppEvent::AgentOutput(bytes)) => {
                    session.push_output(&bytes);
                    if String::from_utf8_lossy(session.output()).contains("vorto-marker") {
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
        assert!(saw_marker, "expected the PTY to echo our marker back");
        assert!(!session.exited());

        // Killing reaps the child; a subsequent kill is harmless.
        session.kill();
        session.kill();
    }

    #[test]
    fn push_output_caps_at_the_buffer_limit() {
        // Drive the tail-cap directly without a process: build a session
        // and feed it more than `OUTPUT_CAP`, then assert the front was
        // trimmed and the tail retained.
        let (tx, _rx) = std::sync::mpsc::channel();
        let cwd = std::env::current_dir().unwrap();
        let Ok(mut session) = AgentSession::spawn(&cat_spec(), &cwd, tx) else {
            // `cat` not available — skip rather than fail.
            return;
        };
        let head = vec![b'a'; OUTPUT_CAP];
        session.push_output(&head);
        session.push_output(b"TAIL");
        assert_eq!(session.output().len(), OUTPUT_CAP);
        assert!(session.output().ends_with(b"TAIL"));
        session.kill();
    }
}
