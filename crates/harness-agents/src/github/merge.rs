//! Merging a done task's pull request on GitHub, then bringing the merged
//! work into the project's main branch (`base`), so the next task starts
//! from it.
//!
//! Lisa decides to merge; the harness only does the clicks. Before merging
//! it checks what GitHub says: the pull request must still be at the commit
//! the harness sent (no commits someone else pushed), without conflicts and
//! with its checks (CI) passed.

use std::time::Instant;

use harness_core::config::Config;
use harness_core::git::Repo;
use harness_core::task::branch::task_branch;
use harness_core::task::{Stage, TaskState};
use harness_core::text;

use super::pull::{self as pr, View};
use super::ready::{check_identity, check_ready, is_commit, NotReady, SHOWN_CHARS};
use super::wait::Waiting;
use super::GitHub;

/// What merging did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    /// The pull request's number.
    pub number: u64,
    /// The branch it was merged into.
    pub base: String,
    /// Whether the project folder now holds the merged work.
    pub folder: Folder,
    /// The task's branch here has commits made after the merge (a retro,
    /// settings); they stay on that branch.
    pub left_behind: bool,
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

/// Merges the pull request of the done task in `state` and updates the
/// project folder. Needs `[github]` in `config` and the GitHub CLI, signed in.
/// While GitHub is still busy with the pull request (checks running, the
/// last commits just sent) it waits as `wait` says, then merges.
pub fn merge(
    repo: &Repo,
    config: &Config,
    state: &TaskState,
    wait: &mut Waiting,
) -> Result<Merged, String> {
    let settings = config
        .github
        .as_ref()
        .ok_or_else(|| "this project is not on GitHub, so there is nothing to merge".to_string())?;
    let github = GitHub::ready(settings, "merge again")?;
    merge_with(repo, state, &github, wait)
}

/// [`merge`] with everything given: tests use a fake `gh` and a local
/// folder as GitHub.
pub fn merge_with(
    repo: &Repo,
    state: &TaskState,
    github: &GitHub,
    wait: &mut Waiting,
) -> Result<Merged, String> {
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
    let branch = task_branch(task);
    let seen = merge_when_ready(repo, github, &branch, number, wait)?;
    // The commit the merge made: after a squash or rebase merge on GitHub
    // the branch's own last commit is not in the base branch, this one is.
    let merged = seen
        .merge_commit
        .as_ref()
        .map_or(seen.head_ref_oid.as_str(), |c| c.oid.as_str());
    if !is_commit(merged) {
        return Err(format!(
            "GitHub gave no merge commit for pull request #{number}; merge again to see where \
             it stands"
        ));
    }
    let left_behind = left_behind(repo, &branch, number, &seen.head_ref_oid)?;
    Ok(Merged {
        number,
        base: github.base.clone(),
        folder: update_base(repo, github, merged),
        left_behind,
    })
}

/// Asks GitHub about the pull request until it is merged: merges it once it
/// is ready, and waits while GitHub is still busy with it. The pull request
/// as GitHub shows it after the merge.
fn merge_when_ready(
    repo: &Repo,
    github: &GitHub,
    branch: &str,
    number: u64,
    wait: &mut Waiting,
) -> Result<View, String> {
    let started = Instant::now();
    loop {
        let seen = pr::view(&github.gh, &github.repo, number)?;
        check_identity(&seen, number, branch, &github.base)?;
        match seen.state.as_str() {
            // Merged earlier (on GitHub, or by a try that stopped before the
            // folder was updated): only the folder is left to do.
            "MERGED" => return Ok(seen),
            "OPEN" => match check_ready(repo, github, branch, number, &seen) {
                Ok(()) => return merge_and_confirm(github, number, &seen.head_ref_oid),
                Err(NotReady::Refused(why)) => return Err(why),
                Err(NotReady::Later(why)) => wait.wait(started, why)?,
            },
            "CLOSED" => {
                return Err(format!(
                    "pull request #{number} was closed on GitHub without merging; open it \
                     again there if it should be merged"
                ))
            }
            other => {
                return Err(format!(
                    "GitHub says pull request #{number} is {}; nothing was merged",
                    text::safe_line(other, SHOWN_CHARS)
                ))
            }
        }
    }
}

