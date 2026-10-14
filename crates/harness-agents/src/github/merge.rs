//! Merging a done task's pull request on GitHub, then bringing the merged
//! work into the project's main branch (`base`), so the next task starts
//! from it.
//!
//! Lisa decides to merge; the harness only does the clicks. Before merging
//! it checks what GitHub says: the pull request must still be at the commit
//! the harness sent (no commits someone else pushed), without conflicts and
//! with its checks (CI) passed.

use std::path::Path;

use harness_core::config::Config;
use harness_core::git::Repo;
use harness_core::task::branch::task_branch;
use harness_core::task::{Stage, TaskState};
use harness_core::text;

use super::pull::{self as pr, Check, View};
use super::GitHub;

/// How many characters of a name or commit from GitHub go into a message.
const SHOWN_CHARS: usize = 80;

/// What merging did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    /// The pull request's number.
    pub number: u64,
    /// The branch it was merged into.
    pub base: String,
    /// Whether the project folder now holds the merged work.
    pub folder: Folder,
}

/// What happened to the project folder after the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folder {
    /// `base` is checked out and has the merged work.
    Updated,
    /// Nothing was changed: these files have uncommitted changes, and
    /// switching branches could lose them.
    Uncommitted(Vec<String>),
    /// Nothing was changed, because bringing in the merged work could not
    /// be done safely: why.
    Failed(String),
}

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

/// Merges the pull request of the done task in `state` and updates the
/// project folder. Needs `[github]` in `config` and the GitHub CLI, signed in.
pub fn merge(repo: &Repo, config: &Config, state: &TaskState) -> Result<Merged, String> {
    let settings = config
        .github
        .as_ref()
        .ok_or_else(|| "this project is not on GitHub, so there is nothing to merge".to_string())?;
    let github = GitHub::ready(settings, "merge again")?;
    merge_with(repo, state, &github)
}

/// [`merge`] with everything given: tests use a fake `gh` and a local
/// folder as GitHub.
pub fn merge_with(repo: &Repo, state: &TaskState, github: &GitHub) -> Result<Merged, String> {
    let task = &state.task_id;
    if state.stage != Stage::Done {
        return Err(format!(
            "{task} is not done yet, so there is nothing to merge"
        ));
    }
    let pull_request = state
        .pull_request
        .as_ref()
        .ok_or_else(|| format!("{task} has no pull request yet; send it to GitHub first"))?;
    let number = pull_request.number;
    // The number is used with `github.repo`, so the address must name that
    // repository too (state.json may have come from anywhere).
    if pull_request.page() != Some(&format!("https://github.com/{}/pull/{number}", github.repo)) {
        return Err(format!(
            "state.json of {task} has a pull request address that is not in {}",
            github.repo
        ));
    }
    let seen = pr::view(&github.gh, &github.repo, number)?;
    if !is_commit(&seen.head_ref_oid) {
        return Err(format!(
            "GitHub gave no commit for pull request #{number}; nothing was merged"
        ));
    }
    if seen.base_ref_name != github.base {
        return Err(format!(
            "pull request #{number} goes into {}, not into {} as harness.toml says; nothing \
             was merged. Look at it on GitHub",
            text::safe_line(&seen.base_ref_name, SHOWN_CHARS),
            github.base
        ));
    }
    match seen.state.as_str() {
        // Merged earlier (on GitHub, or by a try that stopped before the
        // folder was updated): only the folder is left to do.
        "MERGED" => {}
        "OPEN" => {
            check_ready(repo, &task_branch(task), number, &seen)?;
            pr::merge(&github.gh, &github.repo, number, &seen.head_ref_oid).map_err(|e| {
                format!(
                    "GitHub did not confirm the merge of pull request #{number} ({e}); choose \
                     «Merge pull request» again to see where it stands"
                )
            })?;
        }
        "CLOSED" => {
            return Err(format!(
                "pull request #{number} was closed on GitHub without merging; open it again \
                 there if it should be merged"
            ))
        }
        other => {
            return Err(format!(
                "GitHub says pull request #{number} is {}; nothing was merged",
                text::safe_line(other, SHOWN_CHARS)
            ))
        }
    }
    Ok(Merged {
        number,
        base: github.base.clone(),
        folder: update_base(repo, github, &seen.head_ref_oid),
    })
}

