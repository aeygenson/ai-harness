//! Opening a file in Lisa's editor and waiting until she closes it.
//!
//! Zed first (`zed --wait`, from `PATH` or `~/.local/bin`), then `$VISUAL`,
//! `$EDITOR`, and KDE's `kate --block`. The TUI gives the terminal back while
//! the editor is open, so a terminal editor such as vim works too.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The command that edits `file` and returns when the editor is closed.
pub fn command(file: &Path) -> Result<Command, String> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let path = env::var_os("PATH").unwrap_or_default();
    command_with(file, &path, home.as_deref(), &|name| env::var(name).ok())
}

fn command_with(
    file: &Path,
    path: &std::ffi::OsStr,
    home: Option<&Path>,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<Command, String> {
    let find = |program: &str| {
        env::split_paths(path)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    };
    let zed = find("zed").or_else(|| {
        home.map(|h| h.join(".local/bin/zed"))
            .filter(|p| p.is_file())
    });
    if let Some(zed) = zed {
        let mut command = Command::new(zed);
        command.arg("--wait").arg(file);
        return Ok(command);
    }
    for name in ["VISUAL", "EDITOR"] {
        let value = var(name).unwrap_or_default();
        let mut words = value.split_whitespace();
        if let Some(program) = words.next() {
            let mut command = Command::new(program);
            command.args(words).arg(file);
            return Ok(command);
        }
    }
    if let Some(kate) = find("kate") {
        let mut command = Command::new(kate);
        command.arg("--block").arg(file);
        return Ok(command);
    }
    Err("no editor found: install Zed, or set $EDITOR".to_string())
}

/// Edits `file` and waits. `Err` says why it did not work.
pub fn run(file: &Path) -> Result<(), String> {
    let mut command = command(file)?;
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} ended with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn zed_comes_first_then_the_editor_variables() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let file = Path::new("/p/skill.md");
        let none = |_: &str| None;
        let vim = |name: &str| (name == "EDITOR").then(|| "vim -n".to_string());
        let empty = std::ffi::OsStr::new("");

        assert!(command_with(file, empty, Some(home.path()), &none).is_err());
        let command = command_with(file, empty, Some(home.path()), &vim).unwrap();
        assert_eq!(args(&command), ["vim", "-n", "/p/skill.md"]);

        fs::write(bin.join("zed"), "").unwrap();
        let command = command_with(file, empty, Some(home.path()), &vim).unwrap();
        let zed = bin.join("zed").display().to_string();
        assert_eq!(args(&command), [zed.as_str(), "--wait", "/p/skill.md"]);
    }
}
