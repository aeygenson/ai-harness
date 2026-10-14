//! What must hold before a done task's pull request is merged: it is this
//! task's, GitHub's copy is the commit the harness sent, and its checks passed.

use harness_core::git::Repo;
use harness_core::text;

use super::pull::{Check, View};
use super::GitHub;

/// How many characters of a name or commit from GitHub go into a message.
pub(super) const SHOWN_CHARS: usize = 80;

/// What the checks of a pull request say together.
#[derive(Debug, PartialEq, Eq)]
enum Checks {
    /// All passed, or there are none.
    Passed,
    /// None failed, but these are still running.
    Running(Vec<String>),
    /// These failed.
    Failed(Vec<String>),
}

/// Refuses a pull request that is not this task's: from another branch or
/// a fork, or into another branch than `base`.
pub(super) fn check_identity(
    seen: &View,
    number: u64,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    if !is_commit(&seen.head_ref_oid) {
        return Err(format!(
            "GitHub gave no commit for pull request #{number}; nothing was merged"
        ));
    }
    if seen.head_ref_name != branch || seen.is_cross_repository {
        return Err(format!(
            "pull request #{number} does not come from {branch} of this repository; nothing \
             was merged. Look at it on GitHub"
        ));
    }
    if seen.base_ref_name != base {
        return Err(format!(
            "pull request #{number} goes into {}, not into {base} as harness.toml says; nothing \
             was merged. Look at it on GitHub",
            text::safe_line(&seen.base_ref_name, SHOWN_CHARS)
        ));
    }
    Ok(())
}

/// Refuses to merge an open pull request that is a draft, is not at the
/// commit of the task's branch here, may have conflicts or has checks that
/// did not pass; also while the folder has uncommitted changes, which would
/// not be in the merge.
pub(super) fn check_ready(
    repo: &Repo,
    github: &GitHub,
    branch: &str,
    number: u64,
    seen: &View,
) -> Result<(), String> {
    if seen.is_draft {
        return Err(format!(
            "pull request #{number} is a draft; mark it ready on GitHub, then merge"
        ));
    }
    let files = repo.changed_files().map_err(|e| e.to_string())?;
    if !files.is_empty() {
        return Err(format!(
            "these files have uncommitted changes, which would not be in the merge: {}. \
             Nothing was merged; commit them or put them aside (git stash) first",
            files.join(", ")
        ));
    }
    let local = repo
        .branch_head(branch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("the branch {branch} is not in this project; nothing was merged"))?;
    let remote = &seen.head_ref_oid;
    if *remote != local {
        // Commits the harness made after sending (state.json, a retro):
        // they are sent now, and their checks have to run before merging.
        if repo.is_ancestor(remote, &local) {
            repo.push_branch(&github.push_url, branch)
                .map_err(|e| format!("cannot send the last commits of {branch} to GitHub: {e}"))?;
            return Err(format!(
                "sent the last commits of {branch} to GitHub; nothing was merged yet. Merge \
                 again when its checks have passed"
            ));
        }
        return Err(format!(
            "pull request #{number} is at commit {remote}, which the harness did not make (an \
             agent or someone else may have pushed it); nothing was merged. Look at it on GitHub"
        ));
    }
    match seen.mergeable.as_str() {
        "MERGEABLE" => {}
        "CONFLICTING" => {
            return Err(format!(
                "pull request #{number} has conflicts with the main branch; nothing was \
                 merged. Resolve them on GitHub, then merge again"
            ))
        }
        // `UNKNOWN`: GitHub has not worked it out yet, which takes a moment.
        _ => {
            return Err(format!(
                "GitHub is still checking pull request #{number} for conflicts; merge again \
                 in a moment"
            ))
        }
    }
    let rollup = seen.status_check_rollup.as_deref().unwrap_or_default();
    match checks(rollup) {
        // Right after sending, GitHub may not have started the CI yet: no
        // checks at all is "passed" only in a project without workflows.
        Checks::Passed if rollup.is_empty() && base_has_workflows(repo, github)? => Err(format!(
            "GitHub shows no checks for pull request #{number} yet, but the project has \
             workflows; merge again when they have run. If they do not run on pull requests, \
             merge it on GitHub; then «Merge pull request» only updates the folder"
        )),
        Checks::Passed => Ok(()),
        Checks::Running(names) => Err(format!(
            "the checks of pull request #{number} are still running ({}); merge again when \
             they finish",
            names.join(", ")
        )),
        Checks::Failed(names) => Err(format!(
            "checks failed on pull request #{number} ({}); nothing was merged. Look at them \
             on GitHub",
            names.join(", ")
        )),
    }
}

/// Does `base` on GitHub have GitHub Actions workflows? GitHub's copy, not
/// a local branch or the folder: an agent could have changed those.
fn base_has_workflows(repo: &Repo, github: &GitHub) -> Result<bool, String> {
    let commit = repo
        .fetch_branch(&github.push_url, &github.base)
        .map_err(|e| format!("cannot look at {} on GitHub: {e}", github.base))?;
    Ok(repo.has_path_at(&commit, ".github/workflows"))
}

/// Is `commit` a full commit hash? It comes from GitHub and is given to git.
pub(super) fn is_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())
}

/// Sorts the checks into passed, still running and failed. A check that
/// was skipped or only reports something (`NEUTRAL`) counts as passed.
fn checks(all: &[Check]) -> Checks {
    let mut running = Vec::new();
    let mut failed = Vec::new();
    for check in all {
        let name = check
            .name
            .as_deref()
            .or(check.context.as_deref())
            .unwrap_or("?");
        // The names come from GitHub (anyone with access can name a job).
        let name = text::safe_line(name, SHOWN_CHARS);
        let conclusion = check.conclusion.as_deref();
        let state = check.state.as_deref();
        let passed = matches!(conclusion, Some("SUCCESS" | "NEUTRAL" | "SKIPPED"))
            || state == Some("SUCCESS");
        let still_running = check.status.as_deref().is_some_and(|s| s != "COMPLETED")
            || matches!(state, Some("PENDING" | "EXPECTED"));
        if passed {
            continue;
        }
        if still_running {
            running.push(name);
        } else {
            failed.push(name);
        }
    }
    if !failed.is_empty() {
        Checks::Failed(failed)
    } else if !running.is_empty() {
        Checks::Running(running)
    } else {
        Checks::Passed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_names_from_github_are_cleaned() {
        let check = Check {
            name: Some("evil\u{1b}[2Jjob".into()),
            context: None,
            status: Some("COMPLETED".into()),
            conclusion: Some("FAILURE".into()),
            state: None,
        };

        assert_eq!(checks(&[check]), Checks::Failed(vec!["eviljob".into()]));
        assert_eq!(checks(&[]), Checks::Passed);
    }
}
