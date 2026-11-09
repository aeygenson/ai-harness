//! Telling the user that work has ended while they look at another window:
//! the terminal's bell, and a notification from the system (`notify-send` on
//! Linux, `osascript` on macOS). Windows has no such command without extra
//! modules, so there only the bell rings (Windows Terminal then marks the tab
//! and the taskbar button).

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// The bell character: the terminal beeps or flashes, and many terminals mark
/// their tab or taskbar button. It prints nothing on the screen.
const BELL: &[u8] = b"\x07";

/// Rings the terminal's bell. A terminal that cannot ring is not an error.
pub fn bell() {
    let mut out = io::stdout();
    // `let _ =`: a failed write here only means no sound; nothing to report.
    let _ = out.write_all(BELL);
    let _ = out.flush();
}

/// Shows a system notification with `title` and `text`. `Err` says why not,
/// for example when `notify-send` is not installed.
///
/// The text goes to the program as its own argument, never through a shell,
/// so quotes and other special characters in it are safe. The caller removes
/// control characters (the text may come from an agent).
pub fn desktop(title: &str, text: &str) -> Result<(), String> {
    let Some((program, args)) = notifier(title, text) else {
        return Err("this system has no notification command".to_string());
    };
    let started = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match started {
        Ok(mut child) => {
            // It ends at once; waiting keeps no finished process behind.
            std::thread::spawn(move || child.wait());
            Ok(())
        }
        Err(e) => Err(format!("cannot show a notification with {program}: {e}")),
    }
}

/// The program that shows a notification here, with its arguments.
fn notifier(title: &str, text: &str) -> Option<(&'static str, Vec<String>)> {
    if cfg!(target_os = "macos") {
        // The script takes the title and the text from its arguments (`argv`),
        // so they are never written into the script itself.
        let script = [
            "on run argv",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "end run",
        ];
        let mut args = Vec::new();
        for line in script {
            args.push("-e".to_string());
            args.push(line.to_string());
        }
        args.push(title.to_string());
        args.push(text.to_string());
        Some(("osascript", args))
    } else if cfg!(windows) {
        None
    } else {
        // `--` ends the options: a text starting with `-` stays text.
        let args = vec!["--".to_string(), title.to_string(), text.to_string()];
        Some(("notify-send", args))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_and_text_are_separate_arguments() {
        let found = notifier("Harness", "say \"hi\"; rm -rf ~");

        if cfg!(windows) {
            assert_eq!(found, None);
            return;
        }
        let (program, args) = found.unwrap();
        let expected = if cfg!(target_os = "macos") {
            "osascript"
        } else {
            "notify-send"
        };
        assert_eq!(program, expected);
        // The text is passed as it is, as the last argument.
        assert_eq!(args.last().unwrap(), "say \"hi\"; rm -rf ~");
        assert_eq!(args[args.len() - 2], "Harness");
        assert!(!args
            .iter()
            .any(|a| a.contains("say") && a.contains("display")));
    }
}
