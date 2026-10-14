//! Merging a done task's pull request on GitHub, then bringing the merged
//! work into the project's main branch (`base`), so the next task starts
//! from it.
//!
//! Lisa decides to merge; the harness only does the clicks. Before merging
//! it checks what GitHub says: the pull request must still be at the commit
//! the harness sent (no commits someone else pushed), without conflicts and
//! with its checks (CI) passed.

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
    /// Switching to `base` or bringing in the merged work failed: git's message.
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
    pull_request.page().ok_or_else(|| {
        format!("state.json of {task} has a pull request address that is not on GitHub")
    })?;
    let number = pull_request.number;
    let seen = pr::view(&github.gh, &github.repo, number)?;
    match seen.state.as_str() {
        // Merged earlier (on GitHub, or by a try that stopped before the
        // folder was updated): only the folder is left to do.
        "MERGED" => {}
        "OPEN" => {
            check_ready(repo, &task_branch(task), number, &seen)?;
            pr::merge(&github.gh, &github.repo, number, &seen.head_ref_oid)?;
        }
        _ => {
            return Err(format!(
                "pull request #{number} was closed on GitHub without merging; open it again \
                 there if it should be merged"
            ))
        }
    }
    Ok(Merged {
        number,
        base: github.base.clone(),
        folder: update_base(repo, github),
    })
}

/// Refuses to merge an open pull request that is not at the commit of the
/// task's branch here, has conflicts or has checks that did not pass.
fn check_ready(repo: &Repo, branch: &str, number: u64, seen: &View) -> Result<(), String> {
    let local = repo
        .branch_head(branch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("the branch {branch} is not in this project; nothing was merged"))?;
    if seen.head_ref_oid != local {
        return Err(format!(
            "pull request #{number} is at commit {}, but {branch} here is at {local}: either \
             someone else (maybe an agent) pushed to it, or the last commits were not sent. \
             Nothing was merged; look at the pull request on GitHub",
            text::safe_line(&seen.head_ref_oid, SHOWN_CHARS)
        ));
    }
    if seen.mergeable == "CONFLICTING" {
        return Err(format!(
            "pull request #{number} has conflicts with the main branch; nothing was merged. \
             Resolve them on GitHub, then merge again"
        ));
    }
    match checks(&seen.status_check_rollup) {
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

/// Checks out `base` and brings the merged work in from GitHub, only when
/// no uncommitted change could be lost.
fn update_base(repo: &Repo, github: &GitHub) -> Folder {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let files = match repo.changed_files() {
        Ok(files) => files,
        Err(e) => return Folder::Failed(text(&e)),
    };
    if !files.is_empty() {
        return Folder::Uncommitted(files);
    }
    let on_base = matches!(repo.current_branch(), Ok(Some(name)) if name == github.base);
    if !on_base {
        if let Err(e) = repo.switch_branch(&github.base) {
            return Folder::Failed(text(&e));
        }
    }
    match repo.fast_forward_from(&github.push_url, &github.base) {
        Ok(()) => Folder::Updated,
        Err(e) => Folder::Failed(text(&e)),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
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
        let bin = s.dir.path().join("bin");
        let args = bin.join("args.txt");
        let script = format!(
            "append-args {}\nwhen-arg 2 view print {view}\n",
            args.display()
        );
        s.github.gh = harness_fake::install(&bin, "gh", &script);
        args
    }

    fn open_view(head: &str, mergeable: &str, checks: &str) -> String {
        format!(
            r#"{{"state":"OPEN","headRefOid":"{head}","mergeable":"{mergeable}","statusCheckRollup":[{checks}]}}"#
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
        let view = format!(
            r#"{{"state":"MERGED","headRefOid":"{}","mergeable":"UNKNOWN"}}"#,
            s.head
        );
        let args = fake_gh(&mut s, &view);

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
                r#"{"state":"CLOSED","headRefOid":"HEAD","mergeable":"UNKNOWN"}"#.to_string(),
                "closed on GitHub without merging",
            ),
        ];
        for (view, expected) in cases {
            let mut s = setup();
            let view = view.replace("HEAD", &s.head);
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
    fn uncommitted_changes_keep_the_folder_as_it_is() {
        let mut s = setup();
        let view = open_view(&s.head, "MERGEABLE", "");
        fake_gh(&mut s, &view);
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

        s.state.pull_request = Some(PullRequest {
            number: 7,
            url: "file:///tmp/x".into(),
        });
        let error = merge_with(&s.repo, &s.state, &s.github).unwrap_err();
        assert!(error.contains("not on GitHub"), "{error}");
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
