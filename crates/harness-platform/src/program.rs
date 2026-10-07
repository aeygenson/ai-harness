//! Finding a program by name, the way the system's own shell does.
//!
//! On Linux and macOS a program is a file in one of the `PATH` folders. On
//! Windows a name has an extension too: `claude.exe`, but `codex.cmd` and
//! `npx.cmd` (programs installed with npm are small `.cmd` scripts). Rust
//! finds only `.exe` by itself, so the harness looks for every extension in
//! `PATHEXT` and starts the program by its full path. Rust's standard
//! library runs a `.cmd` through `cmd.exe` with safe quoting, and refuses
//! arguments it cannot pass safely.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Windows' list when `PATHEXT` is not set.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The full path of `name`, if it is installed: in `PATH`, or else in one of
/// the [`usual_places`]. A name with a folder in it (`./x`,
/// `C:\tools\x.exe`) is only checked, not searched.
pub fn find(name: &str) -> Option<PathBuf> {
    in_path(name).or_else(|| in_usual_places(name, crate::home::home_dir().as_deref()))
}

/// What to start for `name`. In `PATH` on Linux and macOS: the name itself,
/// which the system looks up on its own; on Windows the full path (so
/// `codex` becomes `…\codex.cmd`). Found only in one of the
/// [`usual_places`]: its full path. A program that is not found stays
/// `name`, and starting it fails with «not found», as before.
pub fn resolve(name: impl AsRef<OsStr>) -> PathBuf {
    let name = name.as_ref();
    if let Some(text) = name.to_str() {
        let found = if cfg!(windows) {
            find(text)
        } else if in_path(text).is_none() {
            in_usual_places(text, crate::home::home_dir().as_deref())
        } else {
            None
        };
        if let Some(found) = found {
            return found;
        }
    }
    PathBuf::from(name)
}

/// `name` in the folders of `PATH`.
fn in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    find_in(name, &path, &path_exts())
}

/// Windows' `PATHEXT`; empty on Linux and macOS.
fn path_exts() -> OsString {
    if cfg!(windows) {
        std::env::var_os("PATHEXT").unwrap_or_else(|| DEFAULT_PATHEXT.into())
    } else {
        OsString::new()
    }
}

/// Folders where installers put programs, besides `PATH`. A program started
/// from a desktop icon gets the desktop's `PATH`, and on Linux that often
/// lacks `~/.local/bin`, where Claude Code and Antigravity install
/// themselves: a shell adds it in `.bashrc` or `.profile`, the desktop does
/// not. Windows installers change the `PATH` of the whole account, but a
/// program already running keeps its old `PATH`: so after an install from
/// the Agents tab the new agent is found in its installer's folder.
pub fn usual_places(home: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(windows) {
        let mut places: Vec<PathBuf> = home
            .map(|h| h.join(".local").join("bin"))
            .into_iter()
            .collect();
        if let Some(roaming) = std::env::var_os("APPDATA") {
            places.push(Path::new(&roaming).join("npm"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            places.push(Path::new(&local).join("agy").join("bin"));
        }
        return places;
    }
    let mut places: Vec<PathBuf> = home
        .map(|home| {
            [
                ".local/bin",
                ".claude/local",
                ".npm-global/bin",
                ".cargo/bin",
            ]
            .iter()
            .map(|dir| home.join(dir))
            .collect()
        })
        .unwrap_or_default();
    places.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    places
}

/// `name` in one of the [`usual_places`] of `home`.
fn in_usual_places(name: &str, home: Option<&Path>) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    let places = std::env::join_paths(usual_places(home)).ok()?;
    find_in(name, &places, &path_exts())
}

/// Can a program here run its commands in a sandbox? On Linux that takes
/// bubblewrap (`bwrap`) in `PATH`; macOS and Windows need no extra program.
pub fn sandbox_ready() -> bool {
    sandbox_program().is_none_or(|program| in_path(program).is_some())
}

/// The extra program a sandbox needs on this system: `bwrap` on Linux,
/// nothing on macOS and Windows.
pub fn sandbox_program() -> Option<&'static str> {
    cfg!(target_os = "linux").then_some("bwrap")
}

/// The one command that installs or updates the harness with Git, Node.js
/// and Zed on this system (see `install/`). Running it again is safe.
pub fn installer_command() -> &'static str {
    on_this_system(
        "curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.sh | bash",
        "irm https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.ps1 | iex",
    )
}

/// `unix` on Linux and macOS, `windows` on Windows: for things written
/// differently per system, such as an installer command.
pub fn on_this_system<T>(unix: T, windows: T) -> T {
    if cfg!(windows) {
        windows
    } else {
        unix
    }
}

