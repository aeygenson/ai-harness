//! Questions about the project's git history: an older text of a file, the
//! files of a commit, the day of a change.

use super::Repo;

impl Repo {
    /// The text of `file` (relative to the project, with `/`) in the commit
    /// checked out; `None` when it is not there.
    pub fn file_at_head(&self, file: &str) -> Option<String> {
        self.git(&["show", &format!("HEAD:{file}")]).ok()
    }

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
        let commit = self.commit_adding(added)?;
        let out = self
            .git(&[
                "show",
                "--no-renames",
                "--name-status",
                "-z",
                "--format=",
                &commit,
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

    /// The changes of the commit that added `added` as a unified diff without
    /// context lines (`-U0`). `None` if no commit added it or git failed.
    /// Renames count as a deleted and an added file.
    pub fn patch_of_commit_adding(&self, added: &str) -> Option<String> {
        let commit = self.commit_adding(added)?;
        self.git(&[
            "show",
            "--no-renames",
            // An agent could have set up its own diff program or text
            // converter in the project's git settings; these run nothing.
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=0",
            "--format=",
            &commit,
        ])
        .ok()
    }

    /// The hash of the commit that added `added`, or `None`.
    fn commit_adding(&self, added: &str) -> Option<String> {
        let commit = self
            .git_literal(&["log", "-1", "--diff-filter=A", "--format=%H", "--", added])
            .ok()?;
        let commit = commit.trim();
        (!commit.is_empty()).then(|| commit.to_string())
    }

    /// The files changed between the last commit that changed `path`
    /// (relative to the project, with `/`) and the commit checked out;
    /// `None` if `path` was never committed.
    pub fn changed_since_last_change(&self, path: &str) -> Option<Vec<String>> {
        let commit = self
            .git_literal(&["log", "-1", "--format=%H", "--", path])
            .ok()?;
        let commit = commit.trim();
        if commit.is_empty() {
            return None;
        }
        let out = self
            .git(&["diff", "--name-only", "-z", commit, "HEAD"])
            .ok()?;
        Some(
            out.split('\0')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect(),
        )
    }

    /// Every file under `path` (relative to the project, with `/`) with the
    /// time of the commit that added it, in seconds since 1970, newest
    /// commits first. Empty when nothing there was committed or git failed.
    pub fn added_times(&self, path: &str) -> Vec<(String, u64)> {
        // Each commit prints a line «\u{1}<time>» and then the files it
        // added; no file name starts with that control character.
        let Ok(out) = self.git_literal(&[
            "log",
            "--no-renames",
            "--diff-filter=A",
            "--name-only",
            "--format=%x01%ct",
            "--",
            path,
        ]) else {
            return Vec::new();
        };
        let mut time = None;
        let mut files = Vec::new();
        for line in out.lines().filter(|l| !l.is_empty()) {
            if let Some(seconds) = line.strip_prefix('\u{1}') {
                time = seconds.trim().parse().ok();
            } else if let Some(time) = time {
                files.push((line.to_string(), time));
            }
        }
        files
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Writes `file` and commits it as if at `seconds` since 1970.
    fn commit_at(repo: &Repo, file: &str, text: &str, seconds: u64) {
        let path = repo.root().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        repo.git(&["add", "--all"]).unwrap();
        let date = format!("@{seconds} +0000");
        repo.run(
            &["commit", "-q", "-m", file],
            &[("GIT_COMMITTER_DATE", &date)],
        )
        .unwrap();
    }

    #[test]
    fn each_added_file_comes_with_the_time_of_its_commit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        // No commit yet: nothing, and no error.
        assert_eq!(repo.added_times("runs"), []);

        commit_at(&repo, "runs/t/task.md", "do it", 1_000);
        commit_at(
            &repo,
            "runs/t/round-01/01-developer/handoff.json",
            "{}",
            1_600,
        );
        commit_at(&repo, "README.md", "outside", 2_000);
        // Changing a file later does not move its time.
        commit_at(&repo, "runs/t/task.md", "do it well", 3_000);

        assert_eq!(
            repo.added_times("runs"),
            [
                (
                    "runs/t/round-01/01-developer/handoff.json".to_string(),
                    1_600
                ),
                ("runs/t/task.md".to_string(), 1_000),
            ]
        );
    }
}