/// Compares what GitHub merged (`head`) with the task's branch here: `true`
/// when the branch has commits made after it. A commit the harness did not
/// make (an agent may have pushed and merged it) stops before the folder changes.
fn left_behind(repo: &Repo, branch: &str, number: u64, head: &str) -> Result<bool, String> {
    let Some(local) = repo.branch_head(branch).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    if local == head {
        return Ok(false);
    }
    if repo.is_ancestor(head, &local) {
        return Ok(true);
    }
    Err(format!(
        "pull request #{number} was merged at commit {head}, which is not on {branch} here (an \
         agent or someone else may have pushed it); the folder was not changed. Look at it on \
         GitHub"
    ))
}

/// Asks GitHub to merge at `head`, then asks again how the pull request
/// stands: `gh` may fail after GitHub merged, or succeed while a merge queue
/// still waits. Returns the merged pull request.
fn merge_and_confirm(github: &GitHub, number: u64, head: &str) -> Result<View, String> {
    let asked = pr::merge(&github.gh, &github.repo, number, head);
    let after = pr::view(&github.gh, &github.repo, number).map_err(|e| {
        format!(
            "GitHub did not confirm the merge of pull request #{number} ({e}); choose «Merge \
             pull request» again to see where it stands"
        )
    })?;
    if after.state == "MERGED" {
        return Ok(after);
    }
    Err(match asked {
        Err(e) => format!("GitHub refused to merge pull request #{number}: {e}"),
        Ok(()) if after.state != "OPEN" => format!(
            "pull request #{number} is not merged, GitHub says it is {}",
            text::safe_line(&after.state, SHOWN_CHARS)
        ),
        Ok(()) => format!(
            "GitHub took the merge of pull request #{number} but has not merged it yet (a \
             merge queue or auto-merge may be waiting); choose «Merge pull request» again later"
        ),
    })
}

