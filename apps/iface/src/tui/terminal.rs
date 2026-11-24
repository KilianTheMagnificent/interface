//! Terminal lifecycle: raw mode and screen setup and teardown, shutdown
//! signals, the frame-capped event loop, and terminal notifications.
use std::{
    io::{Stdout, Write},
    task::{Context, Poll, Waker},
};

use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::signal::unix::{Signal, SignalKind, signal};

use super::view::render_tui;
use super::{
    AgentController, Duration, EffectOutcome, HeldInput, Instant, Rect, TerminalNotification,
    TuiApp, TuiMode, watch,
};

pub(super) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    alternate: bool,
}

pub(crate) struct ShutdownSignals {
    interrupt: Signal,
    terminate: Signal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShutdownSignalAction {
    BeginDrain,
    ForceExit(u8),
}

pub(super) fn accept_shutdown_signal(
    signal_exit: &mut Option<u8>,
    code: u8,
) -> ShutdownSignalAction {
    let already_draining = signal_exit.replace(code).is_some();
    if already_draining {
        ShutdownSignalAction::ForceExit(code)
    } else {
        ShutdownSignalAction::BeginDrain
    }
}

impl ShutdownSignals {
    pub(crate) fn new() -> Result<Self, String> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).map_err(|error| error.to_string())?,
            terminate: signal(SignalKind::terminate()).map_err(|error| error.to_string())?,
        })
    }

    pub(super) async fn next(&mut self) -> u8 {
        tokio::select! {
            _ = self.interrupt.recv() => 130,
            _ = self.terminate.recv() => 143,
        }
    }

    pub(crate) fn pending(&mut self) -> Option<u8> {
        let mut context = Context::from_waker(Waker::noop());
        if matches!(
            self.interrupt.poll_recv(&mut context),
            Poll::Ready(Some(()))
        ) {
            Some(130)
        } else if matches!(
            self.terminate.poll_recv(&mut context),
            Poll::Ready(Some(()))
        ) {
            Some(143)
        } else {
            None
        }
    }
}

impl TerminalSession {
    pub(super) fn start(mode: TuiMode) -> Result<Self, String> {
        enable_raw_mode().map_err(|error| error.to_string())?;
        let mut stdout = std::io::stdout();
        let alternate = mode == TuiMode::Fullscreen;
        let setup = if alternate {
            execute!(
                stdout,
                EnterAlternateScreen,
                EnableBracketedPaste,
                EnableFocusChange,
                cursor::Hide
            )
        } else {
            execute!(
                stdout,
                EnableBracketedPaste,
                EnableFocusChange,
                cursor::Hide
            )
        };
        if let Err(error) = setup {
            let _ = disable_raw_mode();
            return Err(error.to_string());
        }
        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).map_err(|error| error.to_string())?;
        Ok(Self {
            terminal,
            alternate,
        })
    }

    fn notify(&mut self, notification: &TerminalNotification, focused: bool) {
        let sequence = terminal_notification_sequence(notification, focused);
        if sequence.is_empty() {
            return;
        }
        let backend = self.terminal.backend_mut();
        let _ = backend.write_all(sequence.as_bytes());
        let _ = backend.flush();
    }

    /// Writes `text` into native scrollback as 0.1.169 did: leave the
    /// alternate screen, print on the primary screen, come back, and redraw
    /// the whole next frame on the cleared alternate screen.
    fn dump_to_scrollback(&mut self, text: &str) -> Result<(), String> {
        if !self.alternate {
            return Ok(());
        }
        // Raw mode turns off the terminal's newline-to-CRLF translation.
        let text = text.replace('\n', "\r\n");
        let backend = self.terminal.backend_mut();
        write!(backend, "\u{1b}[?1049l\r\n{text}\r\n\u{1b}[?1049h")
            .and_then(|()| backend.flush())
            .map_err(|error| error.to_string())?;
        // A same-size resize clears the screen and ratatui's previous frame
        // without the cursor-position query `Terminal::clear` makes.
        let size = self.terminal.size().map_err(|error| error.to_string())?;
        self.terminal
            .resize(Rect::new(0, 0, size.width, size.height))
            .map_err(|error| error.to_string())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let backend = self.terminal.backend_mut();
        if self.alternate {
            let _ = execute!(
                backend,
                DisableBracketedPaste,
                DisableFocusChange,
                LeaveAlternateScreen,
                cursor::Show
            );
        } else {
            let _ = execute!(
                backend,
                DisableBracketedPaste,
                DisableFocusChange,
                cursor::Show
            );
        }
        let _ = backend.flush();
    }
}

/// Minimum spacing of frames drawn because agent state changed. Streaming
/// deltas otherwise redraw back to back; key, paste, focus and resize frames
/// stay immediate.
pub(super) const REVISION_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Caps revision-driven redraws at one per [`REVISION_FRAME_INTERVAL`]. The
/// first change after an idle interval draws at once, and a change during the
/// interval always gets a trailing frame, so the newest state is never dropped.
pub(super) struct RevisionFrameCap {
    last_draw: Instant,
    pending: bool,
}

impl RevisionFrameCap {
    pub(super) const fn new(now: Instant) -> Self {
        Self {
            last_draw: now,
            pending: false,
        }
    }

