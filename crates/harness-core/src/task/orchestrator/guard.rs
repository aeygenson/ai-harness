//! What the harness notes before an agent runs, and the first checks right
//! after it, before git runs again with whatever settings the agent left.

use super::{RunError, StopReason};
use crate::git::Repo;
use crate::task::agent_stubs::MissingStubs;
use crate::task::handoff::Role;
use crate::task::tripwire::{Snapshot, Watched};

/// The state before one attempt of a role.
pub(super) struct Before {
    head: Option<String>,
    branch: Option<String>,
    branch_heads: String,
    config: Vec<u8>,
    hidden: Snapshot,
    stubs: MissingStubs,
}

impl Before {
    /// Notes the last commit, the branches, `.git/config`, the files the
    /// tripwire watches and which of Claude Code's stub files are missing.
    pub(super) fn take(repo: &Repo, watched: &Watched) -> Result<Self, RunError> {
        Ok(Self {
            head: repo.head()?,
            branch: repo.current_branch()?,
            branch_heads: repo.branch_heads()?,
            config: repo.local_config()?,
            hidden: watched.snapshot()?,
            stubs: MissingStubs::note(repo.root()),
        })
    }

    /// Compares with now: git's settings, the tripwire, the commit and the
    /// branches. Puts back what can be put back and returns why the task
    /// must stop, or `None` when all is as before.
    pub(super) fn check(
        &self,
        repo: &Repo,
        watched: &Watched,
        role: Role,
    ) -> Result<Option<StopReason>, RunError> {
        // First of all, before git runs again with the agent's settings or hooks.
        let config_changed = repo.local_config()? != self.config;
        if config_changed {
            repo.restore_local_config(&self.config)?;
        }
        let changes = self.hidden.check_and_restore(watched)?;
        // Claude Code's empty stub files would otherwise end up in the role's
        // commit once `info/exclude` no longer hides them.
        self.stubs.remove_new_empty()?;
        // A new branch at the same commit would make the harness commit the
        // role's work on that branch, so a switch counts like a commit. So
        // does a made, moved or deleted branch: the harness later checks out
        // a task's branch and reads its settings, which no role may change.
        // Branches cannot be put back, and the next run would take them as
        // they are, so this reason wins when there are several.
        let git_changed = repo.head()? != self.head
            || repo.current_branch()? != self.branch
            || repo.branch_heads()? != self.branch_heads;
        if git_changed {
            return Ok(Some(StopReason::AgentCommitted(role)));
        }
        if config_changed {
            return Ok(Some(StopReason::GitConfigChanged(role)));
        }
        if !changes.is_empty() {
            return Ok(Some(StopReason::HiddenChanges {
                role,
                put_back: changes.put_back,
                reported: changes.reported,
            }));
        }
        Ok(None)
    }
}
