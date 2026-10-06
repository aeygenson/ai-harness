//! What goes into a role's temporary home: toolchain paths, settings with deny rules,
//! and a copy of the saved login.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use harness_core::task::handoff::Role;

use super::NOT_COPIED;

/// Toolchains installed in the user's home folder that find themselves
/// through `$HOME` unless their variable is set: `(variable, folder in home)`.
/// With the temporary `HOME` of a role they would look in the wrong place.
pub(super) const TOOLCHAIN_HOMES: &[(&str, &str)] = &[
    // Rust
    ("CARGO_HOME", ".cargo"),
    ("RUSTUP_HOME", ".rustup"),
    // Go: downloaded modules and installed tools
    ("GOPATH", "go"),
    // Python versions from pyenv
    ("PYENV_ROOT", ".pyenv"),
    // Node versions from nvm
    ("NVM_DIR", ".nvm"),
];

/// The [`TOOLCHAIN_HOMES`] variables pointing at the real home folder, for
/// those Lisa has not set herself and whose folder exists.
pub(super) fn toolchain_env(real_home: &Path) -> Vec<(&'static str, PathBuf)> {
    TOOLCHAIN_HOMES
        .iter()
        .copied()
        .filter(|(name, _)| std::env::var_os(name).is_none())
        .map(|(name, folder)| (name, real_home.join(folder)))
        .filter(|(_, path)| path.is_dir())
        .collect()
}

/// Commands no role may run: the harness makes the commits.
pub(super) const DENIED_FOR_ALL: &[&str] = &["git commit", "git push"];

/// Commands the roles that only look (Architect, Security) may not run: they
/// change files, the git state, install or publish packages, or reach the
/// network. Reading, running tests and audit tools stay allowed. The list
/// cannot be complete; the git check after the role still catches any
/// changed file.
pub(super) const DENIED_FOR_READERS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "git add",
    "git checkout",
    "git reset",
    "git restore",
    "git stash",
    "cargo build",
    "cargo install",
    "cargo run",
    "cargo publish",
    "npm install",
    "npm publish",
    "pip install",
    "go install",
    "dotnet build",
    "curl",
    "wget",
];

/// The role's `settings.json`. `deny` beats everything else.
pub(super) fn settings(role: Role) -> String {
    let mut deny: Vec<&str> = DENIED_FOR_ALL.to_vec();
    if matches!(role, Role::Architect | Role::Security) {
        deny.extend(DENIED_FOR_READERS);
    }
    let deny: Vec<String> = deny.iter().map(|c| format!("command({c})")).collect();
    let settings = serde_json::json!({
        "permissions": { "allow": [], "deny": deny },
        "allowNonWorkspaceAccess": false,
    });
    serde_json::to_string_pretty(&settings).expect("settings are plain JSON")
}

/// Copies the saved login folder, leaving out logs and history.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if NOT_COPIED.contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(&name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}
