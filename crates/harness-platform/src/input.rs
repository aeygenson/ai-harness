//! Waiting for a key from the terminal, and noticing that the terminal is gone.
//!
//! The TUI reads keys with crossterm. When its terminal window is closed,
//! crossterm 0.29 on Linux and macOS keeps reading the empty input forever
//! (each read returns 0 bytes) and never comes back, so the TUI would neither
//! quit nor stop its agent. [`wait_for_input`] waits first: crossterm is asked
//! only when a key is really there, and a closed terminal ends the TUI.
//! (Closing the window in the instant between the wait and crossterm's read
//! can still catch crossterm; the stop signal cannot help there either.)

use std::io;
use std::time::Duration;

/// What [`wait_for_input`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A key (or a click, a paste) is waiting: crossterm can read it at once.
    Ready,
    /// Nothing came before the time ran out.
    Nothing,
    /// The terminal is closed: nothing will ever come again.
    Closed,
    /// This input cannot be watched here (Windows, or the input is not a
    /// terminal): ask crossterm itself, with the same timeout.
    Unwatched,
}

/// Waits up to `timeout` for input from the terminal (the program's stdin).
///
/// # Errors
///
/// When the system cannot wait on the input at all.
#[cfg(unix)]
pub fn wait_for_input(timeout: Duration) -> io::Result<Input> {
    use std::io::IsTerminal;

    let stdin = io::stdin();
    // crossterm reads from `/dev/tty` instead when stdin is not a terminal.
    if !stdin.is_terminal() {
        return Ok(Input::Unwatched);
    }
    wait_on(&stdin, timeout)
}

/// Waits up to `timeout` for input from the terminal. Windows ends a program
/// whose console window is closed, so there is nothing to watch for here.
///
/// # Errors
///
/// Never; the `Result` keeps the signature the same on every system.
#[cfg(windows)]
pub fn wait_for_input(_timeout: Duration) -> io::Result<Input> {
    Ok(Input::Unwatched)
}

/// Waits up to `timeout` until `input` can be read or is closed.
///
/// `impl AsFd` accepts anything that owns or borrows an open file
/// descriptor: stdin, a pipe in the tests.
#[cfg(unix)]
fn wait_on(input: &impl std::os::fd::AsFd, timeout: Duration) -> io::Result<Input> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};

    let timeout = Timespec::try_from(timeout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut watched = [PollFd::new(input, PollFlags::IN)];
    match poll(&mut watched, Some(&timeout)) {
        Ok(_) => {}
        // A signal interrupted the wait: the loop simply comes back.
        Err(rustix::io::Errno::INTR) => return Ok(Input::Nothing),
        Err(error) => return Err(error.into()),
    }
    let found = watched[0].revents();
    // NVAL: this system cannot watch this input (macOS cannot poll some
    // terminals); crossterm then waits as it did before.
    if found.contains(PollFlags::NVAL) {
        Ok(Input::Unwatched)
    // HUP: the other end (the terminal window, the writer of a pipe) is gone.
    // ERR: the input is broken.
    } else if found.intersects(PollFlags::HUP | PollFlags::ERR) {
        Ok(Input::Closed)
    } else if found.contains(PollFlags::IN) {
        Ok(Input::Ready)
    } else {
        Ok(Input::Nothing)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Write;

    use super::*;

    const SHORT: Duration = Duration::from_millis(50);

    #[test]
    fn a_waiting_key_is_ready() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"q").unwrap();

        assert_eq!(wait_on(&reader, SHORT).unwrap(), Input::Ready);
    }

    #[test]
    fn without_a_key_the_time_runs_out() {
        let (reader, _writer) = io::pipe().unwrap();

        assert_eq!(wait_on(&reader, SHORT).unwrap(), Input::Nothing);
    }

    /// A pseudo-terminal, as a terminal window gives the TUI: the window's
    /// end (`window`) and the program's end (`program`, its stdin).
    fn pseudo_terminal() -> (std::os::fd::OwnedFd, std::fs::File) {
        use rustix::pty::{grantpt, openpt, ptsname, unlockpt, OpenptFlags};

        let window = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&window).unwrap();
        unlockpt(&window).unwrap();
        let name = ptsname(&window, Vec::new()).unwrap();
        let program = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().unwrap())
            .unwrap();
        (window, program)
    }

    #[test]
    fn a_key_typed_in_the_terminal_is_ready_and_a_closed_window_is_noticed() {
        let (window, program) = pseudo_terminal();
        assert_eq!(wait_on(&program, SHORT).unwrap(), Input::Nothing);

        // A new terminal hands over whole lines, so the key comes with Enter.
        rustix::io::write(&window, b"q\n").unwrap();
        assert_eq!(wait_on(&program, SHORT).unwrap(), Input::Ready);

        drop(window);
        assert_eq!(wait_on(&program, SHORT).unwrap(), Input::Closed);
    }

    #[test]
    fn a_closed_input_is_noticed() {
        let (reader, writer) = io::pipe().unwrap();
        drop(writer);

        assert_eq!(wait_on(&reader, SHORT).unwrap(), Input::Closed);
    }
}
