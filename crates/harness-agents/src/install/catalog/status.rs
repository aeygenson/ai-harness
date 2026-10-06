//! What was found for one agent on this computer, and the commands offered for it.

use std::path::{Path, PathBuf};

use super::entries::Entry;
use super::older;

/// What was found for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The catalog entry this status is about.
    pub entry: Entry,
    /// The program's full path; `None`: not installed.
    pub path: Option<PathBuf>,
    /// What `--version` printed, reduced to the number.
    pub version: Option<String>,
    /// Why there is no version, when the program is there.
    pub problem: Option<String>,
    /// Is a login saved? `None` for agents the harness does not run yet.
    pub login: Option<bool>,
}

impl Status {
    /// Its program is here. An agent that is a model inside another agent's
    /// program counts only when the harness runs it: Claude Code alone is
    /// not GLM, which also needs its plan's key and an adapter.
    pub fn installed(&self) -> bool {
        self.path.is_some() && (self.entry.inside.is_none() || self.entry.runs())
    }

    /// The program this agent lives inside is here, but the agent itself is
    /// not usable yet (see [`Status::installed`]).
    pub fn host_only(&self) -> bool {
        self.path.is_some() && !self.installed()
    }

    /// What can be done now: update an installed agent, install a missing
    /// one, nothing for a [`host_only`](Status::host_only) one. With the command for
    /// this computer, if the maker gives one.
    pub fn action(&self) -> Option<(Action, Option<String>)> {
        if self.host_only() {
            None
        } else if self.installed() {
            let command = match (self.entry.npm_package(), self.npm_place()) {
                (Some(_), NpmPlace::System) => None,
                (Some(package), NpmPlace::User(prefix)) => {
                    Some(npm_command("install", prefix, &format!("{package}@latest")))
                }
                (None, _) => self.entry.update.here(),
            };
            Some((Action::Update, command))
        } else {
            Some((Action::Install, self.entry.install.here()))
        }
    }

    /// For an agent npm installed into the system's own folders (on Linux,
    /// with `sudo`): the command to run in a terminal for `action`. The
    /// harness cannot run it, it would ask for a password; installing a
    /// second copy into `~/.local` instead would leave the old one in use
    /// by everything else.
    pub fn needs_sudo(&self, action: Action) -> Option<String> {
        let package = self.entry.npm_package()?;
        if !self.installed() || self.npm_place() != NpmPlace::System {
            return None;
        }
        match action {
            Action::Install => None,
            Action::Update => Some(format!("sudo npm install -g {package}@latest")),
            Action::Remove => Some(format!("sudo npm uninstall -g {package}")),
        }
    }

    /// Where npm keeps this installed agent, judged by its program's path.
    fn npm_place(&self) -> NpmPlace {
        let Some(prefix) = harness_platform::program::npm_user_prefix() else {
            // macOS (Homebrew) and Windows: npm's own folder is the user's.
            return NpmPlace::User(None);
        };
        let home = harness_platform::home::home_dir();
        let path = self.path.as_deref().unwrap_or(Path::new(""));
        match home {
            Some(home) if path.starts_with(home.join(".local")) => NpmPlace::User(Some(prefix)),
            // nvm and other npm folders of the user's own.
            Some(home) if path.starts_with(&home) => NpmPlace::User(None),
            _ => NpmPlace::System,
        }
    }

    /// How to take the agent's program off this computer: `npm uninstall`
    /// for what npm installed, otherwise deleting the program file that was
    /// found (Claude Code, Antigravity and Cursor install one command there).
    /// Its settings, saved logins and data stay. `None` when it is not
    /// installed, or lives inside another agent's program: removing Claude
    /// Code for «Claude Code + GLM» would surprise.
    pub fn removal(&self) -> Option<String> {
        if !self.installed() || self.entry.inside.is_some() {
            return None;
        }
        let path = self.path.as_ref()?;
        if let Some(package) = self.entry.npm_package() {
            return match self.npm_place() {
                NpmPlace::User(prefix) => Some(npm_command("uninstall", prefix, package)),
                NpmPlace::System => None,
            };
        }
        // The maker's own way, for an install in the usual place.
        let local = harness_platform::home::home_dir()
            .is_some_and(|home| path.starts_with(home.join(".local")));
        if let (true, Some(command)) = (local, self.entry.remove.here()) {
            return Some(command);
        }
        let path = path.to_string_lossy();
        Some(harness_platform::program::on_this_system(
            format!("rm -f '{}'", path.replace('\'', "'\\''")),
            format!("Remove-Item -LiteralPath '{}'", path.replace('\'', "''")),
        ))
    }

    /// Installed, but older than the harness was checked with.
    pub fn old(&self) -> bool {
        match (self.version.as_deref(), self.entry.min_version) {
            (Some(version), Some(min)) => older(version, min),
            _ => false,
        }
    }
}

/// Where npm keeps an installed agent: the user's own folders (with the
/// `--prefix` it was installed with, if any) or the system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NpmPlace {
    User(Option<&'static str>),
    System,
}

/// `npm <verb> -g [--prefix "<prefix>"] <what>`.
pub(super) fn npm_command(verb: &str, prefix: Option<&str>, what: &str) -> String {
    match prefix {
        Some(prefix) => format!("npm {verb} -g --prefix \"{prefix}\" {what}"),
        None => format!("npm {verb} -g {what}"),
    }
}

/// What can be done with an agent on the Agents tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Install the agent.
    Install,
    /// Update the installed agent to its newest version.
    Update,
    /// Take the agent off this computer.
    Remove,
}
