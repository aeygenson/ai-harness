//! Sending a task's branch to GitHub and bringing a merged pull request back:
//! the only git commands of the harness that use the network. The address is
//! always given in full (made from `[github]` in `harness.toml`), never a
//! remote's name, and nothing is forced: the harness only adds commits to a
//! branch, it never rewrites one.

use super::{GitError, Repo};

impl Repo {
    /// Sends the local branch `branch` to the branch of the same name at
    /// `url`. Git refuses when the branch there has commits this one has not.
    pub fn push_branch(&self, url: &str, branch: &str) -> Result<(), GitError> {
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        self.git_online(&["push", "--quiet", url, &refspec])?;
        Ok(())
    }

    /// Brings the commits of the branch `branch` at `url` into the branch
    /// checked out now, only when that needs no merge commit (a "fast
    /// forward"). Git refuses when the two have gone different ways.
    pub fn fast_forward_from(&self, url: &str, branch: &str) -> Result<(), GitError> {
        // `fetch` + `merge` instead of `pull`: `pull` follows Lisa's
        // `pull.rebase` setting and could rewrite her commits.
        let reference = format!("refs/heads/{branch}");
        self.git_online(&["fetch", "--quiet", url, &reference])?;
        self.git(&["merge", "--quiet", "--ff-only", "FETCH_HEAD"])?;
        Ok(())
    }

    /// The commit the branch `branch` is at in the repository at `url`;
    /// `None` when there is no such branch.
    pub fn remote_branch_head(&self, url: &str, branch: &str) -> Result<Option<String>, GitError> {
        let reference = format!("refs/heads/{branch}");
        let out = self.git_online(&["ls-remote", url, &reference])?;
        // One line per match: `<commit>\t<reference>`. The commit is checked to
        // be a hash, because it is given to git again.
        Ok(out
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find(|(_, name)| *name == reference)
            .map(|(commit, _)| commit.to_string())
            .filter(|commit| is_hash(commit)))
    }

    /// Every branch in the repository at `url`, as `(commit, name)`.
    pub fn remote_branches(&self, url: &str) -> Result<Vec<(String, String)>, GitError> {
        let out = self.git_online(&["ls-remote", "--heads", url])?;
        Ok(out
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter_map(|(commit, name)| {
                let name = name.strip_prefix("refs/heads/")?;
                is_hash(commit).then(|| (commit.to_string(), name.to_string()))
            })
            .collect())
    }

    /// Adds the remote `origin` with `url` when the project has none, so a
    /// plain `git pull` brings merged pull requests in. An `origin` that is
    /// already there is left alone; `true` when one was added.
    pub fn ensure_origin(&self, url: &str) -> Result<bool, GitError> {
        if self.git(&["remote", "get-url", "origin"]).is_ok() {
            return Ok(false);
        }
        self.git(&["remote", "add", "origin", url])?;
        Ok(true)
    }

    /// Does this repository have `commit` (on any branch, or anywhere in
    /// its object store)?
    pub fn has_commit(&self, commit: &str) -> bool {
        self.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .is_ok()
    }

    /// Is `commit` part of the history of the checked-out commit? `false`
    /// also when git does not know the commit at all.
    pub fn has_in_history(&self, commit: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", commit, "HEAD"])
            .is_ok()
    }
}

