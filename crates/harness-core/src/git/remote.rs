//! Git for other repositories than the project: downloading plugin catalogs
//! and plugins.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::GitError;

/// Makes `into` hold `revision` (a branch, tag or commit; the default branch
/// if `None`) of the repository at `url`, and returns the commit. Used for
/// plugin catalogs and plugins: only the newest state is downloaded, and a
/// second call updates the same folder.
pub fn fetch(url: &str, revision: Option<&str>, into: &Path) -> Result<String, GitError> {
    fs::create_dir_all(into).map_err(|source| GitError::Io {
        path: into.to_path_buf(),
        source,
    })?;
    if into.join(".git").exists() {
        outside_git(into, &["remote", "set-url", "origin", url])?;
    } else {
        outside_git(into, &["init", "--quiet"])?;
        outside_git(into, &["remote", "add", "origin", url])?;
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
pub(super) fn outside_git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["-c", "core.fsmonitor=false"])
        .args(["-c", "advice.detachedHead=false"])
        .args(["-c", "core.autocrlf=false"])
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
