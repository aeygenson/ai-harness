//! Running a maker's install, update or remove command and showing its lines.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// Runs a maker's install or update `command` in the system's shell, with
/// the user's own environment (installers need the network settings and
/// `PATH`), nothing on standard input. Each line it prints goes to
/// `on_line`, cleaned of colours and progress bars. `Err` says how it ended.
pub fn run_command(command: &str, mut on_line: impl FnMut(String)) -> Result<(), String> {
    let (shell, args) = harness_platform::program::shell();
    let mut child = Command::new(harness_platform::program::resolve(shell));
    child
        .args(args)
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Started from PowerShell 7, Windows PowerShell 5.1 inherits a module
    // path it cannot load its own modules from, and installers fail with
    // «'Get-FileHash' is not recognized». Without it, it uses its default.
    child.env_remove("PSModulePath");
    // Programs installed into ~/.local/bin are found by the next steps.
    if let Some(path) = with_local_bin() {
        child.env("PATH", path);
    }
    let mut child =
        crate::process::spawn(&mut child).map_err(|e| format!("cannot start {shell}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let pipes: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|pipe| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                    let _ = tx.send(String::from_utf8_lossy(&line).into_owned());
                }
            })
        })
        .collect();
    drop(tx);
    for line in rx {
        let line = clean_line(&line);
        if !line.is_empty() {
            on_line(line);
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the command ended with {status}"))
    }
}

/// `PATH` with `~/.local/bin` in front, if it is not there already.
pub(super) fn with_local_bin() -> Option<std::ffi::OsString> {
    let local = harness_platform::home::home_dir()?
        .join(".local")
        .join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&path).any(|dir| dir == local) {
        return None;
    }
    std::env::join_paths(std::iter::once(local).chain(std::env::split_paths(&path))).ok()
}

/// A line as a terminal would end up showing it: after the last carriage
/// return (progress bars redraw with `\r`), without colour codes.
pub fn clean_line(line: &str) -> String {
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.rsplit('\r').next().unwrap_or(line);
    harness_core::text::safe(line).trim_end().to_string()
}
