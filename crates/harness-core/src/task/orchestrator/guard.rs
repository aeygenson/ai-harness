//! What the harness notes before an agent runs, and the first checks right
//! after it, before git runs again with whatever settings the agent left.

use super::{RunError, StopReason};
use crate::git::Repo;
use crate::task::handoff::Role;
use crate::task::tripwire::{Snapshot, Watched};

/// The state before one attempt of a role.
pub(super) struct Before {
    head: Option<String>,
    config: Vec<u8>,
    hidden: Snapshot,
}

impl Before {
    /// Notes the last commit, `.git/config` and the files the tripwire watches.
    pub(super) fn take(repo: &Repo, watched: &Watched) -> Result<Self, RunError> {
        Ok(Self {
            head: repo.head()?,
            config: repo.local_config()?,
            hidden: watched.snapshot()?,
        })
    }

    /// Compares with now: git's settings, the tripwire, the last commit.
    /// Puts back what can be put back and returns why the task must stop,
    /// or `None` when all is as before.
    pub(super) fn check(
        &self,
        repo: &Repo,
        watched: &Watched,
        role: Role,
    ) -> Result<Option<StopReason>, RunError> {
        // First of all, before git runs again with the agent's settings.
        if repo.local_config()? != self.config {
            repo.restore_local_config(&self.config)?;
            return Ok(Some(StopReason::GitConfigChanged(role)));
        }
        let changes = self.hidden.check_and_restore(watched)?;
        if !changes.is_empty() {
            return Ok(Some(StopReason::HiddenChanges {
                role,
                put_back: changes.put_back,
                reported: changes.reported,
            }));
        }
        if repo.head()? != self.head {
            return Ok(Some(StopReason::AgentCommitted(role)));
        }
        Ok(None)
    }
}