/// Where `npm install -g` puts programs without asking for `sudo`: on Linux
/// Node usually belongs to the system, so programs go into `~/.local`
/// (`~/.local/bin`, which [`usual_places`] covers), as the harness's own
/// installer does. On macOS (Homebrew's Node) and Windows npm's own place
/// is writable: `None`.
pub fn npm_user_prefix() -> Option<&'static str> {
    cfg!(target_os = "linux").then_some("$HOME/.local")
}

/// The shell that runs a command line: `sh -c` on Linux and macOS,
/// PowerShell on Windows (installers there are `irm … | iex`).
pub fn shell() -> (&'static str, &'static [&'static str]) {
    on_this_system(
        ("sh", &["-c"][..]),
        (
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command"][..],
        ),
    )
}

/// Looks for `name` in the folders of `path`, trying each extension of
/// `exts` (`;`-separated, as in `PATHEXT`; empty on Unix).
pub fn find_in(name: &str, path: &OsStr, exts: &OsStr) -> Option<PathBuf> {
    let exts: Vec<String> = exts
        .to_string_lossy()
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect();
    let candidates = |base: &Path| -> Vec<PathBuf> {
        let mut all = Vec::new();
        // A name that already has one of the extensions is tried as it is.
        let has_ext = exts
            .iter()
            .any(|e| name.to_ascii_lowercase().ends_with(&e.to_ascii_lowercase()));
        if exts.is_empty() || has_ext {
            all.push(base.join(name));
        }
        if !has_ext {
            all.extend(
                exts.iter()
                    .map(|e| base.join(format!("{name}{}", e.to_ascii_lowercase()))),
            );
        }
        all
    };
    if name.contains('/') || name.contains('\\') {
        let path = Path::new(name);
        let dir = path.parent().unwrap_or(Path::new(""));
        let file = path.file_name()?.to_str()?;
        let exts = OsString::from(exts.join(";"));
        return find_in(file, dir.as_os_str(), &exts);
    }
    std::env::split_paths(path)
        .flat_map(|dir| candidates(&dir))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_linux_needs_bubblewrap_for_a_sandbox() {
        if cfg!(target_os = "linux") {
            assert_eq!(sandbox_ready(), in_path("bwrap").is_some());
        } else {
            assert!(sandbox_ready());
        }
        assert_eq!(sandbox_program().is_some(), cfg!(target_os = "linux"));
    }

    #[test]
    fn the_installer_command_fits_this_system() {
        let script = if cfg!(windows) {
            "install.ps1"
        } else {
            "install.sh"
        };
        assert!(
            installer_command().contains(script),
            "{}",
            installer_command()
        );
    }

    #[test]
    fn a_program_is_found_with_its_windows_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        fs::write(dir.path().join("codex.cmd"), "").unwrap();
        fs::write(dir.path().join("claude.exe"), "").unwrap();
        let exts = OsStr::new(".COM;.EXE;.BAT;.CMD");
        assert_eq!(
            find_in("codex", &path, exts),
            Some(dir.path().join("codex.cmd"))
        );
        assert_eq!(
            find_in("claude", &path, exts),
            Some(dir.path().join("claude.exe"))
        );
        assert_eq!(
            find_in("claude.exe", &path, exts),
            Some(dir.path().join("claude.exe"))
        );
        assert_eq!(find_in("agy", &path, exts), None);
    }

    #[test]
    fn on_unix_the_name_is_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        fs::write(dir.path().join("codex"), "").unwrap();
        fs::write(dir.path().join("npx.cmd"), "").unwrap();
        let none = OsStr::new("");
        assert_eq!(
            find_in("codex", &path, none),
            Some(dir.path().join("codex"))
        );
        assert_eq!(find_in("npx", &path, none), None);
    }

    #[test]
    fn a_program_outside_path_is_found_in_the_usual_places() {
        let home = tempfile::tempdir().unwrap();
        let places = usual_places(Some(home.path()));
        let bin = home.path().join(".local").join("bin");
        assert!(places.contains(&bin));
        assert_eq!(in_usual_places("agy", Some(home.path())), None);
        let file = bin.join(if cfg!(windows) { "agy.exe" } else { "agy" });
        fs::create_dir_all(&bin).unwrap();
        fs::write(&file, "").unwrap();
        assert_eq!(in_usual_places("agy", Some(home.path())), Some(file));
        // A path is never looked up there.
        assert_eq!(in_usual_places("./agy", Some(home.path())), None);
    }

    #[test]
    fn a_path_is_checked_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("tool.cmd"), "").unwrap();
        let name = dir.path().join("tool").display().to_string();
        let found = find_in(&name, OsStr::new(""), OsStr::new(".EXE;.CMD"));
        assert_eq!(found, Some(dir.path().join("tool.cmd")));
        assert_eq!(
            resolve("/no/such/program"),
            PathBuf::from("/no/such/program")
        );
    }
}
