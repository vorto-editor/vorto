mod action;
mod agent;
mod app;
mod bookmark;
mod buffer_ref;
mod config;
mod copilot;
mod editor;
mod effect;
mod event;
mod finder;
mod format;
mod grammar;
mod log;
mod lsp;
mod mode;
mod prompt;
mod syntax;
mod text_width;
mod theme;
mod ui;
mod vcs;

use std::io::{self, Stdout, Write};
use std::sync::mpsc;
use std::thread;

use anyhow::Result;
use crossterm::event::{
    self as crossterm_event, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture,
    EnableBracketedPaste, EnableFocusChange, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::action::PromptKind;
use crate::app::App;
use crate::app::root_cause;
use crate::config::CursorShape;
use crate::finder::{FuzzyKind, IgnoreOpts};

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    // `vorto grammar …` is a one-shot CLI that builds and installs
    // tree-sitter `.so` libraries; it never enters the TUI, so handle
    // it before we touch the terminal.
    if argv.get(1).map(String::as_str) == Some("grammar") {
        return grammar::cli::run(&argv[2..]);
    }
    // `--version` / `--help` are likewise one-shots — print and exit
    // before any terminal setup.
    match argv.get(1).map(String::as_str) {
        Some("-V" | "--version") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("-h" | "--help") => {
            print_usage();
            return Ok(());
        }
        _ => {}
    }

    let path = argv.into_iter().nth(1);
    // Anchor for LSP workspace root discovery — captured once here so the
    // value can't shift mid-session if anything changes the process's
    // cwd. Every later `:e` resolves against the same directory.
    let mut startup_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // `vorto <dir>` (e.g. `vorto .`) means "open this directory as the
    // workspace root" — not "load the directory as a file" (which would
    // EISDIR out of `read_to_string`). Treat the arg as a workspace
    // anchor and open the fuzzy file picker instead.
    let (file_arg, dir_arg) = match path {
        Some(p) => {
            let pb = std::path::PathBuf::from(&p);
            if pb.is_dir() {
                let abs = if pb.is_absolute() {
                    pb.clone()
                } else {
                    startup_cwd.join(&pb)
                };
                let canon = abs.canonicalize().unwrap_or(abs);
                // chdir so child processes spawned by LSP / git inherit
                // the workspace root, matching what a user `cd` would do.
                let _ = std::env::set_current_dir(&canon);
                startup_cwd = canon;
                (None, true)
            } else {
                (Some(p), false)
            }
        }
        None => (None, false),
    };

    log::init();
    vlog!(
        "startup pid={} version={} cwd={}",
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
        startup_cwd.display(),
    );

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    // Bracketed paste: the terminal wraps pasted text in `\x1b[200~` …
    // `\x1b[201~` so crossterm can surface it as `Event::Paste(String)`
    // instead of a stream of synthesized key events. Without this, every
    // `\n` in the paste fires the Enter handler and auto-indent compounds
    // on the indent the pasted text already carries.
    execute!(stdout, EnableBracketedPaste)?;
    // Focus reporting: the terminal emits `\x1b[I` / `\x1b[O` on focus
    // gain/loss, surfaced as `Event::FocusGained` / `Event::FocusLost`.
    // The autoreload watcher uses these to pause disk polling while the
    // editor is backgrounded. Terminals that don't support it simply
    // never send the events — the watcher then just stays active.
    execute!(stdout, EnableFocusChange)?;
    // Mouse reporting: the terminal forwards wheel / click events as
    // `Event::Mouse` instead of letting the host translate wheel scroll
    // into arrow keys (which, in the alt screen, would just drive the
    // focused pane's cursor). We want the wheel to scroll the agent
    // pane's terminal scrollback. Trade-off: with capture on, the host
    // terminal's native click-drag text selection is suppressed — most
    // terminals fall back to it when the user holds Shift/Option.
    execute!(stdout, EnableMouseCapture)?;
    // Kitty keyboard protocol: with `DISAMBIGUATE_ESCAPE_CODES`, the
    // terminal reports Shift+Tab, Ctrl+modified keys, etc. as distinct
    // events instead of collapsing them onto plain ASCII codes. Without
    // it, e.g. macOS Terminal.app sends Shift+Tab as plain Tab (no
    // SHIFT modifier), making it indistinguishable from Tab. Push only
    // on terminals that advertise support — pushing on an unsupported
    // terminal is usually harmless but `supports_keyboard_enhancement`
    // is the documented gate.
    let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    vlog!("kbd_enhanced={kbd_enhanced}");
    if kbd_enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // EnterAlternateScreen *should* clear the alt screen, but not every
    // terminal honors that. Without an explicit clear, stale cells from
    // a previous vorto session (or any program that ran in the same alt
    // buffer) can leak through anywhere our render doesn't write — and
    // ratatui's diff won't fix them, because the previous-buffer it
    // diffs against is its own empty buffer, not the terminal's actual
    // contents. Force a full clear so the next draw paints onto a known
    // blank screen.
    terminal.clear()?;

    let cfg_path = config::default_path();
    // A broken user config (typo, parse error) is non-fatal: log it and
    // fall back to the built-in defaults so the editor still starts. The
    // user can fix the file and `:config-reload` without losing the session.
    // The error is surfaced as a toast once the app is up (see below).
    let mut cfg_warning: Option<String> = None;
    let cfg = match config::Config::load(cfg_path.as_deref()) {
        Ok(c) => {
            vlog!(
                "config loaded path={}",
                cfg_path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<default>".into()),
            );
            c
        }
        Err(e) => {
            vlog!("config load failed, using defaults: {e:#}");
            cfg_warning = Some(format!("config: {}; using defaults", root_cause(&e)));
            config::Config::load(None).expect("default config loads")
        }
    };
    // Apply the configured theme. A bad name (typo, deleted file) is
    // non-fatal: log it and keep the `ansi` seed so the editor still
    // starts and renders.
    match theme::load_by_name(&cfg.theme) {
        Ok(t) => theme::set_active(t),
        Err(e) => vlog!("theme `{}` load failed: {e:#}", cfg.theme),
    }

    // Extract any grammars embedded in the binary (release builds with
    // `--features bundled-grammars`) into `grammar_dir`/`query_dir` so the
    // loader below finds them. Best-effort: a failure just means those
    // languages won't highlight until `grammar install` runs.
    #[cfg(feature = "bundled-grammars")]
    match grammar::bundled::bootstrap(&cfg.grammar_dir, &cfg.query_dir, &cfg.grammars) {
        Ok(0) => {}
        Ok(n) => vlog!("bundled grammars: extracted {n}"),
        Err(e) => vlog!("bundled grammar bootstrap failed: {e:#}"),
    }

    let loader = syntax::Loader::new(cfg.grammar_dir.clone(), cfg.query_dir.clone());

    // Unified event channel. Terminal input runs on a dedicated thread
    // that pushes `Event::Term`; LSP reader threads push `Event::Lsp`.
    let (event_tx, event_rx) = mpsc::channel::<event::AppEvent>();
    let input_tx = event_tx.clone();
    thread::spawn(move || {
        loop {
            match crossterm_event::read() {
                Ok(ev) => {
                    if input_tx.send(event::AppEvent::Term(ev)).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let mut app = App::new(cfg, loader, event_tx, startup_cwd);
    // Surface a config-load failure now that the UI exists. The editor is
    // running on built-in defaults; the toast tells the user why so a
    // silently-ignored config doesn't read as "my settings don't work."
    if let Some(w) = cfg_warning {
        app.push_toast(app::Toast::warn(w));
    }
    // Best-effort: spawn Copilot eagerly so ghost-text completions are
    // ready by the time the user starts typing. Silent no-op when the
    // server binary isn't installed.
    app.spawn_copilot_if_needed();
    if let Some(p) = file_arg {
        app.open_path(std::path::Path::new(&p))?;
    } else if dir_arg {
        app.open_prompt(PromptKind::Fuzzy(FuzzyKind::Files {
            ignore: IgnoreOpts::DEFAULT,
        }));
    }

    let result = run(&mut terminal, &mut app, &event_rx);

    // Kill the single agent process on the quit path. `Drop` would also
    // do this when `app` falls out of scope, but doing it explicitly
    // here means the child is reaped before we tear down the terminal.
    if let Some(agent) = app.agent.as_mut() {
        agent.kill();
    }

    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    let _ = execute!(terminal.backend_mut(), DisableFocusChange);
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    // `\x1b[0 q` = DECSCUSR Ps=0 → restore the user's configured shape.
    let _ = io::stdout().write_all(b"\x1b[0 q");
    let _ = io::stdout().flush();
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    event_rx: &mpsc::Receiver<event::AppEvent>,
) -> Result<()> {
    let mut last_shape: Option<CursorShape> = None;
    let mut prev_prompt_open = false;
    while !app.should_quit {
        app.active_doc_mut().refresh_highlights();
        app.tick_toasts();
        // When any modal prompt (fuzzy picker, hover popup, completion,
        // …) just closed, force a full repaint of the next frame. The
        // popup widgets only `Clear` their own rect, so cells the popup
        // wrote that *aren't* covered by the post-close render would
        // otherwise rely on ratatui's per-cell diff to clean them up.
        // That has been observed to leak syntax-highlighted fragments
        // when a fuzzy preview disappears, presumably because of a
        // diff-vs-terminal-state mismatch the previous-buffer doesn't
        // catch. `terminal.clear()` resets the back buffer so the next
        // diff emits every cell, masking the issue.
        let now_open = app.prompt.is_open();
        if prev_prompt_open && !now_open {
            terminal.clear()?;
        }
        prev_prompt_open = now_open;
        terminal.draw(|f| ui::draw(f, app))?;
        // Cursor shape follows the focused pane: the editor's per-mode
        // shape for an editor pane, or the agent terminal's own cursor
        // shape when the agent pane is focused.
        let shape = if Some(app.active_pane) == app.agent_pane {
            app.agent
                .as_ref()
                .map(|a| a.cursor_shape())
                .unwrap_or_else(|| app.config.cursor_shapes.for_mode(app.editor.mode))
        } else {
            app.config.cursor_shapes.for_mode(app.editor.mode)
        };
        if last_shape != Some(shape) {
            let mut out = io::stdout();
            out.write_all(cursor_ansi(shape, app.config.cursor_shapes.blinking))?;
            out.flush()?;
            last_shape = Some(shape);
        }
        // Block on the next event. Both terminal input and LSP reader
        // threads feed this channel, so we wake on whichever comes first
        // and only redraw once after we drain the burst.
        //
        // When a toast is on screen, fall back to `recv_timeout` so the
        // loop wakes when the TTL expires and the next redraw can drop
        // the toast — otherwise it would linger until the user happens
        // to press a key.
        // Merge wake sources: toast TTL, indent-guide animation, and
        // the debounced inline-completion fire. Smallest non-`None`
        // wins; `None`-vs-`None` falls back to a blocking `recv`.
        let wake = [
            app.toast_remaining(),
            app.indent_anim_remaining(),
            app.inline_request_remaining(),
            app.file_check_remaining(),
        ]
        .into_iter()
        .flatten()
        .min();
        let first = match wake {
            Some(rem) => match event_rx.recv_timeout(rem) {
                Ok(ev) => Some(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => match event_rx.recv() {
                Ok(ev) => Some(ev),
                Err(_) => return Ok(()),
            },
        };
        if let Some(ev) = first {
            dispatch(app, ev)?;
        }
        // Drain any events that piled up while we were blocked so we
        // don't redraw between a Term+Lsp pair (e.g. didChange burst).
        while let Ok(ev) = event_rx.try_recv() {
            dispatch(app, ev)?;
        }
        app.sync_buffer_if_dirty();
        // Poll the active buffer's backing file for external edits. No-op
        // until `FILE_CHECK_INTERVAL` elapses; opens a reload prompt on
        // drift. Woken either by the `file_check_remaining` timeout above
        // or piggybacking on any other event.
        app.check_active_file_changed();
        // Fire the inline-completion request if the debounce deadline
        // has elapsed. Sits after the dispatch loop so a burst of
        // typing events all extend the deadline before we evaluate it.
        app.tick_inline_suggestion();
    }
    Ok(())
}

/// Append a single key event to the path in `VORTO_KEY_LOG`, if that
/// env var is set. For diagnosing terminals that swallow or remap keys
/// like Shift+Tab. Errors are silently dropped — this is opt-in debug.
fn log_key_event(key: &crossterm_event::KeyEvent) {
    use std::fs::OpenOptions;
    let Ok(path) = std::env::var("VORTO_KEY_LOG") else {
        return;
    };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{key:?}");
    }
}

fn dispatch(app: &mut App, ev: event::AppEvent) -> Result<()> {
    match ev {
        event::AppEvent::Term(Event::Key(key)) => {
            log_key_event(&key);
            app.handle_key(key)?;
        }
        event::AppEvent::Term(Event::Paste(s)) => app.handle_paste(s),
        event::AppEvent::Term(Event::FocusGained) => app.set_focused(true),
        event::AppEvent::Term(Event::FocusLost) => app.set_focused(false),
        event::AppEvent::Term(Event::Mouse(me)) => app.handle_mouse(me)?,
        event::AppEvent::Term(_) => {}
        event::AppEvent::Lsp(lsp_ev) => app.handle_lsp_event(lsp_ev),
        event::AppEvent::Copilot(cp_ev) => app.handle_copilot_event(cp_ev),
        event::AppEvent::CopilotReady { result } => app.handle_copilot_ready(result),
        event::AppEvent::EngineReady { generation, result } => {
            app.handle_engine_ready(generation, result);
        }
        event::AppEvent::LspReady {
            generation,
            client_key,
            lang,
            path,
            result,
        } => {
            app.handle_lsp_ready(generation, client_key, lang, path, result);
        }
        event::AppEvent::GrammarInstalled { name, result } => {
            app.handle_grammar_installed(name, result);
        }
        event::AppEvent::PreviewReady(entry) => app.handle_preview_ready(entry),
        event::AppEvent::VcsBaseReady {
            generation,
            path,
            base,
        } => app.handle_vcs_base_ready(generation, path, base),
        event::AppEvent::AgentOutput(bytes) => {
            if let Some(agent) = app.agent.as_ref() {
                agent.push_output(&bytes);
            }
        }
        event::AppEvent::AgentExited => {
            app.close_agent_pane();
        }
    }
    Ok(())
}

fn print_usage() {
    println!(
        "{name} {version}

Usage:
    vorto [FILE|DIR]
    vorto grammar <list|install|remove> [args]
    vorto -h | --help
    vorto -V | --version",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );
}

/// DECSCUSR escape sequence — `CSI Ps SP q`, where Ps picks the shape.
/// Written directly to stdout from the main loop so the terminal
/// switches shape as the user changes mode.
fn cursor_ansi(shape: CursorShape, blinking: bool) -> &'static [u8] {
    match (shape, blinking) {
        (CursorShape::Terminal, _) => b"\x1b[0 q",
        (CursorShape::Block, true) => b"\x1b[1 q",
        (CursorShape::Block, false) => b"\x1b[2 q",
        (CursorShape::Underbar, true) => b"\x1b[3 q",
        (CursorShape::Underbar, false) => b"\x1b[4 q",
        (CursorShape::Bar, true) => b"\x1b[5 q",
        (CursorShape::Bar, false) => b"\x1b[6 q",
    }
}
