//! Opening a file in Lisa's editor: to edit it and wait until she closes it,
//! or only to look at it (Zed opens it and the TUI goes on).
//!
//! Zed first (`zed --wait`), then `$VISUAL`, `$EDITOR`, and last the
//! system's own editor: KDE's `kate --block` on Linux, TextEdit on macOS
//! (`open -W -t`, which waits until it is closed). The TUI gives the
//! terminal back while the editor is open, so a terminal editor such as vim
//! works too.
//!
//! Zed is found in `PATH`, in `~/.local/bin` (its Linux install), or on
//! macOS in the Zed app itself (`/Applications` or `~/Applications`), whose
//! `cli` is the same `zed` command.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where macOS keeps apps for everyone.
const MAC_APPS: &str = "/Applications";

/// Inside an app folder: Zed's command-line tool.
const ZED_APP_CLI: &str = "Zed.app/Contents/MacOS/cli";

/// The places this system can look in: `PATH`, the home folder, and the
/// system's own apps folder.
struct Places<'a> {
    path: &'a std::ffi::OsStr,
    home: Option<&'a Path>,
    apps: &'a Path,
}

/// The command that edits `file` and returns when the editor is closed.
pub fn command(file: &Path) -> Result<Command, String> {
    let home = harness_platform::home::home_dir();
    let path = env::var_os("PATH").unwrap_or_default();
    let places = Places {
        path: &path,
        home: home.as_deref(),
        apps: Path::new(MAC_APPS),
    };
    command_with(file, &places, &|name| env::var(name).ok())
}

fn command_with(
    file: &Path,
    places: &Places,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<Command, String> {
    if let Some(zed) = find_zed(places) {
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
    system_editor(file, places)
        .ok_or_else(|| "no editor found: install Zed, or set $EDITOR".to_string())
}

/// The editor that comes with the system, opened so that it is waited for.
fn system_editor(file: &Path, places: &Places) -> Option<Command> {
    if cfg!(target_os = "macos") {
        // `open` comes with every Mac; `-W` waits, `-t` uses the text editor.
        let mut command = Command::new("open");
        command.args(["-W", "-t"]).arg(file);
        Some(command)
    } else {
        let kate = find(places.path, "kate")?;
        let mut command = Command::new(kate);
        command.arg("--block").arg(file);
        Some(command)
    }
}

/// `program` in one of the folders of `path`.
fn find(path: &std::ffi::OsStr, program: &str) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

/// Zed, from `PATH`, `~/.local/bin`, or (on macOS) the Zed app.
fn find_zed(places: &Places) -> Option<PathBuf> {
    if let Some(zed) = find(places.path, "zed") {
        return Some(zed);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = places.home {
        candidates.push(home.join(".local/bin/zed"));
    }
    if cfg!(target_os = "macos") {
        candidates.push(places.apps.join(ZED_APP_CLI));
        if let Some(home) = places.home {
            candidates.push(home.join("Applications").join(ZED_APP_CLI));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// The command that opens `file` in Zed and returns at once; `None` without Zed.
pub fn viewer(file: &Path) -> Option<Command> {
    let home = harness_platform::home::home_dir();
    let path = env::var_os("PATH").unwrap_or_default();
    let places = Places {
        path: &path,
        home: home.as_deref(),
        apps: Path::new(MAC_APPS),
    };
    viewer_with(file, &places)
}

fn viewer_with(file: &Path, places: &Places) -> Option<Command> {
    let mut command = Command::new(find_zed(places)?);
    command.arg(file);
    Some(command)
}

/// Opens `file` in Zed without waiting. `Err` says why it did not work.
pub fn view(mut command: Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            // Zed's command ends soon; wait for it aside so it leaves nothing behind.
            std::thread::spawn(move || child.wait());
        })
        .map_err(|e| format!("cannot start {program}: {e}"))
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

    /// Places with nothing in `PATH` and an empty apps folder.
    fn places<'a>(home: &'a Path, apps: &'a Path) -> Places<'a> {
        Places {
            path: std::ffi::OsStr::new(""),
            home: Some(home),
            apps,
        }
    }

    #[test]
    fn zed_comes_first_then_the_editor_variables() {
        let home = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let file = Path::new("/p/skill.md");
        let none = |_: &str| None;
        let vim = |name: &str| (name == "EDITOR").then(|| "vim -n".to_string());
        let places = places(home.path(), apps.path());

        // Without Zed or a variable: the system's editor, which on Linux
        // must be installed (kate) and on a Mac always is (TextEdit).
        let fallback = command_with(file, &places, &none);
        if cfg!(target_os = "macos") {
            assert_eq!(
                args(&fallback.unwrap()),
                ["open", "-W", "-t", "/p/skill.md"]
            );
        } else {
            assert!(fallback.is_err());
        }
        let command = command_with(file, &places, &vim).unwrap();
        assert_eq!(args(&command), ["vim", "-n", "/p/skill.md"]);

        fs::write(bin.join("zed"), "").unwrap();
        let command = command_with(file, &places, &vim).unwrap();
        let zed = bin.join("zed").display().to_string();
        assert_eq!(args(&command), [zed.as_str(), "--wait", "/p/skill.md"]);
    }

    #[test]
    fn zed_views_without_waiting() {
        let home = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let file = Path::new("/p/docs/design.md");
        let places = places(home.path(), apps.path());
        assert!(viewer_with(file, &places).is_none());

        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("zed"), "").unwrap();
        let command = viewer_with(file, &places).unwrap();
        let zed = bin.join("zed").display().to_string();
        assert_eq!(args(&command), [zed.as_str(), "/p/docs/design.md"]);
    }

    #[test]
    fn on_a_mac_zed_is_found_in_its_app() {
        let home = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let cli = apps.path().join(ZED_APP_CLI);
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::write(&cli, "").unwrap();
        let found = find_zed(&places(home.path(), apps.path()));
        if cfg!(target_os = "macos") {
            assert_eq!(found, Some(cli));
        } else {
            assert_eq!(found, None);
        }
    }
}
