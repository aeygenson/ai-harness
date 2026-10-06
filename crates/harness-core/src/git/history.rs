//! Questions about the project's git history: an older text of a file, the
//! files of a commit, the day of a change.

use super::Repo;

impl Repo {
    /// The text of `file` in the last commit that changed `changed` (both
    /// relative to the project, with `/`). `None` if there is no such commit
    /// or the file did not exist in it.
    pub fn file_at_last_change(&self, changed: &str, file: &str) -> Option<String> {
        let commit = self
            .git_literal(&["log", "-1", "--format=%H", "--", changed])
            .ok()?;
        let commit = commit.trim();
        if commit.is_empty() {
            return None;
        }
        self.git(&["show", &format!("{commit}:{file}")]).ok()
    }

    /// The files of the commit that added `added` (relative to the project,
    /// with `/`), each with how it changed: `A` added, `M` changed, `D`
    /// deleted. `None` if no commit added it. Renames count as a deleted
    /// and an added file.
    pub fn files_of_commit_adding(&self, added: &str) -> Option<Vec<(char, String)>> {
        let commit = self
            .git_literal(&["log", "-1", "--diff-filter=A", "--format=%H", "--", added])
            .ok()?;
        let commit = commit.trim();
        if commit.is_empty() {
            return None;
        }
        let out = self
            .git(&[
                "show",
                "--no-renames",
                "--name-status",
                "-z",
                "--format=",
                commit,
            ])
            .ok()?;
        // `-z` gives "status\0path\0" pairs and never quotes paths.
        let mut parts = out
            .split('\0')
            .map(str::trim_start)
            .filter(|p| !p.is_empty());
        let mut files = Vec::new();
        while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
            files.push((status.chars().next().unwrap_or('M'), path.to_string()));
        }
        Some(files)
    }

    /// The day of the last commit that changed `path` (relative to the
    /// project), as `2026-10-02`; `None` if it was never committed.
    pub fn last_change_date(&self, path: &str) -> Option<String> {
        let day = self
            .git_literal(&["log", "-1", "--format=%cs", "--", path])
            .ok()?;
        let day = day.trim();
        (!day.is_empty()).then(|| day.to_string())
    }
}
