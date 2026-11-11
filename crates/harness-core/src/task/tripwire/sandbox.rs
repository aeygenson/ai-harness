//! The empty folder Claude Code's sandbox makes in git's `info/`.
//!
//! With `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` Claude Code 2.1.322 makes an empty
//! folder `.claude-sandbox-noreplace` next to `info/exclude` when it starts,
//! also when it runs no command, and leaves it there. In a task's own
//! worktree `.git` is a file, so Claude Code cannot write its block of stub
//! lines there: this folder is then the only sign that its empty stub files
//! are its own (see [`crate::task::agent_stubs`]). Git does not read folders
//! inside `info/`, so an empty one changes nothing.

use std::fs;
use std::path::Path;

use super::{Content, Snapshot, Watched};

/// The folder's name inside `info/`.
const SANDBOX_FOLDER: &str = ".claude-sandbox-noreplace";

impl Watched {
    /// Is `path` Claude Code's new, empty sandbox folder in `info/`?
    /// Anything inside it (`now` holds what is there) makes it a real change.
    pub(super) fn is_sandbox_folder(
        &self,
        path: &Path,
        before: Option<&Content>,
        now: &Snapshot,
    ) -> bool {
        let Some(info) = self.exclude_file.parent() else {
            return false;
        };
        path == info.join(SANDBOX_FOLDER)
            && before.is_none()
            && matches!(now.files.get(path), Some(Some(Content::Folder(_))))
            && !now.files.keys().any(|p| p != path && p.starts_with(path))
    }
}

/// Removes the sandbox folder an unwatched run left (the user's own Claude
/// Code, the retrospective) in the git folder `common`, so that the tripwire
/// sees it appear when the next role runs Claude Code.
///
/// Best effort: `remove_dir` only removes an empty folder, so one with
/// something inside stays (and a link in its place, on Linux and macOS).
/// Then a role in a task's own worktree has no sign that Claude Code ran,
/// and its empty stub files are not removed: they show up in the role's
/// commit, where the user can see them. Nothing else depends on it.
pub(crate) fn remove_leftover_sandbox_folder(common: &Path) {
    // Ignored on purpose: see "Best effort" above.
    let _ = fs::remove_dir(common.join("info").join(SANDBOX_FOLDER));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;

    #[test]
    fn an_empty_leftover_folder_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let folder = dir.path().join(".git/info").join(SANDBOX_FOLDER);
        fs::create_dir_all(&folder).unwrap();

        remove_leftover_sandbox_folder(&repo.git_common_dir().unwrap());

        assert!(!folder.exists());
    }

    #[test]
    fn a_leftover_folder_with_something_inside_stays() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let folder = dir.path().join(".git/info").join(SANDBOX_FOLDER);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a"), "a").unwrap();

        remove_leftover_sandbox_folder(&repo.git_common_dir().unwrap());

        assert!(folder.join("a").exists());
    }
}
