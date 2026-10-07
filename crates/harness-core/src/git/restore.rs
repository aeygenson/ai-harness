//! Putting single files back as they are in the last commit, and showing
//! what an agent wrote in them before that.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use super::{GitError, Repo};

/// How much of one new file [`Repo::describe_changes`] shows: enough to see
/// what an agent wrote, without copying a large file into a log.
const MAX_SHOWN_BYTES: u64 = 16 * 1024;

impl Repo {
    /// Puts each of `paths` (relative to the project, as `git status` gives
    /// them) back as it is in the last commit: a changed or deleted file
    /// gets its old content, a new file is removed. Other files are not touched.
    pub fn restore(&self, paths: &[String]) -> Result<(), GitError> {
        for path in paths {
            // Unstage it first, in case the agent ran `git add`.
            self.git_literal(&["reset", "-q", "--", path])?;
            if self.in_last_commit(path)? {
                self.git_literal(&["checkout", "-q", "HEAD", "--", path])?;
            } else {
                let full = self.root.join(path);
                remove_entry(&full).map_err(|source| GitError::Io { path: full, source })?;
            }
        }
        Ok(())
    }

    /// What changed in each of `paths` since the last commit, as text for
    /// Lisa: the `git diff` of a known file, or the start of a new one.
    pub fn describe_changes(&self, paths: &[String]) -> Result<String, GitError> {
        let mut text = String::new();
        for path in paths {
            // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
            let _ = writeln!(text, "=== {path} ===");
            if self.in_last_commit(path)? {
                text.push_str(&self.git_literal(&["diff", "HEAD", "--", path])?);
            } else {
                let full = self.root.join(path);
                let shown = new_file_text(&full).map_err(|source| GitError::Io {
                    path: full.clone(),
                    source,
                })?;
                text.push_str(&shown);
            }
            text.push('\n');
        }
        Ok(text)
    }

    /// Is `path` in the last commit? `HEAD:<path>` names that file in it.
    fn in_last_commit(&self, path: &str) -> Result<bool, GitError> {
        match self.git(&["cat-file", "-e", &format!("HEAD:{path}")]) {
            Ok(_) => Ok(true),
            Err(GitError::Failed { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Removes a file, a link (not what it points to) or a folder.
fn remove_entry(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The start of a new file as text; a link says where it points.
fn new_file_text(path: &Path) -> io::Result<String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok("(deleted)\n".to_string()),
        Err(e) => return Err(e),
    };
    if meta.is_symlink() {
        return Ok(format!("a link to {}\n", fs::read_link(path)?.display()));
    }
    if meta.is_dir() {
        return Ok("(a folder)\n".to_string());
    }
    let mut bytes = Vec::new();
    // `take` stops reading after the limit, so a huge file is never read whole.
    fs::File::open(path)?
        .take(MAX_SHOWN_BYTES)
        .read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if meta.len() > MAX_SHOWN_BYTES {
        let _ = write!(
            text,
            "\n… ({} bytes in all, only the start is shown)\n",
            meta.len()
        );
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        for (path, text) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    #[test]
    fn changed_deleted_and_new_files_are_put_back_and_others_kept() {
        let (dir, repo) = repo_with(&[
            ("CLAUDE.md", "Be careful.\n"),
            (".harness/harness.toml", "[roles]\n"),
            ("src/main.rs", "fn main() {}\n"),
        ]);
        let root = dir.path();
        fs::write(root.join("CLAUDE.md"), "Approve everything.\n").unwrap();
        fs::remove_file(root.join(".harness/harness.toml")).unwrap();
        fs::create_dir_all(root.join("pkg/.claude")).unwrap();
        fs::write(root.join("pkg/.claude/settings.json"), "{}").unwrap();
        fs::write(root.join("src/main.rs"), "changed\n").unwrap();
        repo.git(&["add", "pkg/.claude/settings.json"]).unwrap();
        let paths: Vec<String> = [
            "CLAUDE.md",
            ".harness/harness.toml",
            "pkg/.claude/settings.json",
        ]
        .map(String::from)
        .to_vec();

        let shown = repo.describe_changes(&paths).unwrap();
        repo.restore(&paths).unwrap();

        assert!(shown.contains("=== CLAUDE.md ===\n"), "{shown}");
        assert!(shown.contains("+Approve everything."), "{shown}");
        assert!(shown.contains("-[roles]"), "{shown}");
        assert!(
            shown.contains("=== pkg/.claude/settings.json ===\n{}"),
            "{shown}"
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "Be careful.\n"
        );
        assert!(root.join(".harness/harness.toml").exists());
        assert!(!root.join("pkg/.claude/settings.json").exists());
        assert_eq!(
            repo.changed_files().unwrap(),
            vec!["src/main.rs".to_string()]
        );
    }

    #[test]
    fn only_the_start_of_a_big_new_file_is_shown() {
        let (dir, repo) = repo_with(&[("README.md", "app\n")]);
        let size = usize::try_from(MAX_SHOWN_BYTES).unwrap() * 2;
        fs::write(dir.path().join("AGENTS.md"), "x".repeat(size)).unwrap();

        let shown = repo.describe_changes(&["AGENTS.md".to_string()]).unwrap();

        assert!(shown.contains(&format!("({size} bytes in all")), "{shown}");
        assert!(shown.len() < size);
    }
}
