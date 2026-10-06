//! The few git operations the harness needs, done by running the `git` program.
//!
//! Why the `git` program and not a Rust git library? It is the same git Lisa uses
//! in the terminal and in RustRover, so the harness sees exactly what she sees,
//! and there is no extra dependency to learn.

mod history;
mod remote;

pub use remote::{fetch, head_commit};

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

/// What can go wrong when the harness runs git or writes a file in the repository.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The `git` program could not be started, for example it is not installed.
    #[error("cannot run git: {0}")]
    CannotRun(#[source] io::Error),
    /// Git ran but ended with an error.
    #[error("`git {command}` failed: {stderr}")]
    Failed {
        /// The arguments given to git, joined with spaces (without `git` itself).
        command: String,
        /// What git printed to its error output, trimmed; may be empty.
        stderr: String,
    },
    /// The folder is not part of any git repository.
    #[error("{0} is not inside a git repository")]
    NotARepo(PathBuf),
    /// A file or folder could not be read, written or created.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file or folder the harness tried to use.
        path: PathBuf,
        /// The error the operating system gave.
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
        // Lisa's own git in this folder keeps the files as they are too.
        repo.git(&["config", "core.autocrlf", "false"])?;
        Ok(repo)
    }

    /// Is `root` the top folder of its repository (not a folder inside one)?
    pub fn is_top_level(&self) -> Result<bool, GitError> {
        let top = self.git(&["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim());
        Ok(top.canonicalize().ok() == self.root.canonicalize().ok())
    }

    /// The top folder of the repository, as it was given to `open` or `init`.
    pub fn root(&self) -> &Path {
        &self.root
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

    /// The repository's own settings file (`.git/config`) as it is now; empty if missing.
    ///
    /// The harness reads it before and after each role: an agent with a shell
    /// could add a setting there that makes git run a command of its choice
    /// (`core.fsmonitor`, a `filter` driver) the next time anyone runs git.
    pub fn local_config(&self) -> Result<Vec<u8>, GitError> {
        let path = self.local_config_path()?;
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            // A deleted file is a change too; it differs from what was read before.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(source) => Err(GitError::Io { path, source }),
        }
    }

    /// Puts back the settings file that [`Repo::local_config`] read earlier.
    pub fn restore_local_config(&self, bytes: &[u8]) -> Result<(), GitError> {
        let path = self.local_config_path()?;
        fs::write(&path, bytes).map_err(|source| GitError::Io { path, source })
    }

    /// Where `.git/config` is. Git says so itself, because `.git` can also be a
    /// file pointing elsewhere (a worktree or a submodule).
    fn local_config_path(&self) -> Result<PathBuf, GitError> {
        let path = self.git(&["rev-parse", "--git-path", "config"])?;
        // A relative answer is relative to the project; `join` keeps an absolute one as it is.
        Ok(self.root.join(path.trim()))
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

    /// Is `path` in the last commit or staged?
    pub fn is_tracked(&self, path: &Path) -> bool {
        path.to_str().is_some_and(|p| {
            self.git_literal(&["ls-files", "--error-unmatch", "--", p])
                .is_ok()
        })
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

    pub(super) fn git(&self, args: &[&str]) -> Result<String, GitError> {
        self.run(args, false)
    }

    /// Like `git`, but paths are taken literally: a file named `*.rs` means only that file.
    pub(super) fn git_literal(&self, args: &[&str]) -> Result<String, GitError> {
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
            // Files stay byte for byte as the agents wrote them: Git for
            // Windows would otherwise turn `\n` into `\r\n` on checkout.
            .args(["-c", "core.autocrlf=false"])
            // Never run hooks: an agent could have written one to run its own code
            // outside its sandbox when the harness commits.
            .args(["-c", "core.hooksPath=/dev/null"])
            // The same for the file-watcher command `git status` would start.
            .args(["-c", "core.fsmonitor=false"])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_watcher_an_agent_set_up_is_never_started() {
        let (dir, repo) = new_repo();
        let ran = dir.path().join("watcher-ran.txt");
        // The command git would run on `git status`; it would create `ran`.
        let watcher = format!("echo ran > '{}'; false", ran.display());
        repo.git(&["config", "core.fsmonitor", &watcher]).unwrap();
        fs::write(dir.path().join("README.md"), "changed\n").unwrap();

        let changed = repo.changed_files().unwrap();

        assert_eq!(changed, ["README.md"]);
        assert!(!ran.exists());
    }

    #[test]
    fn the_settings_file_is_read_and_put_back() {
        let (_dir, repo) = new_repo();
        let before = repo.local_config().unwrap();
        assert!(String::from_utf8_lossy(&before).contains("[core]"));

        repo.git(&["config", "filter.agent.clean", "sh agent-script.sh"])
            .unwrap();
        assert_ne!(repo.local_config().unwrap(), before);

        repo.restore_local_config(&before).unwrap();
        assert_eq!(repo.local_config().unwrap(), before);
    }

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
    fn the_files_of_the_commit_that_added_a_file() {
        let (_dir, repo) = new_repo();
        assert_eq!(repo.files_of_commit_adding("runs/1/handoff.json"), None);
        write(&repo, "runs/1/handoff.json", "{}");
        write(&repo, "docs/my design.md", "d");
        write(&repo, "README.md", "changed");
        repo.commit_all("step 1").unwrap();
        write(&repo, "docs/my design.md", "later");
        repo.commit_all("step 2").unwrap();
        assert_eq!(
            repo.files_of_commit_adding("runs/1/handoff.json").unwrap(),
            [
                ('M', "README.md".to_string()),
                ('A', "docs/my design.md".to_string()),
                ('A', "runs/1/handoff.json".to_string()),
            ]
        );
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
