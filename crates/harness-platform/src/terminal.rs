//! The terminal: reading a secret without showing it, and the key that lets
//! the terminal select text while a full-screen program has the mouse.

use std::io::{self, BufRead};
#[cfg(not(windows))]
use std::process::{Command, Stdio};

/// The key held while dragging to select text in a terminal whose mouse is
/// taken by the TUI: `⌥` (or `fn`) on macOS, `Shift` everywhere else.
pub fn select_text_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥/fn"
    } else {
        "Shift"
    }
}

/// One line from the keyboard, not shown on the screen: `stty -echo` turns
/// the echo off while it is typed.
#[cfg(not(windows))]
pub fn read_line_hidden() -> io::Result<String> {
    let stty = |arg: &str| {
        Command::new("stty")
            .arg(arg)
            .stdin(Stdio::inherit())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let hidden = stty("-echo");
    let mut line = String::new();
    let read = io::stdin().lock().read_line(&mut line);
    if hidden {
        stty("echo");
        println!();
    }
    read.map(|_| line)
}

/// One line from the keyboard, not shown on the screen. Windows has no
/// `stty`, so the keys are read one by one with the console's echo off.
/// Only presses count: Windows also reports each key's release, and a
/// pasted key would otherwise be taken twice.
#[cfg(windows)]
pub fn read_line_hidden() -> io::Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use io::IsTerminal;

    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        return Ok(line);
    }
    crossterm::terminal::enable_raw_mode()?;
    let mut line = String::new();
    let result = loop {
        match read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Backspace => {
                    line.pop();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                KeyCode::Char(c) => line.push(c),
                _ => {}
            },
            Ok(Event::Paste(text)) => line.push_str(&text),
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    result.map(|()| line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_select_key_is_named_for_this_system() {
        let key = select_text_key();
        assert!(key == "Shift" || key == "⌥/fn");
    }
}
