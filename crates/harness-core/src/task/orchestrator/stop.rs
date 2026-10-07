//! Why the run loop returned ([`StopReason`]) and the harness's own errors ([`RunError`]).

use std::path::PathBuf;

use crate::git::GitError;
use crate::task::handoff::Role;
use crate::task::store::StoreError;
use crate::task::WaitReason;

/// Why `run` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The task is finished.
    Done,
    /// The task waits for Lisa, for the reason given.
    WaitingForHuman(WaitReason),
    /// The subscription's usage limit stopped this role; run again later.
    UsageLimitReached(Role),
    /// The role did not produce an acceptable handoff after every attempt.
    RoleFailed {
        /// The role that failed.
        role: Role,
        /// Why the last attempt failed, ready to show to Lisa.
        problem: String,
    },
    /// [`MAX_STEPS_PER_RUN`](super::MAX_STEPS_PER_RUN) roles ran in this call;
    /// run again to continue.
    StepLimitReached,
    /// The project had uncommitted changes before a role started. Lisa commits
    /// or removes them, then runs again.
    DirtyWorkingTree(Vec<String>),
    /// The role changed files it may not touch. The changes are left in place,
    /// uncommitted, so Lisa can look at them.
    ForbiddenChanges {
        /// The role that made the changes.
        role: Role,
        /// The forbidden files, as paths inside the project.
        files: Vec<String>,
    },
    /// The role's change is too big to commit (see [`MAX_CHANGE_BYTES`](super::MAX_CHANGE_BYTES)):
    /// each entry is a file or folder and its size in bytes, `.` meaning the
    /// change as a whole. Everything is left in place, uncommitted, so Lisa
    /// can add it to `.gitignore` or delete it.
    TooLarge {
        /// The role that made the change.
        role: Role,
        /// Each too big file or folder with its size in bytes.
        files: Vec<(String, u64)>,
    },
    /// The agent made a git commit itself, which agents must never do.
    AgentCommitted(Role),
    /// The agent changed files no role may change: agent instructions and
    /// settings (`CLAUDE.md`, `.claude/`, ...) or the harness's own files
    /// (`.harness/`). They are put back at once, so they never reach the next
    /// role; what the agent wrote is in `log`. Its other changes are left
    /// uncommitted for Lisa to look at.
    ProtectedFilesChanged {
        /// The role that made the changes.
        role: Role,
        /// The protected files it changed, as paths inside the project.
        files: Vec<String>,
        /// The failure log with what the agent wrote in them.
        log: PathBuf,
    },
    /// The agent changed files `git status` does not show (see
    /// [`crate::task::tripwire`]): git's hooks or `info/` (put back at once)
    /// or files outside the project such as shell start-up files or the
    /// global git settings (left for Lisa to check, not put back).
    HiddenChanges {
        /// The role that made the changes.
        role: Role,
        /// Files in git's own folders, now as they were before the role.
        put_back: Vec<PathBuf>,
        /// Files outside the project that changed during the role.
        reported: Vec<PathBuf>,
    },
    /// The agent changed the repository's settings (`.git/config`), where a
    /// setting can make git run any command. The old settings are put back;
    /// the role's other changes are left uncommitted for Lisa to look at.
    GitConfigChanged(Role),
}

/// A real problem of the harness itself, not a mistake of an agent.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Saving or reading the task's files failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// Reading or putting back git's hooks or `info/` failed.
    #[error("cannot check git's hooks: {0}")]
    Tripwire(#[from] std::io::Error),
    /// An unfinished task already has this text: a second one is not created.
    #[error(
        "{task} already has this text and is not done ({stage}); continue it or change the text"
    )]
    SameTask {
        /// The id of the unfinished task, for example `task-001`.
        task: String,
        /// Where that task is now, as text for Lisa.
        stage: String,
    },
}