/// A commit hash from git's output: checked, because it is given to git again.
fn is_hash(commit: &str) -> bool {
    !commit.is_empty() && commit.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::git::Repo;

    /// A project with one commit on `harness/task-001`, and an empty
    /// repository standing in for GitHub.
    fn project_and_remote() -> (tempfile::TempDir, Repo, String) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let repo = Repo::init(&project).unwrap();
        fs::write(project.join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        repo.create_branch("harness/task-001").unwrap();
        let remote = dir.path().join("remote.git");
        let status = std::process::Command::new("git")
            .args(["init", "--quiet", "--bare"])
            .arg(&remote)
            .status()
            .unwrap();
        assert!(status.success());
        let url = remote.to_string_lossy().into_owned();
        (dir, repo, url)
    }

    #[test]
    fn every_branch_at_the_remote_is_listed() {
        let (_dir, repo, url) = project_and_remote();
        assert_eq!(repo.remote_branches(&url).unwrap(), vec![]);

        repo.push_branch(&url, "harness/task-001").unwrap();

        let head = repo.head().unwrap().unwrap();
        assert_eq!(
            repo.remote_branches(&url).unwrap(),
            vec![(head, "harness/task-001".to_string())]
        );
    }

    #[test]
    fn a_commit_is_known_on_any_branch_and_files_are_read_at_head() {
        let (_dir, repo, _url) = project_and_remote();
        let head = repo.head().unwrap().unwrap();

        assert!(repo.has_commit(&head));
        assert!(!repo.has_commit(&"0".repeat(40)));
        assert_eq!(repo.file_at_head("README.md").as_deref(), Some("hello\n"));
        assert_eq!(repo.file_at_head("missing.md"), None);
    }

    #[test]
    fn origin_is_added_only_when_missing() {
        let (_dir, repo, url) = project_and_remote();

        assert!(repo.ensure_origin(&url).unwrap());
        assert!(!repo
            .ensure_origin("https://github.com/other/app.git")
            .unwrap());

        let origin = repo.git(&["remote", "get-url", "origin"]).unwrap();
        assert_eq!(origin.trim(), url);
    }

    #[test]
    fn a_pushed_branch_is_found_at_the_remote() {
        let (_dir, repo, url) = project_and_remote();
        assert_eq!(
            repo.remote_branch_head(&url, "harness/task-001").unwrap(),
            None
        );

        repo.push_branch(&url, "harness/task-001").unwrap();

        let head = repo.head().unwrap();
        assert_eq!(
            repo.remote_branch_head(&url, "harness/task-001").unwrap(),
            head
        );
        assert!(repo.has_in_history(&head.unwrap()));
        // Only the full name counts.
        assert_eq!(repo.remote_branch_head(&url, "harness/task").unwrap(), None);
    }

    #[test]
    fn a_branch_at_the_remote_is_brought_in_only_as_a_fast_forward() {
        let (dir, repo, url) = project_and_remote();
        repo.push_branch(&url, "harness/task-001").unwrap();
        // GitHub gets a newer commit, as when a pull request is merged there.
        fs::create_dir(dir.path().join("other")).unwrap();
        let other = Repo::init(&dir.path().join("other")).unwrap();
        other
            .git(&["fetch", "--quiet", &url, "harness/task-001"])
            .unwrap();
        other
            .git(&["switch", "--quiet", "-c", "work", "FETCH_HEAD"])
            .unwrap();
        fs::write(dir.path().join("other/x.txt"), "x\n").unwrap();
        other.commit_all("merged on GitHub").unwrap();
        other
            .git(&["push", "--quiet", &url, "work:harness/task-001"])
            .unwrap();

        repo.fast_forward_from(&url, "harness/task-001").unwrap();

        assert_eq!(repo.head().unwrap(), other.head().unwrap());
        assert_eq!(
            fs::read_to_string(repo.root().join("x.txt")).unwrap(),
            "x\n"
        );
        // A local commit the remote does not have: no merge commit is made.
        fs::write(repo.root().join("y.txt"), "y\n").unwrap();
        repo.commit_all("only here").unwrap();
        fs::write(dir.path().join("other/z.txt"), "z\n").unwrap();
        other.commit_all("newer on GitHub").unwrap();
        other
            .git(&["push", "--quiet", &url, "work:harness/task-001"])
            .unwrap();
        let mine = repo.head().unwrap();

        repo.fast_forward_from(&url, "harness/task-001")
            .unwrap_err();

        assert_eq!(repo.head().unwrap(), mine);
    }

    #[test]
    fn a_branch_head_is_known_only_for_a_real_branch() {
        let (_dir, repo, _url) = project_and_remote();

        assert_eq!(
            repo.branch_head("harness/task-001").unwrap(),
            repo.head().unwrap()
        );
        assert_eq!(repo.branch_head("harness/task-002").unwrap(), None);
    }

    #[test]
    fn a_push_never_overwrites_other_commits() {
        let (dir, repo, url) = project_and_remote();
        repo.push_branch(&url, "harness/task-001").unwrap();
        // Someone else adds a commit to the branch at the remote.
        fs::create_dir(dir.path().join("other")).unwrap();
        let other = Repo::init(&dir.path().join("other")).unwrap();
        other
            .git(&["fetch", "--quiet", &url, "harness/task-001"])
            .unwrap();
        other
            .git(&["switch", "--quiet", "-c", "work", "FETCH_HEAD"])
            .unwrap();
        fs::write(dir.path().join("other/x.txt"), "x\n").unwrap();
        other.commit_all("not the harness").unwrap();
        other
            .git(&["push", "--quiet", &url, "work:harness/task-001"])
            .unwrap();
        fs::write(repo.root().join("y.txt"), "y\n").unwrap();
        repo.commit_all("the harness").unwrap();

        repo.push_branch(&url, "harness/task-001").unwrap_err();

        let theirs = repo
            .remote_branch_head(&url, "harness/task-001")
            .unwrap()
            .unwrap();
        assert_eq!(theirs, other.head().unwrap().unwrap());
        assert!(!repo.has_in_history(&theirs));
    }
}