/// Brings the merged work (`merged`, the merge's commit) into `base` here
/// and checks `base` out. Everything is checked before the folder changes:
/// no uncommitted change, GitHub's `base` holds the merge, and the local
/// `base` has nothing GitHub lacks.
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
            "{base} on GitHub does not hold the merge commit {merged} yet; GitHub may still be \
             updating it, so choose «Merge pull request» again in a moment"
        ));
    }
    let local = match repo.branch_head(base) {
        Ok(local) => local,
        Err(e) => return Folder::Failed(message(&e)),
    };
    if let Some(local) = &local {
        if !repo.is_ancestor(local, &fetched) {
            return Folder::Failed(format!(
                "{base} here has commits that are not on GitHub; bring the two together \
                 yourself"
            ));
        }
    }
    let on_base = matches!(repo.current_branch(), Ok(Some(name)) if name == *base);
    if on_base {
        return match repo.fast_forward_to(&fetched) {
            Ok(()) => Folder::Updated,
            Err(e) => Folder::Failed(message(&e)),
        };
    }
    // Not checked out: the branch is moved first (no file changes, and only
    // from where it was a moment ago), then checked out.
    if let Err(e) = repo.move_branch(base, &fetched, local.as_deref()) {
        return Folder::Failed(message(&e));
    }
    match repo.switch_branch(base) {
        Ok(()) => Folder::Updated,
        Err(e) => Folder::Failed(format!(
            "{base} here now holds the merged work, but the folder could not switch to it: {e}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

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

    /// A fake `gh`: `pr view` prints `view` until `pr merge` runs, then
    /// `after` (`None`: `pr merge` fails and nothing changes). `BASE` and
    /// `HEAD` in them become the real names. Returns the file with the calls.
    fn fake_gh(s: &mut Setup, view: &str, after: Option<&str>) -> PathBuf {
        let real = |text: &str| text.replace("BASE", &s.base).replace("HEAD", &s.head);
        let bin = s.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let args = bin.join("args.txt");
        let now = bin.join("view.json");
        fs::write(&now, real(view)).unwrap();
        let merge = match after {
            Some(after) => {
                let next = bin.join("after.json");
                fs::write(&next, real(after)).unwrap();
                format!("when-arg 2 merge copy {} {}", next.display(), now.display())
            }
            None => "when-arg 2 merge exit 1".to_string(),
        };
        let script = format!(
            "append-args {}\n{merge}\nwhen-arg 2 view print-file {}\n",
            args.display(),
            now.display()
        );
        s.github.gh = harness_fake::install(&bin, "gh", &script);
        args
    }

    /// What `gh pr view` prints for an open pull request of task-001 into `BASE`.
    fn open_view(head: &str, mergeable: &str, checks: &str) -> String {
        format!(
            r#"{{"state":"OPEN","baseRefName":"BASE","headRefName":"harness/task-001","headRefOid":"{head}","mergeable":"{mergeable}","statusCheckRollup":[{checks}]}}"#
        )
    }

    /// What `gh pr view` prints once it is merged with the commit `merge`.
    fn merged_view(merge: &str) -> String {
        format!(
            r#"{{"state":"MERGED","baseRefName":"BASE","headRefName":"harness/task-001","headRefOid":"HEAD","mergeable":"UNKNOWN","mergeCommit":{{"oid":"{merge}"}},"statusCheckRollup":null}}"#
        )
    }

    const GREEN: &str = r#"{"name":"server","status":"COMPLETED","conclusion":"SUCCESS"},{"context":"lint","state":"SUCCESS"},{"name":"docs","status":"COMPLETED","conclusion":"SKIPPED"}"#;

    fn merged_calls(args: &Path) -> bool {
        fs::read_to_string(args).unwrap().contains("merge\n7\n")
    }

    fn on_task_branch(s: &Setup) -> bool {
        s.repo.current_branch().unwrap().as_deref() == Some("harness/task-001")
    }

    #[test]
    fn a_green_pull_request_is_merged_and_the_folder_updated() {
        let mut s = setup();
        let open = open_view("HEAD", "MERGEABLE", GREEN);
        let args = fake_gh(&mut s, &open, Some(&merged_view("HEAD")));

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();

        assert_eq!(
            merged,
            Merged {
                number: 7,
                base: s.base.clone(),
                folder: Folder::Updated,
                left_behind: false,
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
    fn running_checks_are_waited_for_and_then_merged() {
        let mut s = setup();
        let running = open_view(
            "HEAD",
            "MERGEABLE",
            r#"{"name":"server","status":"IN_PROGRESS","conclusion":null}"#,
        );
        let args = fake_gh(&mut s, &running, Some(&merged_view("HEAD")));
        // While the harness waits, GitHub finishes the checks.
        let view_file = s.dir.path().join("bin").join("view.json");
        let green = open_view(&s.head, "MERGEABLE", GREEN).replace("BASE", &s.base);
        let mut notes = Vec::new();
        let mut wait = Waiting {
            limit: Duration::from_mins(1),
            pause: Duration::from_millis(10),
            stopped: Box::new(|| false),
            note: Box::new(|line| {
                notes.push(line.to_string());
                fs::write(&view_file, &green).unwrap();
            }),
        };

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut wait).unwrap();

        drop(wait);
        assert_eq!(merged.folder, Folder::Updated);
        assert!(merged_calls(&args));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("still running (server)"), "{notes:?}");
    }

    #[test]
    fn a_refusal_is_not_waited_for() {
        let mut s = setup();
        let conflicting = open_view("HEAD", "CONFLICTING", GREEN);
        let args = fake_gh(&mut s, &conflicting, None);
        let mut waited = false;
        let mut wait = Waiting {
            limit: Duration::from_mins(1),
            pause: Duration::from_millis(10),
            stopped: Box::new(|| false),
            note: Box::new(|_| waited = true),
        };

        let error = merge_with(&s.repo, &s.state, &s.github, &mut wait).unwrap_err();

        drop(wait);
        assert!(error.contains("has conflicts"), "{error}");
        assert!(!waited);
        assert!(!merged_calls(&args));
    }

    #[test]
    fn a_pull_request_squashed_on_github_only_updates_the_folder() {
        let mut s = setup();
        // GitHub's base gets one new commit instead of the task's branch.
        s.repo.switch_branch(&s.base).unwrap();
        fs::write(s.repo.root().join("parser.txt"), "parser\n").unwrap();
        s.repo.commit_all("task-001 squashed").unwrap();
        let squashed = s.repo.head().unwrap().unwrap();
        let force = format!("+{squashed}:refs/heads/{}", s.base);
        git(s.repo.root(), &["push", "-q", &s.github.push_url, &force]);
        git(s.repo.root(), &["reset", "-q", "--hard", "HEAD~1"]);
        s.repo.switch_branch("harness/task-001").unwrap();
        let args = fake_gh(&mut s, &merged_view(&squashed), None);

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();

        assert_eq!(merged.folder, Folder::Updated);
        assert!(!merged_calls(&args));
        assert_eq!(s.repo.current_branch().unwrap(), Some(s.base.clone()));
        assert_eq!(s.repo.head().unwrap(), Some(squashed));
    }

    #[test]
    fn a_merged_pull_request_is_compared_with_the_task_branch() {
        // GitHub merged a commit the harness never made.
        let mut s = setup();
        let foreign = merged_view("HEAD").replace(
            r#""headRefOid":"HEAD""#,
            &format!(r#""headRefOid":"{}""#, "e".repeat(40)),
        );
        fake_gh(&mut s, &foreign, None);
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("not on harness/task-001 here"), "{error}");
        assert!(on_task_branch(&s));

        // The task's branch has a commit made after the merge.
        let mut s = setup();
        fake_gh(&mut s, &merged_view("HEAD"), None);
        fs::write(s.repo.root().join("retro.md"), "later\n").unwrap();
        s.repo.commit_all("retro").unwrap();
        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();
        assert!(merged.left_behind);
        assert_eq!(merged.folder, Folder::Updated);
    }

    #[test]
    fn a_missing_local_base_is_made_from_github() {
        let mut s = setup();
        git(s.repo.root(), &["branch", "-q", "-D", &s.base.clone()]);
        fake_gh(&mut s, &merged_view("HEAD"), None);

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();

        assert_eq!(merged.folder, Folder::Updated);
        assert_eq!(s.repo.branch_head(&s.base).unwrap(), Some(s.head.clone()));
    }

    #[test]
    fn a_pull_request_that_is_not_ready_is_not_merged() {
        let cases = [
            (
                open_view(&"f".repeat(40), "MERGEABLE", GREEN),
                "the harness did not make",
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
                merged_view("HEAD").replace("MERGED", "CLOSED"),
                "closed on GitHub without merging",
            ),
            (open_view("HEAD", "UNKNOWN", GREEN), "still checking"),
            (
                open_view("HEAD", "MERGEABLE", GREEN).replace("BASE", "release"),
                "goes into release",
            ),
            (
                open_view("HEAD", "MERGEABLE", GREEN).replace("harness/task-001", "planted"),
                "does not come from harness/task-001",
            ),
            (
                open_view("HEAD", "MERGEABLE", GREEN)
                    .replace("\"state\"", "\"isCrossRepository\":true,\"state\""),
                "does not come from harness/task-001",
            ),
            (
                open_view("HEAD", "MERGEABLE", GREEN)
                    .replace("\"state\"", "\"isDraft\":true,\"state\""),
                "is a draft",
            ),
            (
                open_view("not-a-commit", "MERGEABLE", GREEN),
                "gave no commit",
            ),
        ];
        for (view, expected) in cases {
            let mut s = setup();
            let args = fake_gh(&mut s, &view, None);

            let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();

            assert!(error.contains(expected), "{error}");
            assert!(!merged_calls(&args), "{expected}");
            assert!(on_task_branch(&s));
        }
    }

    #[test]
    fn what_github_says_after_merging_decides() {
        // GitHub refused: the pull request is still open.
        let mut s = setup();
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", GREEN), None);
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("GitHub refused to merge"), "{error}");

        // GitHub took it, but a merge queue still waits.
        let mut s = setup();
        let open = open_view("HEAD", "MERGEABLE", GREEN);
        fake_gh(&mut s, &open, Some(&open));
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("has not merged it yet"), "{error}");
        assert!(on_task_branch(&s));
    }

    #[test]
    fn uncommitted_changes_stop_the_merge_or_keep_the_folder() {
        let mut s = setup();
        let args = fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", ""), None);
        fs::write(s.repo.root().join("notes.txt"), "mine\n").unwrap();
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("uncommitted changes"), "{error}");
        assert!(!merged_calls(&args));

        // Already merged: only the folder is left, and it waits.
        fake_gh(&mut s, &merged_view("HEAD"), None);
        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();
        assert_eq!(merged.folder, Folder::Uncommitted(vec!["notes.txt".into()]));
        assert!(on_task_branch(&s));
    }

    #[test]
    fn unsent_commits_or_missing_checks_stop_the_merge() {
        let mut s = setup();
        // A commit here that GitHub does not have yet.
        fs::write(s.repo.root().join("late.txt"), "late\n").unwrap();
        s.repo.commit_all("late").unwrap();
        let args = fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", GREEN), None);
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("sent the last commits"), "{error}");
        assert!(!merged_calls(&args));
        let sent = s
            .repo
            .remote_branch_head(&s.github.push_url, "harness/task-001")
            .unwrap();
        assert_eq!(sent, s.repo.head().unwrap());

        // The base branch on GitHub has workflows, but no checks have shown
        // up yet. (The local branches do not count: an agent could change them.)
        let mut s = setup();
        s.repo.create_branch("with-ci").unwrap();
        let workflows = s.repo.root().join(".github/workflows");
        fs::create_dir_all(&workflows).unwrap();
        fs::write(workflows.join("ci.yml"), "on: push\n").unwrap();
        s.repo.commit_all("ci").unwrap();
        let to_base = format!("with-ci:refs/heads/{}", s.base);
        git(s.repo.root(), &["push", "-q", &s.github.push_url, &to_base]);
        s.repo.switch_branch("harness/task-001").unwrap();
        fake_gh(&mut s, &open_view("HEAD", "MERGEABLE", ""), None);
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("no checks"), "{error}");
    }

    #[test]
    fn a_base_that_cannot_move_forward_safely_is_left_alone() {
        // The local base has a commit GitHub does not have.
        let mut s = setup();
        fake_gh(&mut s, &merged_view("HEAD"), None);
        s.repo.switch_branch(&s.base).unwrap();
        fs::write(s.repo.root().join("mine.txt"), "mine\n").unwrap();
        s.repo.commit_all("only here").unwrap();
        let mine = s.repo.head().unwrap();
        s.repo.switch_branch("harness/task-001").unwrap();

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();

        assert!(
            matches!(&merged.folder, Folder::Failed(why) if why.contains("not on GitHub")),
            "{merged:?}"
        );
        assert!(on_task_branch(&s));
        assert_eq!(s.repo.branch_head(&s.base).unwrap(), mine);

        // GitHub's base does not hold the merge commit.
        let mut s = setup();
        let first = s.repo.branch_head(&s.base).unwrap().unwrap();
        let back = format!("+{first}:refs/heads/{}", s.base);
        git(s.repo.root(), &["push", "-q", &s.github.push_url, &back]);
        fake_gh(&mut s, &merged_view("HEAD"), None);

        let merged = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap();

        assert!(
            matches!(&merged.folder, Folder::Failed(why) if why.contains("does not hold")),
            "{merged:?}"
        );
        assert!(on_task_branch(&s));
    }

    #[test]
    fn only_a_done_task_with_its_pull_request_is_merged() {
        let mut s = setup();
        s.state.stage = Stage::Working(Role::Developer);
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("not done yet"), "{error}");

        s.state.stage = Stage::Done;
        s.state.pull_request = None;
        let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
        assert!(error.contains("no pull request yet"), "{error}");

        for url in ["file:///tmp/x", "https://github.com/mallory/app/pull/7"] {
            s.state.pull_request = Some(PullRequest {
                number: 7,
                url: url.into(),
            });
            let error = merge_with(&s.repo, &s.state, &s.github, &mut Waiting::none()).unwrap_err();
            assert!(error.contains("not in lisa/app"), "{error}");
        }
    }

    #[test]
    fn a_project_not_on_github_has_nothing_to_merge() {
        let s = setup();
        let config = Config::parse("").unwrap();

        let error = merge(&s.repo, &config, &s.state, &mut Waiting::none()).unwrap_err();

        assert!(error.contains("not on GitHub"), "{error}");
    }
}
