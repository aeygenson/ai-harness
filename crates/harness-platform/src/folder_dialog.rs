//! The system's own «choose a folder» dialog: KDE's `kdialog` or `zenity` on
//! Linux, the Finder's «Choose folder» on macOS, the «Browse for folder»
//! window on Windows.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the system dialog answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    Chosen(PathBuf),
    Cancelled,
    /// There is no dialog program or no desktop: use the browser.
    Unavailable,
}

/// The command for the system's folder dialog: `desktop` says whether there
/// is a graphical session, `installed` whether a program can be found.
fn native_command(
    title: &str,
    start: &Path,
    desktop: bool,
    installed: impl Fn(&str) -> bool,
) -> Option<Command> {
    if !desktop {
        None
    } else if cfg!(target_os = "macos") {
        Some(mac_command(title, start))
    } else if cfg!(windows) {
        Some(windows_command(title, start))
    } else {
        linux_command(title, start, installed)
    }
}

/// macOS: AppleScript's «choose folder», run by `osascript`, which comes
/// with every Mac. The title and the folder are passed as arguments, never
/// written into the script, so quotes in them cannot change it. Cancel ends
/// it with an error (exit code 1).
fn mac_command(title: &str, start: &Path) -> Command {
    let mut command = Command::new("osascript");
    command
        .args(["-e", "on run argv"])
        .args([
            "-e",
            "POSIX path of (choose folder with prompt (item 1 of argv) \
             default location (POSIX file (item 2 of argv)))",
        ])
        .args(["-e", "end run"])
        .arg(title)
        .arg(start);
    command
}

/// The PowerShell script for Windows' folder window: it prints the chosen
/// folder, or ends with 1 on Cancel.
const WINDOWS_SCRIPT: &str = "\
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = $env:HARNESS_PICK_TITLE
$dialog.SelectedPath = $env:HARNESS_PICK_START
if ($dialog.ShowDialog() -eq 'OK') { $dialog.SelectedPath } else { exit 1 }
";

/// Windows: the folder window of Windows Forms, run by PowerShell, which
/// comes with every Windows. The title and the folder are passed in
/// variables, never written into the script, so quotes cannot change it.
fn windows_command(title: &str, start: &Path) -> Command {
    let mut command = Command::new(crate::program::resolve("powershell"));
    command
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command"])
        .arg(WINDOWS_SCRIPT)
        .env("HARNESS_PICK_TITLE", title)
        .env("HARNESS_PICK_START", start);
    command
}

/// Linux: KDE's dialog, or GNOME's `zenity`.
fn linux_command(title: &str, start: &Path, installed: impl Fn(&str) -> bool) -> Option<Command> {
    if installed("kdialog") {
        let mut command = Command::new("kdialog");
        command
            .arg("--title")
            .arg(title)
            .arg("--getexistingdirectory")
            .arg(start);
        Some(command)
    } else if installed("zenity") {
        let mut command = Command::new("zenity");
        command
            .arg("--file-selection")
            .arg("--directory")
            .arg(format!("--title={title}"))
            .arg(format!("--filename={}/", start.display()));
        Some(command)
    } else {
        None
    }
}

/// Opens the system's folder dialog and waits for it.
pub fn native_folder(title: &str, start: &Path) -> Native {
    let desktop = if cfg!(any(target_os = "macos", windows)) {
        // A Mac or Windows always has its desktop, unless the TUI runs over SSH.
        std::env::var_os("SSH_CONNECTION").is_none()
    } else {
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
    };
    let installed = |name: &str| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
    };
    match native_command(title, start, desktop, installed) {
        Some(command) => run_dialog(command),
        None => Native::Unavailable,
    }
}

/// Runs a dialog that prints the chosen folder, or exits with 1 on Cancel.
fn run_dialog(mut command: Command) -> Native {
    match command.output() {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // macOS ends a folder with `/`; the root folder keeps it.
            let path = match path.strip_suffix('/') {
                Some(inner) if !inner.is_empty() => inner.to_string(),
                _ => path,
            };
            if path.is_empty() {
                Native::Cancelled
            } else {
                Native::Chosen(PathBuf::from(path))
            }
        }
        // Cancel (exit code 1) closes the dialog without a folder.
        Ok(_) => Native::Cancelled,
        Err(_) => Native::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_dialog_is_used_when_there_is_one() {
        let start = Path::new("/home/me/code");
        let args = |command: Option<Command>| {
            command.map(|c| {
                let mut all = vec![c.get_program().to_string_lossy().into_owned()];
                all.extend(c.get_args().map(|a| a.to_string_lossy().into_owned()));
                all
            })
        };
        assert_eq!(
            args(linux_command("Pick", start, |_| true)).unwrap(),
            [
                "kdialog",
                "--title",
                "Pick",
                "--getexistingdirectory",
                "/home/me/code"
            ]
        );
        assert_eq!(
            args(linux_command("Pick", start, |n| n == "zenity")).unwrap()[0],
            "zenity"
        );
        assert!(linux_command("Pick", start, |_| false).is_none());
        assert!(native_command("Pick", start, false, |_| true).is_none());

        // On a Mac: the title and the folder are arguments, not script text.
        let mac = args(Some(mac_command("Pick \"x\"", start))).unwrap();
        assert_eq!(mac[0], "osascript");
        assert_eq!(mac[mac.len() - 2..], ["Pick \"x\"", "/home/me/code"]);
        assert!(mac
            .iter()
            .all(|a| !a.contains("/home/me") || a == "/home/me/code"));
        let on_this_system = args(native_command("Pick", start, true, |_| true)).unwrap();
        let expected = if cfg!(target_os = "macos") {
            "osascript"
        } else if cfg!(windows) {
            "powershell"
        } else {
            "kdialog"
        };
        assert!(on_this_system[0].contains(expected), "{on_this_system:?}");

        // On Windows: the title and the folder are variables, not script text.
        let windows = windows_command("Pick \"x\"", start);
        let vars: Vec<_> = windows.get_envs().collect();
        assert!(vars.contains(&(
            std::ffi::OsStr::new("HARNESS_PICK_TITLE"),
            Some(std::ffi::OsStr::new("Pick \"x\""))
        )));
        assert!(windows
            .get_args()
            .all(|a| !a.to_string_lossy().contains("Pick")));

        // `sh` and `false` stand in for a dialog that answers or is cancelled.
        #[cfg(unix)]
        {
            let mut chosen = Command::new("sh");
            chosen.args(["-c", "echo /home/me/code/app"]);
            assert_eq!(
                run_dialog(chosen),
                Native::Chosen("/home/me/code/app".into())
            );
            // As macOS prints it: with a `/` at the end.
            let mut chosen = Command::new("sh");
            chosen.args(["-c", "echo /Users/me/code/app/"]);
            assert_eq!(
                run_dialog(chosen),
                Native::Chosen("/Users/me/code/app".into())
            );
            assert_eq!(run_dialog(Command::new("false")), Native::Cancelled);
        }
        assert_eq!(
            run_dialog(Command::new("/no/such/dialog")),
            Native::Unavailable
        );
    }
}
