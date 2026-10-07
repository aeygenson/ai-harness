//! Noticing that the system asks the harness to stop, so it can stop the
//! running agent first.
//!
//! Each agent runs in its own process group (see [`crate::process`]), so the
//! terminal's signals reach only the harness. If the harness simply died, the
//! agent would keep working with its login and nobody watching. With these
//! signals caught, the harness ends normally instead, and ending normally
//! stops the agent with everything it started.
//!
//! - Linux and macOS: Ctrl+C (`SIGINT`), `kill` and logout (`SIGTERM`), and
//!   the terminal window closed (`SIGHUP`).
//! - Windows: Ctrl+C, Ctrl+Break and the console window closed. Windows gives
//!   a closing program only a few seconds, enough to stop the agent's tree.

use std::io;

#[cfg(unix)]
use tokio::signal::unix::{signal, Signal, SignalKind};
#[cfg(windows)]
use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, CtrlBreak, CtrlC, CtrlClose};

/// The stop signals the harness listens for. While a value of this type
/// exists, those signals no longer end the program at once; [`StopSignals::recv`]
/// says when one came.
#[derive(Debug)]
pub struct StopSignals {
    #[cfg(unix)]
    interrupt: Signal,
    #[cfg(unix)]
    terminate: Signal,
    #[cfg(unix)]
    hangup: Signal,
    #[cfg(windows)]
    ctrl_c: CtrlC,
    #[cfg(windows)]
    ctrl_break: CtrlBreak,
    #[cfg(windows)]
    close: CtrlClose,
}

impl StopSignals {
    /// Starts listening. Must be called inside a `tokio` runtime; from this
    /// moment on the signals are caught, even before [`StopSignals::recv`] runs.
    pub fn listen() -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
                hangup: signal(SignalKind::hangup())?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                ctrl_c: ctrl_c()?,
                ctrl_break: ctrl_break()?,
                close: ctrl_close()?,
            })
        }
    }

    /// Waits until one of the stop signals comes.
    pub async fn recv(&mut self) {
        // `select!` waits for whichever comes first.
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
            _ = self.hangup.recv() => {}
        }
        #[cfg(windows)]
        tokio::select! {
            _ = self.ctrl_c.recv() => {}
            _ = self.ctrl_break.recv() => {}
            _ = self.close.recv() => {}
        }
    }
}

// Sending a signal to the test program itself works only on Linux and macOS.
#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;
    use std::time::Duration;

    use super::*;

    fn send_to_self(name: &str) {
        let status = Command::new("kill")
            .args([&format!("-{name}"), &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn a_closed_terminal_and_kill_are_noticed_instead_of_ending_the_program() {
        let mut signals = StopSignals::listen().unwrap();

        for name in ["HUP", "TERM"] {
            send_to_self(name);
            // Still running: the signal was caught, and `recv` says so.
            tokio::time::timeout(Duration::from_secs(10), signals.recv())
                .await
                .unwrap_or_else(|_| panic!("SIG{name} was not noticed"));
        }
    }
}
