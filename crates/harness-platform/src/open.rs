//! Opening an address in Lisa's browser: `open` on macOS, `xdg-open` on
//! Linux, and on Windows the system's own handler for addresses (the same
//! one a double-click on a link uses). None of them goes through a shell, so
//! `&` and other special characters in the address are safe.

use std::process::{Command, Stdio};

/// Opens `address` in the browser. `Err` says why it did not work.
pub fn url(address: &str) -> Result<(), String> {
    let (program, args) = opener();
    let started = Command::new(program)
        .args(args)
        .arg(address)
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
        Err(e) => Err(format!("cannot open the browser with {program}: {e}")),
    }
}

/// The program (and its first arguments) that opens an address here.
fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else {
        ("xdg-open", &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_system_has_its_opener() {
        let (program, _) = opener();
        let expected = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "rundll32"
        } else {
            "xdg-open"
        };
        assert_eq!(program, expected);
    }
}
