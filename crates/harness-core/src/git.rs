//! The few git operations the harness needs, done by running the `git` program.
//!
//! Why the `git` program and not a Rust git library? It is the same git Lisa uses
//! in the terminal and in RustRover, so the harness sees exactly what she sees,
//! and there is no extra dependency to learn.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder inside a project that belongs to the harness.
pub const HARNESS_DIR: &str = ".harness";

/// What `.harness/.gitignore` must contain: scratch space and per-project agent
/// settings (which may hold a login token) never go into git.
const HARNESS_IGNORES: &str = "runs/*/inbox/\nretros/*/inbox/\nagents/\n";

/// Every commit is made by this name, so Lisa can see which commits the harness made.
const AUTHOR_NAME: &str = "AI Harness";
const AUTHOR_EMAIL: &str = "harness@localhost";

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("cannot run git: {0}")]
    CannotRun(#[source] io::Error),
    #[error("`git {command}` failed: {stderr}")]
    Failed { command: String, stderr: String },
    #[error("{0} is not inside a git repository")]
    NotARepo(PathBuf),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A project folder that is a git repository.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Opens the repository whose top folder is `root`.
    pub fn open(root: &Path) -> Result<Self, GitError> {
        let repo = Self {
            root: root.to_path_buf(),
        };
        match repo.git(&["rev-parse", "--show-toplevel"]) {
            Ok(_) => Ok(repo),
            Err(GitError::Failed { .. }) => Err(GitError::NotARepo(root.to_path_buf())),
            Err(e) => Err(e),
        }
    }

    /// Runs `git init` in `root` (used by tests and later by `harness init`).
    pub fn init(root: &Path) -> Result<Self, GitError> {
        let repo = Self {
            root: root.to_path_buf(),
        };
        repo.git(&["init", "-q"])?;
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
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

    /// Where task folders live: `<project>/.harness/runs`.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join(HARNESS_DIR).join("runs")
    }

    /// The current commit id, or `None` in a repository without commits.
    pub fn head(&self) -> Result<Option<String>, GitError> {
        match self.git(&["rev-parse", "-q", "--verify", "HEAD"]) {
            Ok(out) => Ok(Some(out.trim().to_string())),
            Err(GitError::Failed { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes `.harness/.gitignore` if it is missing or different, and commits it.
    pub fn ensure_harness_ignores(&self) -> Result<(), GitError> {
        let dir = self.root.join(HARNESS_DIR);
        let path = dir.join(".gitignore");
        if fs::read_to_string(&path).ok().as_deref() == Some(HARNESS_IGNORES) {
            return Ok(());
        }
        let write = fs::create_dir_all(&dir).and_then(|()| fs::write(&path, HARNESS_IGNORES));
        write.map_err(|source| GitError::Io {
            path: path.clone(),
            source,
        })?;
        self.commit_paths(&[&path], "harness: ignore scratch folders")?;
        Ok(())
    }

    /// Every file that differs from the last commit: changed, added, deleted or
    /// renamed, tracked or not. Paths are relative to the project, with `/`.
    pub fn changed_files(&self) -> Result<Vec<String>, GitError> {
        // `-z` separates entries with a zero byte and never quotes paths,
        // so names with spaces or unusual letters are read correctly.
        let out = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut files = Vec::new();
        let mut entries = out.split('\0').filter(|e| !e.is_empty());
        while let Some(entry) = entries.next() {
            // Each entry is "XY path": two status letters, a space, the path.
            let (status, path) = entry.split_at(3.min(entry.len()));
            files.push(path.to_string());
            // A rename or copy is followed by one more entry: the old path.
            if status.starts_with('R') || status.starts_with('C') {
                if let Some(old) = entries.next() {
                    files.push(old.to_string());
                }
            }
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Commits everything that changed. Returns `false` if there was nothing to commit.
    pub fn commit_all(&self, message: &str) -> Result<bool, GitError> {
        self.git(&["add", "-A"])?;
        self.commit(message, &[])
    }

    /// Commits only the given paths (files or folders), whatever else changed.
    pub fn commit_paths(&self, paths: &[&Path], message: &str) -> Result<bool, GitError> {
        let paths: Vec<&str> = paths.iter().filter_map(|p| p.to_str()).collect();
        let mut add = vec!["add", "-A", "--"];
        add.extend(&paths);
        self.git(&add)?;
        self.commit(message, &paths)
    }

    /// Throws away every change outside `.harness/`, so a failed attempt of a role
    /// leaves nothing behind. Ignored files (like `target/`) are kept.
    pub fn discard_changes(&self) -> Result<(), GitError> {
        const OUTSIDE_HARNESS: &str = ":(exclude).harness";
        if self.head()?.is_none() {
            return Ok(());
        }
        // 1. Unstage everything the agent may have `git add`-ed.
        self.git(&["reset", "-q"])?;
        // 2. Put changed and deleted tracked files back as they are in the last commit.
        let out = self.git(&[
            "diff",
            "--name-only",
            "-z",
            "HEAD",
            "--",
            ".",
            OUTSIDE_HARNESS,
        ])?;
        let tracked: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
        if !tracked.is_empty() {
            let mut checkout = vec!["checkout", "-q", "HEAD", "--"];
            checkout.extend(&tracked);
            self.git_literal(&checkout)?;
        }
        // 3. Delete new files and folders.
        self.git(&["clean", "-fdq", "--", ".", OUTSIDE_HARNESS])?;
        Ok(())
    }

    fn commit(&self, message: &str, paths: &[&str]) -> Result<bool, GitError> {
        let mut check = vec!["diff", "--cached", "--quiet", "--"];
        check.extend(paths);
        match self.git(&check) {
            Ok(_) => return Ok(false), // exit code 0: nothing staged
            Err(GitError::Failed { stderr, .. }) if stderr.is_empty() => {} // exit code 1: changes
            Err(e) => return Err(e),
        }
        let mut commit = vec!["commit", "-q", "--no-verify", "-m", message, "--"];
        commit.extend(paths);
        self.git(&commit)?;
        Ok(true)
    }

    fn git(&self, args: &[&str]) -> Result<String, GitError> {
        self.run(args, false)
    }

    /// Like `git`, but paths are taken literally: a file named `*.rs` means only that file.
    fn git_literal(&self, args: &[&str]) -> Result<String, GitError> {
        self.run(args, true)
    }

    fn run(&self, args: &[&str], literal_paths: bool) -> Result<String, GitError> {
        let mut command = Command::new("git");
        command
            .current_dir(&self.root)
            .args(["-c", &format!("user.name={AUTHOR_NAME}")])
            .args(["-c", &format!("user.email={AUTHOR_EMAIL}")])
            // Do not ask for a signing key or passphrase in the middle of a run.
            .args(["-c", "commit.gpgsign=false"])
            // Never run hooks: an agent could have written one to run its own code
            // outside its sandbox when the harness commits.
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args);
        if literal_paths {
            command.env("GIT_LITERAL_PATHSPECS", "1");
        }
        let output = command.output().map_err(GitError::CannotRun)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(GitError::Failed {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }
}

/// Makes `into` hold `revision` (a branch, tag or commit; the default branch
/// if `None`) of the repository at `url`, and returns the commit. Used for
/// plugin catalogs and plugins: only the newest state is downloaded, and a
/// second call updates the same folder.
pub fn fetch(url: &str, revision: Option<&str>, into: &Path) -> Result<String, GitError> {
    fs::create_dir_all(into).map_err(|source| GitError::Io {
        path: into.to_path_buf(),
        source,
    })?;
    if !into.join(".git").exists() {
        outside_git(into, &["init", "--quiet"])?;
        outside_git(into, &["remote", "add", "origin", url])?;
    } else {
        outside_git(into, &["remote", "set-url", "origin", url])?;
    }
    let revision = revision.unwrap_or("HEAD");
    outside_git(
        into,
        &["fetch", "--quiet", "--depth", "1", "origin", revision],
    )?;
    outside_git(into, &["checkout", "--quiet", "--force", "FETCH_HEAD"])?;
    outside_git(into, &["clean", "--quiet", "-dxff"])?;
    head_commit(into).ok_or_else(|| GitError::NotARepo(into.to_path_buf()))
}

/// The commit a folder's repository is at, if it is one.
pub fn head_commit(dir: &Path) -> Option<String> {
    outside_git(dir, &["rev-parse", "HEAD"])
        .ok()
        .map(|out| out.trim().to_string())
}

/// Git for repositories other than the project: no password prompts (a wrong
/// address fails instead of waiting), no hooks, no user settings needed.
fn outside_git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "advice.detachedHead=false"])
        .args(args)
        .output()
        .map_err(GitError::CannotRun)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(GitError::Failed {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_as_it_was_at_another_files_last_change() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        assert_eq!(repo.file_at_last_change("a.txt", "b.txt"), None);
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join("b.txt"), "old").unwrap();
        repo.commit_all("first").unwrap();
        fs::write(dir.path().join("b.txt"), "new").unwrap();
        repo.commit_all("second").unwrap();
        assert_eq!(
            repo.file_at_last_change("a.txt", "b.txt").as_deref(),
            Some("old")
        );
        assert_eq!(repo.file_at_last_change("a.txt", "missing.txt"), None);
        assert_eq!(repo.file_at_last_change("nothing", "b.txt"), None);
    }

    #[test]
    fn fetch_copies_and_updates_another_repository() {
        let (source_dir, source) = new_repo();
        let url = format!("file://{}", source_dir.path().display());
        let copy = tempfile::tempdir().unwrap();
        let into = copy.path().join("catalog");

        let first = fetch(&url, None, &into).unwrap();
        assert_eq!(Some(first.clone()), source.head().unwrap());
        assert_eq!(
            fs::read_to_string(into.join("README.md")).unwrap(),
            "hello\n"
        );

        fs::write(source_dir.path().join("README.md"), "changed\n").unwrap();
        source.commit_all("second").unwrap();
        fs::write(into.join("stray.txt"), "left over").unwrap();
        let second = fetch(&url, None, &into).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(into.join("README.md")).unwrap(),
            "changed\n"
        );
        assert!(!into.join("stray.txt").exists());
        assert_eq!(head_commit(&into), Some(second));

        let missing = fetch("file:///no/such/repo", None, &copy.path().join("x"));
        assert!(missing.is_err());
    }

    fn new_repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    fn write(repo: &Repo, path: &str, text: &str) {
        let path = repo.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn open_refuses_a_folder_without_git() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(Repo::open(dir.path()), Err(GitError::NotARepo(_))));
    }

    #[test]
    fn changed_files_lists_new_changed_and_deleted_files() {
        let (_dir, repo) = new_repo();
        write(&repo, "src/new file.rs", "x");
        write(&repo, "README.md", "changed");
        assert_eq!(
            repo.changed_files().unwrap(),
            ["README.md", "src/new file.rs"]
        );

        repo.commit_all("more").unwrap();
        fs::remove_file(repo.root().join("README.md")).unwrap();
        assert_eq!(repo.changed_files().unwrap(), ["README.md"]);
    }

    #[test]
    fn a_rename_reports_both_names() {
        let (_dir, repo) = new_repo();
        repo.git(&["mv", "README.md", "docs.md"]).unwrap();
        assert_eq!(repo.changed_files().unwrap(), ["README.md", "docs.md"]);
    }

    #[test]
    fn commit_all_commits_and_says_when_there_was_nothing() {
        let (_dir, repo) = new_repo();
        let before = repo.head().unwrap();
        assert!(!repo.commit_all("nothing").unwrap());
        assert_eq!(repo.head().unwrap(), before);

        write(&repo, "a.txt", "a");
        assert!(repo.commit_all("add a").unwrap());
        assert_ne!(repo.head().unwrap(), before);
        assert!(repo.changed_files().unwrap().is_empty());
    }

    #[test]
    fn commit_paths_leaves_other_changes_alone() {
        let (_dir, repo) = new_repo();
        write(&repo, ".harness/runs/t/state.json", "{}");
        write(&repo, "src/main.rs", "fn main() {}");
        let harness = repo.root().join(HARNESS_DIR);
        assert!(repo.commit_paths(&[&harness], "state").unwrap());
        assert_eq!(repo.changed_files().unwrap(), ["src/main.rs"]);
    }

    #[test]
    fn discard_changes_keeps_harness_files_and_ignored_files() {
        let (_dir, repo) = new_repo();
        write(&repo, ".gitignore", "target/\n");
        repo.commit_all("ignore").unwrap();

        write(&repo, "README.md", "broken");
        write(&repo, "src/new.rs", "x");
        write(&repo, ".harness/note.md", "keep me");
        write(&repo, "target/build.bin", "keep me too");
        repo.git(&["add", "src/new.rs"]).unwrap();

        repo.discard_changes().unwrap();

        assert_eq!(repo.changed_files().unwrap(), [".harness/note.md"]);
        let readme = fs::read_to_string(repo.root().join("README.md")).unwrap();
        assert_eq!(readme, "hello\n");
        assert!(repo.root().join("target/build.bin").exists());
    }

    #[test]
    fn harness_ignores_hide_the_inbox_and_agent_settings() {
        let (_dir, repo) = new_repo();
        repo.ensure_harness_ignores().unwrap();
        assert!(repo.changed_files().unwrap().is_empty());

        write(&repo, ".harness/runs/task-001/inbox/handoff.json", "{}");
        write(&repo, ".harness/agents/claude/settings.json", "{}");
        assert!(repo.changed_files().unwrap().is_empty());

        // Running it again changes nothing and makes no new commit.
        let head = repo.head().unwrap();
        repo.ensure_harness_ignores().unwrap();
        assert_eq!(repo.head().unwrap(), head);
    }
}
