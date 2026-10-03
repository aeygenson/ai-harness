//! The home folder: `$HOME` on Linux and macOS, `%USERPROFILE%` on Windows
//! (for example `C:\Users\Son`). The harness keeps its own files in
//! `.harness` inside it on every system.

use std::path::PathBuf;

/// The folder inside the home folder where the harness keeps its files.
pub const HARNESS_DIR: &str = ".harness";

/// The home folder, if the system knows one.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|p| !p.as_os_str().is_empty())
}

/// `~/.harness`: the project list, logins, plugin catalogs, TUI settings.
pub fn harness_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(HARNESS_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_harness_folder_is_inside_the_home_folder() {
        // CI machines and Lisa's computers all have a home folder.
        let home = home_dir().unwrap();
        assert_eq!(harness_dir().unwrap(), home.join(".harness"));
    }
}
