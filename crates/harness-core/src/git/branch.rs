//! Branches: which one is checked out, making a new one, moving to another.

use super::{GitError, Repo};

impl Repo {
    /// The name of the branch checked out now; `None` when no branch is
    /// (a "detached HEAD", for example after checking out a commit).
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        // `symbolic-ref` ends with an error and no message when HEAD is not
        // a branch. The full name is read: `--short` turns it into
        // `heads/<name>` when a tag has the same name. (`git branch
        // --show-current` would be simpler but needs git 2.22.)
        match self.git(&["symbolic-ref", "-q", "HEAD"]) {
            Ok(out) => Ok(out.trim().strip_prefix("refs/heads/").map(str::to_string)),
            Err(GitError::Failed { stderr, .. }) if stderr.is_empty() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Does a local branch with this name exist?
    pub fn has_branch(&self, name: &str) -> Result<bool, GitError> {
        let reference = format!("refs/heads/{name}");
        // `for-each-ref` also lists the branches below `<name>/`, so the
        // full name is compared.
        let out = self.git(&["for-each-ref", "--format=%(refname)", &reference])?;
        Ok(out.lines().any(|line| line == reference))
    }

    /// The local branches whose names start with `prefix`, sorted.
    pub fn branches_starting_with(&self, prefix: &str) -> Result<Vec<String>, GitError> {
        let pattern = format!("refs/heads/{prefix}*");
        // `lstrip=2` drops `refs/heads/`; `short` would add `heads/` when a
        // tag has the same name.
        let out = self.git(&["for-each-ref", "--format=%(refname:lstrip=2)", &pattern])?;
        let mut names: Vec<String> = out.lines().map(str::to_string).collect();
        names.sort();
        Ok(names)
    }

    /// Every local branch with its commit, one per line. The run loop
    /// compares this before and after an agent, which must not make, move
    /// or delete a branch.
    pub fn branch_heads(&self) -> Result<String, GitError> {
        self.git(&[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads/",
        ])
    }

    /// Makes the branch `name` at the current commit and checks it out. The
    /// files do not change, so uncommitted changes stay as they are.
    pub fn create_branch(&self, name: &str) -> Result<(), GitError> {
        self.git(&["switch", "-q", "-c", name])?;
        Ok(())
    }

    /// Checks out the existing branch `name`. Git refuses when an uncommitted
    /// change would be lost; callers check for changes first.
    pub fn switch_branch(&self, name: &str) -> Result<(), GitError> {
        // By default git overwrites an ignored file (such as a `.env` with
        // Lisa's own values) when the other branch has a file of that name.
        self.git(&["switch", "-q", "--no-overwrite-ignore", name])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::git::Repo;

    fn repo_with_a_commit() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    #[test]
    fn a_new_branch_is_checked_out_and_listed() {
        let (_dir, repo) = repo_with_a_commit();
        let first = repo.current_branch().unwrap().unwrap();
        assert!(!repo.has_branch("harness/task-001").unwrap());

        repo.create_branch("harness/task-001").unwrap();

        assert_eq!(
            repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
        assert!(repo.has_branch("harness/task-001").unwrap());
        assert_eq!(
            repo.branches_starting_with("harness/").unwrap(),
            ["harness/task-001"]
        );
        repo.switch_branch(&first).unwrap();
        assert_eq!(repo.current_branch().unwrap(), Some(first));
    }

    #[test]
    fn switching_never_overwrites_an_ignored_file() {
        let (dir, repo) = repo_with_a_commit();
        let first = repo.current_branch().unwrap().unwrap();
        repo.create_branch("other").unwrap();
        fs::write(dir.path().join(".env"), "SAMPLE=1\n").unwrap();
        repo.commit_all("a sample .env").unwrap();
        repo.switch_branch(&first).unwrap();
        fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        repo.commit_all("ignore .env").unwrap();
        fs::write(dir.path().join(".env"), "MINE=1\n").unwrap();

        repo.switch_branch("other").unwrap_err();

        let kept = fs::read_to_string(dir.path().join(".env")).unwrap();
        assert_eq!(kept, "MINE=1\n");
    }

    #[test]
    fn a_branch_is_found_only_by_its_full_name() {
        let (_dir, repo) = repo_with_a_commit();
        repo.create_branch("harness/task-001x").unwrap();

        assert!(!repo.has_branch("harness/task-001").unwrap());
        assert!(!repo.has_branch("harness").unwrap());
        assert!(repo.has_branch("harness/task-001x").unwrap());
    }

    #[test]
    fn a_tag_with_the_same_name_does_not_hide_the_branch() {
        let (_dir, repo) = repo_with_a_commit();
        repo.git(&["tag", "harness/task-001"]).unwrap();
        repo.create_branch("harness/task-001").unwrap();

        assert_eq!(
            repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
        assert_eq!(
            repo.branches_starting_with("harness/").unwrap(),
            ["harness/task-001"]
        );
    }

    #[test]
    fn a_detached_head_is_no_branch() {
        let (_dir, repo) = repo_with_a_commit();
        let commit = repo.head().unwrap().unwrap();
        repo.git(&["switch", "-q", "--detach", &commit]).unwrap();

        assert_eq!(repo.current_branch().unwrap(), None);
    }
}
