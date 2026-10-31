//! What the harness notes before an agent runs, and the first checks right
//! after it, before git runs again with whatever settings the agent left.

use std::path::PathBuf;

use super::{RunError, StopReason};
use crate::git::{LocalConfig, Repo};
use crate::task::agent_stubs::MissingStubs;
use crate::task::handoff::Role;
use crate::task::tripwire::{Changes, Snapshot, Watched};

/// The state before one attempt of a role.
pub(super) struct Before {
    head: Option<String>,
    branch: Option<String>,
    branch_heads: String,
    config_path: PathBuf,
    config: LocalConfig,
    hidden: Snapshot,
    stubs: MissingStubs,
}

impl Before {
    /// Notes the last commit, the branches, `.git/config`, the files the
    /// tripwire watches and which of Claude Code's stub files are missing.
    pub(super) fn take(repo: &Repo, watched: &Watched) -> Result<Self, RunError> {
        let config_path = repo.local_config_path()?;
        Ok(Self {
            head: repo.head()?,
            branch: repo.current_branch()?,
            branch_heads: repo.branch_heads()?,
            config: LocalConfig::read(&config_path)?,
            config_path,
            hidden: watched.snapshot(),
            stubs: MissingStubs::note(repo.root()),
        })
    }

    /// Compares with now: git's settings, the tripwire, the commit and the
    /// branches. Puts back what can be put back and returns why the task
    /// must stop, or `None` when all is as before. When Claude Code's stub
    /// block was there, removes its new empty stub files (see
    /// [`crate::task::agent_stubs`]), except after a commit or branch change
    /// (then the next run removes them).
    pub(super) fn check(
        &self,
        repo: &Repo,
        watched: &Watched,
        role: Role,
    ) -> Result<Option<StopReason>, RunError> {
        // First of all, before git runs again with the agent's settings or
        // hooks. The tripwire goes first: it runs no git and cannot fail, so
        // an agent cannot keep a planted hook by breaking `.git/config`.
        let changes = self.hidden.check_and_restore(watched);
        let config_changed = self.restore_config();
        // A file that could not be put back may be a planted hook that git
        // would run on the user's next command: this wins over everything,
        // also over an error with the settings, and its message asks the
        // user to check the commits and settings too.
        if !changes.not_put_back.is_empty() {
            return Ok(Some(hidden_changes(role, changes)));
        }
        let config_changed = config_changed?;
        // A new branch at the same commit would make the harness commit the
        // role's work on that branch, so a switch counts like a commit. So
        // does a made, moved or deleted branch: the harness later checks out
        // a task's branch and reads its settings, which no role may change.
        // Branches cannot be put back, and the next run would take them as
        // they are, so this reason wins when there are several. The stubs
        // stay here: whether the new commit holds them only `git status`
        // could tell, which does not run on a stop, and a tracked file is
        // never removed. The next run removes them if they are still empty
        // and untracked (`agent_stubs::remove_leftover_stubs`).
        let git_changed = repo.head()? != self.head
            || repo.current_branch()? != self.branch
            || repo.branch_heads()? != self.branch_heads;
        if git_changed {
            return Ok(Some(StopReason::AgentCommitted(role)));
        }
        let claude_stubs = changes.claude_stubs;
        let stop = if config_changed {
            Some(StopReason::GitConfigChanged(role))
        } else if changes.is_empty() {
            None
        } else {
            Some(hidden_changes(role, changes))
        };
        // Claude Code's empty stub files would otherwise end up in the role's
        // commit, or stop the next run as uncommitted changes, once
        // `info/exclude` no longer hides them. Only when its block was there:
        // another agent's empty file is its own work.
        if claude_stubs {
            match stop {
                // No git here: something was changed that should not have
                // been, and `git status` would run with whatever else the
                // agent planted. A file that cannot be removed is shown to
                // the user by the next run; the stop reason matters more.
                Some(_) => self.stubs.remove_empty_without_git(),
                // Only now, with nothing left that could make git run the
                // agent's command (`git status` reads `~/.gitconfig` and its
                // filters).
                None => self.stubs.remove_new_empty(repo)?,
            }
        }
        Ok(stop)
    }

    /// Puts `.git/config` back when it changed; `true` when it did. No git
    /// runs here: with broken settings it would not start. A file that can
    /// no longer be read (`chmod 000`, too big) has changed too.
    fn restore_config(&self) -> Result<bool, RunError> {
        let changed = LocalConfig::read(&self.config_path).map_or(true, |now| now != self.config);
        if changed {
            self.config.put_back(&self.config_path)?;
        }
        Ok(changed)
    }
}

/// The stop reason for what the tripwire found.
fn hidden_changes(role: Role, changes: Changes) -> StopReason {
    StopReason::HiddenChanges {
        role,
        put_back: changes.put_back,
        reported: changes.reported,
        not_put_back: changes.not_put_back,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // As root (in a container) every file can be read, so there this passes
    // also without the fix; as a user (CI) the unreadable file used to end
    // the check with an error before anything was put back.
    #[cfg(unix)]
    #[test]
    fn unreadable_settings_are_put_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let watched = Watched::default();
        let before = Before::take(&repo, &watched).unwrap();
        let path = repo.local_config_path().unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        let stop = before.check(&repo, &watched, Role::Developer).unwrap();

        assert_eq!(stop, Some(StopReason::GitConfigChanged(Role::Developer)));
        assert_eq!(repo.local_config().unwrap(), before.config);
    }
}