/// Refuses to merge an open pull request that is not at the commit of the
/// task's branch here, may have conflicts or has checks that did not pass.
fn check_ready(repo: &Repo, branch: &str, number: u64, seen: &View) -> Result<(), String> {
    let local = repo
        .branch_head(branch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("the branch {branch} is not in this project; nothing was merged"))?;
    let remote = &seen.head_ref_oid;
    if *remote != local {
        if repo.is_ancestor(remote, &local) {
            return Err(format!(
                "the last commits of {branch} are not on GitHub yet; nothing was merged. \
                 Choose «Send to GitHub», then merge"
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
    let checks = checks(&seen.status_check_rollup);
    // Right after sending, GitHub may not have started the CI yet: no
    // checks at all is "passed" only in a project without workflows.
    if checks == Checks::Passed && seen.status_check_rollup.is_empty() && has_workflows(repo.root())
    {
        return Err(format!(
            "GitHub shows no checks for pull request #{number} yet, but the project has \
             workflows; merge again when they have run"
        ));
    }
    match checks {
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

/// Does the project have GitHub Actions workflows (`.github/workflows/`)?
fn has_workflows(root: &Path) -> bool {
    std::fs::read_dir(root.join(".github").join("workflows"))
        .is_ok_and(|mut entries| entries.next().is_some())
}

/// Is `commit` a full commit hash? It comes from GitHub and is given to git.
fn is_commit(commit: &str) -> bool {
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

/// Checks out `base` and brings the merged work (up to `merged`, the pull
/// request's last commit) in from GitHub. Everything is checked before the
/// folder changes: no uncommitted change, GitHub's `base` holds the work, and
/// the local `base` has nothing GitHub lacks. A step that fails after the
/// switch switches back, so a failure leaves the folder as it was.
fn update_base(repo: &Repo, github: &GitHub, merged: &str) -> Folder {
    let message = |e: &dyn std::fmt::Display| e.to_string();
    let base = &github.base;
    match repo.changed_files() {
        Ok(files) if files.is_empty() => {}
        Ok(files) => return Folder::Uncommitted(files),
        Err(e) => return Folder::Failed(message(&e)),
    }
    let fetched = match repo.fetch_branch(&github.push_url, base) {
        Ok(commit) => commit,
        Err(e) => return Folder::Failed(message(&e)),
    };
    if !repo.is_ancestor(merged, &fetched) {
        return Folder::Failed(format!(
            "{base} on GitHub does not hold the merged commit {merged} yet"
        ));
    }
    let local = match repo.branch_head(base) {
        Ok(Some(commit)) => commit,
        Ok(None) => return Folder::Failed(format!("there is no branch {base} in this project")),
        Err(e) => return Folder::Failed(message(&e)),
    };
    if !repo.is_ancestor(&local, &fetched) {
        return Folder::Failed(format!(
            "{base} here has commits that are not on GitHub; bring the two together yourself"
        ));
    }
    let before = match repo.current_branch() {
        Ok(branch) => branch,
        Err(e) => return Folder::Failed(message(&e)),
    };
    if before.as_deref() != Some(base.as_str()) {
        if let Err(e) = repo.switch_branch(base) {
            return Folder::Failed(message(&e));
        }
    }
    if let Err(e) = repo.fast_forward_to(&fetched) {
        if let Some(branch) = before.filter(|b| b != base) {
            // Back to where Lisa was; if even that fails, git's first error
            // is the one that explains what happened.
            let _ = repo.switch_branch(&branch);
        }
        return Folder::Failed(message(&e));
    }
    Folder::Updated
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use harness_core::task::handoff::Role;
    use harness_core::task::pull_request::PullRequest;
    use harness_core::task::scenario::Workflow;

    use super::super::publish::bare_repo;
    use super::*;

    /// A project with a done task-001 whose pull request is #7, both
    /// branches sent to a folder standing in for GitHub, and the task's
    /// branch already merged into `base` there (as GitHub would do when the
    /// fake `gh` is asked to merge).
    struct Setup {
        dir: tempfile::TempDir,
        repo: Repo,
        state: TaskState,
        github: GitHub,
        /// The branch the project started on.
        base: String,
        /// The commit of the task's branch.
        head: String,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let repo = Repo::init(&project).unwrap();
        fs::write(project.join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        let base = repo.current_branch().unwrap().unwrap();
        repo.create_branch("harness/task-001").unwrap();
        fs::write(project.join("parser.txt"), "parser\n").unwrap();
        repo.commit_all("task-001: the work").unwrap();
        let head = repo.head().unwrap().unwrap();
        let push_url = bare_repo(&dir.path().join("github.git"));
        repo.push_branch(&push_url, "harness/task-001").unwrap();
        let merged = format!("harness/task-001:refs/heads/{base}");
        git(&project, &["push", "-q", &push_url, &merged]);
        let mut state = TaskState::new("task-001", Workflow::default(), 5);
        state.stage = Stage::Done;
        state.pull_request = Some(PullRequest {
            number: 7,
            url: "https://github.com/lisa/app/pull/7".into(),
        });
        let github = GitHub {
            repo: "lisa/app".into(),
            base: base.clone(),
            gh: dir.path().join("no-gh"),
            push_url,
        };
        Setup {
            dir,
            repo,
            state,
            github,
            base,
            head,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    }

    /// A fake `gh` whose `pr view` prints `view` and whose calls are saved
    /// in `args.txt`; returns that file.
    fn fake_gh(s: &mut Setup, view: &str) -> std::path::PathBuf {
        let view = view.replace("BASE", &s.base).replace("HEAD", &s.head);
        let bin = s.dir.path().join("bin");
        let args = bin.join("args.txt");
        let script = format!(
            "append-args {}\nwhen-arg 2 view print {view}\n",
            args.display()
        );
        s.github.gh = harness_fake::install(&bin, "gh", &script);
        args
    }

    /// What `gh pr view` prints for an open pull request into `BASE` (the
    /// tests put the real name in).
    fn open_view(head: &str, mergeable: &str, checks: &str) -> String {
        format!(
            r#"{{"state":"OPEN","baseRefName":"BASE","headRefOid":"{head}","mergeable":"{mergeable}","statusCheckRollup":[{checks}]}}"#
        )
    }

    const GREEN: &str = r#"{"name":"server","status":"COMPLETED","conclusion":"SUCCESS"},{"context":"lint","state":"SUCCESS"},{"name":"docs","status":"COMPLETED","conclusion":"SKIPPED"}"#;

    #[test]
    fn a_green_pull_request_is_merged_and_the_folder_updated() {
        let mut s = setup();
        let view = open_view(&s.head, "MERGEABLE", GREEN);
        let args = fake_gh(&mut s, &view);

        let merged = merge_with(&s.repo, &s.state, &s.github).unwrap();

        assert_eq!(
            merged,
            Merged {
                number: 7,
                base: s.base.clone(),
                folder: Folder::Updated,
            }
        );
        let saved = fs::read_to_string(args).unwrap();
        let wanted = format!(
            "merge\n7\n--repo=lisa/app\n--merge\n--match-head-commit={}\n",
            s.head
        );
        assert!(saved.contains(&wanted), "{saved}");
        assert_eq!(s.repo.current_branch().unwrap(), Some(s.base.clone()));
        assert_eq!(s.repo.head().unwrap(), Some(s.head.clone()));
    }

    #[test]
    fn an_already_merged_pull_request_only_updates_the_folder() {
        let mut s = setup();
        let view =
            r#"{"state":"MERGED","baseRefName":"BASE","headRefOid":"HEAD","mergeable":"UNKNOWN"}"#;
        let args = fake_gh(&mut s, view);

        let merged = merge_with(&s.repo, &s.state, &s.github).unwrap();

        assert_eq!(merged.folder, Folder::Updated);
        let saved = fs::read_to_string(args).unwrap();
        assert!(!saved.contains("merge\n"), "{saved}");
        assert_eq!(s.repo.current_branch().unwrap(), Some(s.base.clone()));
    }

    #[test]
    fn a_pull_request_that_is_not_ready_is_not_merged() {
        let cases = [
            (
                open_view(&"f".repeat(40), "MERGEABLE", GREEN),
                "someone else",
            ),
            (open_view("HEAD", "CONFLICTING", GREEN), "has conflicts"),
            (
                open_view(
                    "HEAD",
                    "MERGEABLE",
                    r#"{"name":"server","status":"COMPLETED","conclusion":"FAILURE"},{"name":"web","status":"IN_PROGRESS","conclusion":null}"#,
                ),
                "checks failed on pull request #7 (server)",
            ),
            (
                open_view(
                    "HEAD",
                    "MERGEABLE",
                    r#"{"name":"web","status":"IN_PROGRESS","conclusion":null},{"context":"lint","state":"PENDING"}"#,
                ),
                "still running (web, lint)",
            ),
            (
                r#"{"state":"CLOSED","baseRefName":"BASE","headRefOid":"HEAD","mergeable":"UNKNOWN"}"#.to_string(),
                "closed on GitHub without merging",
            ),
            (open_view("HEAD", "UNKNOWN", GREEN), "still checking"),
            (
                open_view("HEAD", "MERGEABLE", GREEN).replace("BASE", "release"),
                "goes into release",
            ),
            (open_view("not-a-commit", "MERGEABLE", GREEN), "gave no commit"),
        ];
        for (view, expected) in cases {
            let mut s = setup();
            let args = fake_gh(&mut s, &view);

            let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();

            assert!(error.contains(expected), "{error}");
            let saved = fs::read_to_string(args).unwrap();
            assert!(!saved.contains("merge\n"), "{saved}");
            assert_eq!(
                s.repo.current_branch().unwrap().as_deref(),
                Some("harness/task-001")
            );
        }
    }

    #[test]
    fn unsent_commits_or_missing_checks_stop_the_merge() {
        let mut s = setup();
        // A commit here that GitHub does not have yet.
        let sent = s.head.clone();
        fs::write(s.repo.root().join("late.txt"), "late\n").unwrap();
        s.repo.commit_all("late").unwrap();
        fake_gh(&mut s, &open_view(&sent, "MERGEABLE", GREEN));
        let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
        assert!(error.contains("Choose «Send to GitHub»"), "{error}");

        // A project with workflows whose checks have not shown up yet.
        let mut s = setup();
        fs::create_dir_all(s.repo.root().join(".github/workflows")).unwrap();
        fs::write(s.repo.root().join(".github/workflows/ci.yml"), "on: push\n").unwrap();
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", ""));
        let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
        assert!(error.contains("no checks"), "{error}");
    }

    #[test]
    fn a_base_that_cannot_move_forward_safely_is_left_alone() {
        // The local base has a commit GitHub does not have.
        let mut s = setup();
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", GREEN));
        s.repo.switch_branch(&s.base).unwrap();
        fs::write(s.repo.root().join("mine.txt"), "mine\n").unwrap();
        s.repo.commit_all("only here").unwrap();
        let mine = s.repo.head().unwrap();
        s.repo.switch_branch("harness/task-001").unwrap();

        let merged = merge_with(&s.repo, &s.state, &s.github).unwrap();

        assert!(
            matches!(&merged.folder, Folder::Failed(why) if why.contains("not on GitHub")),
            "{merged:?}"
        );
        assert_eq!(
            s.repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
        assert_eq!(s.repo.branch_head(&s.base).unwrap(), mine);

        // GitHub's base does not hold the merged commit.
        let mut s = setup();
        let first = s.repo.branch_head(&s.base).unwrap().unwrap();
        let back = format!("+{first}:refs/heads/{}", s.base);
        git(s.repo.root(), &["push", "-q", &s.github.push_url, &back]);
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", GREEN));

        let merged = merge_with(&s.repo, &s.state, &s.github).unwrap();

        assert!(
            matches!(&merged.folder, Folder::Failed(why) if why.contains("does not hold")),
            "{merged:?}"
        );
        assert_eq!(
            s.repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
    }

    #[test]
    fn uncommitted_changes_keep_the_folder_as_it_is() {
        let mut s = setup();
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", ""));
        fs::write(s.repo.root().join("notes.txt"), "mine\n").unwrap();

        let merged = merge_with(&s.repo, &s.state, &s.github).unwrap();

        assert_eq!(merged.folder, Folder::Uncommitted(vec!["notes.txt".into()]));
        assert_eq!(
            s.repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
    }

    #[test]
    fn only_a_done_task_with_a_pull_request_is_merged() {
        let mut s = setup();
        s.state.stage = Stage::Working(Role::Developer);
        let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
        assert!(error.contains("not done yet"), "{error}");

        s.state.stage = Stage::Done;
        s.state.pull_request = None;
        let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
        assert!(error.contains("no pull request yet"), "{error}");

        for url in ["file:///tmp/x", "https://github.com/mallory/app/pull/7"] {
            s.state.pull_request = Some(PullRequest {
                number: 7,
                url: url.into(),
            });
            let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
            assert!(error.contains("not in lisa/app"), "{error}");
        }
    }

    #[test]
    fn a_project_not_on_github_has_nothing_to_merge() {
        let s = setup();
        let config = Config::parse("").unwrap();

        let error = merge(&s.repo, &config, &s.state).unwrap_err();

        assert!(error.contains("not on GitHub"), "{error}");
    }

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
