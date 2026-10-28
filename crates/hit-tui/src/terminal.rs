//! Terminal setup and teardown.
//!
//! The terminal is restored on every exit path: normal return, `?` errors
//! (via [`TerminalGuard`]'s `Drop`) and panics (via [`install_panic_hook`]).

use std::{
    io::{self, Stdout, stdout},
    panic,
    sync::{
        Mutex, PoisonError, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::sync::Notify;

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::error::TuiError;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub struct TerminalGuard {
    terminal: Term,
    active: bool,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self, TuiError> {
        let setup = || -> io::Result<Term> {
            enable_raw_mode()?;
            execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste, Hide)?;
            Terminal::new(CrosstermBackend::new(stdout()))
        };
        match setup() {
            Ok(terminal) => Ok(Self {
                terminal,
                active: true,
            }),
            Err(e) => {
                // Undo whatever part of the setup succeeded.
                let _ = restore_terminal();
                Err(TuiError::Setup(e))
            }
        }
    }

    pub fn terminal(&mut self) -> &mut Term {
        &mut self.terminal
    }

    /// Restores the terminal. Safe to call more than once.
    pub fn restore(&mut self) -> Result<(), TuiError> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        restore_terminal().map_err(TuiError::Restore)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            tracing::error!(error = ?e, "terminal restore failed during drop");
        }
    }
}

/// Puts the terminal back into its normal state. Every step is attempted even
/// if an earlier one fails; the first error is returned.
pub fn restore_terminal() -> io::Result<()> {
    let raw = disable_raw_mode();
    let screen = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen, Show);
    raw.and(screen)
}

/// Set once any thread panics. After that the TUI must never draw again.
static PANICKED: AtomicBool = AtomicBool::new(false);
static PANIC_NOTIFY: Notify = Notify::const_new();
/// Serializes drawing with the panic hook's terminal restore, so a frame can
/// never be written after the terminal was handed back.
static DRAW_LOCK: Mutex<()> = Mutex::new(());

/// Wraps the current panic hook. On a panic in *any* thread (including tokio
/// tasks, which would otherwise die silently) it restores the terminal, logs
/// the panic, prints the report and tells the app loop to shut down.
/// Install after the error reporter's own hook.
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        PANICKED.store(true, Ordering::SeqCst);
        restore_after_panic();
        tracing::error!(%info, "panic");
        previous(info);
        PANIC_NOTIFY.notify_waiters();
    }));
}

pub(crate) fn panicked() -> bool {
    PANICKED.load(Ordering::SeqCst)
}

/// Resolves once a panic happened anywhere in the process.
pub(crate) async fn wait_for_panic() {
    loop {
        let notified = PANIC_NOTIFY.notified();
        if panicked() {
            return;
        }
        notified.await;
    }
}

/// Runs `draw` unless a panic already happened.
pub(crate) fn draw_guarded<T>(draw: impl FnOnce() -> T) -> Option<T> {
    let _lock = DRAW_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    (!panicked()).then(draw)
}

fn restore_after_panic() {
    // Wait briefly for an in-flight frame to finish. If the panic happened
    // while drawing on this very thread the lock is already held, so give up
    // after a short while instead of deadlocking.
    for _ in 0..50 {
        match DRAW_LOCK.try_lock() {
            Ok(_lock) => {
                let _ = restore_terminal();
                return;
            }
            Err(TryLockError::Poisoned(lock)) => {
                let _lock = lock.into_inner();
                let _ = restore_terminal();
                return;
            }
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(2)),
        }
    }
    let _ = restore_terminal();
}