    /// Records a frame of any kind; it already shows every change seen so far.
    pub(super) fn drew(&mut self, now: Instant) {
        self.last_draw = now;
        self.pending = false;
    }

    /// Resolves when a revision-driven frame is due. Cancel-safe: a dropped
    /// wait keeps its pending change for the next call.
    pub(super) async fn next_frame<T>(&mut self, revision: &mut watch::Receiver<T>) {
        if !self.pending {
            if revision.changed().await.is_err() {
                // No more revisions can arrive; only other events draw now.
                std::future::pending::<()>().await;
            }
            self.pending = true;
        }
        tokio::time::sleep_until(self.last_draw + REVISION_FRAME_INTERVAL).await;
    }
}

pub(super) async fn run_terminal_loop(
    mut terminal: TerminalSession,
    app: &mut TuiApp,
    signals: &mut ShutdownSignals,
    mut discover_after_paint: bool,
) -> Result<Option<u8>, String> {
    // JoinSet aborts unfinished read-only discovery on every loop exit, before
    // provider shutdown. No detached startup work can repopulate closed state.
    let mut discovery = tokio::task::JoinSet::new();
    // The provider effect in flight (see `effects`). Unlike discovery it is
    // never aborted: every loop exit drops the handle, which detaches the task,
    // and shutdown's runtime fence settles whatever it still has in flight.
    let mut effect: Option<tokio::task::JoinHandle<EffectOutcome>> = None;
    let mut revision = app.controller.subscribe_agent_state();
    let mut frame_cap = RevisionFrameCap::new(Instant::now());
    let mut dirty = true;
    let mut poll = None;
    loop {
        if !app.quit
            && effect.is_none()
            && let Some(run) = app.next_effect()
        {
            effect = Some(tokio::spawn(run));
        }
        if dirty {
            // While an effect is pending the inline await used to hold these
            // back, and its result may put the prompt back in the composer
            // first, so they wait for it.
            if !app.effect_pending() {
                app.apply_queued_send_failures();
            }
            if let Some(dump) = app.scrollback_dump.take() {
                terminal.dump_to_scrollback(&dump)?;
            }
            // Mark the revision seen before drawing: a change from here on
            // gets a frame of its own.
            revision.borrow_and_update();
            terminal
                .terminal
                .draw(|frame| render_tui(frame, app))
                .map_err(|error| error.to_string())?;
            frame_cap.drew(Instant::now());
            let notifications = app.controller.take_notifications();
            if app.config.notifications() {
                for notification in notifications {
                    terminal.notify(&notification, app.focused);
                }
            }
            dirty = false;
            if discover_after_paint {
                discover_after_paint = false;
                let controller = app.controller.clone();
                discovery.spawn(async move {
                    controller.discover_providers().await;
                });
            }
        }
        if app.quit {
            return Ok(None);
        }
        if poll.is_none() {
            // event::poll blocks its thread, so one blocking thread stays parked on
            // the terminal for the whole session. The handle must be held across
            // iterations: dropping it would not cancel the blocking call, and a
            // fresh spawn each pass would park a second thread on the same tty.
            poll = Some(tokio::task::spawn_blocking(|| {
                event::poll(Duration::from_secs(1))
            }));
        }
        tokio::select! {
            code = signals.next() => {
                return Ok(Some(code));
            }
            result = poll.as_mut().expect("poll handle") => {
                poll = None;
                let ready = result.map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                if ready {
                    match event::read().map_err(|error| error.to_string())? {
                        Event::Key(key) => {
                            app.terminal_input(HeldInput::Key(key)).await;
                            dirty = true;
                        }
                        Event::Paste(text) => {
                            app.terminal_input(HeldInput::Paste(text)).await;
                            dirty = true;
                        }
                        Event::FocusGained => {
                            app.focused = true;
                            dirty = true;
                        }
                        Event::FocusLost => {
                            app.focused = false;
                            dirty = true;
                        }
                        Event::Resize(_, _) => {
                            dirty = true;
                        }
                        Event::Mouse(_) => {}
                    }
                }
            }
            // Lazy: select! evaluates a disabled branch's expression too.
            joined = async { effect.as_mut().expect("effect handle").await }, if effect.is_some() => {
                effect = None;
                let outcome = match joined {
                    Ok(outcome) => outcome,
                    // A panic surfaces here exactly as it did from the inline await.
                    Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                    Err(error) => return Err(error.to_string()),
                };
                app.finish_effect(outcome).await;
                dirty = true;
            }
            () = frame_cap.next_frame(&mut revision) => {
                dirty = true;
            }
        }
    }
}

pub(super) fn terminal_notification_sequence(
    notification: &TerminalNotification,
    focused: bool,
) -> String {
    if focused {
        return String::new();
    }
    let title = notification_part(&notification.title);
    let body = notification_part(&notification.body);
    format!(
        "\u{7}\u{1b}]9;{title}{}\u{1b}\\",
        if body.is_empty() {
            String::new()
        } else {
            format!(": {body}")
        }
    )
}

pub(super) fn notification_part(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() || ('\u{7f}'..='\u{9f}').contains(&character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}
