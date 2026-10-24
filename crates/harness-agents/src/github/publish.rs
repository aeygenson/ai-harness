//! Sending a finished task to GitHub: push its branch, open its pull
//! request (or find the one it has), remember the pull request in
//! `state.json`, and push that commit too.
//!
//! Before anything is sent, the branch on GitHub is compared with the local
//! one: commits there that the harness does not have were made by someone
//! else (maybe an agent), and the task stops so the user can look.

use std::path::PathBuf;

use harness_core::config::{Config, GitHubConfig};
use harness_core::git::Repo;
use harness_core::task::branch::task_branch;
use harness_core::task::pull_request::{self, PullRequest};
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState};

use super::pull::{self as pr, NewPullRequest};
use crate::install::credentials;

/// Every push goes to an address that starts like this: GitHub itself.
pub const PUSH_URL_START: &str = "https://github.com/";

/// Where a task goes: the repository from `[github]`, the `gh` program, and
/// the address git pushes to (a local folder in tests).
#[derive(Debug, Clone)]
pub struct GitHub {
    /// The repository as `owner/name`.
    pub repo: String,
    /// The branch pull requests go into.
    pub base: String,
    /// The `gh` program.
    pub gh: PathBuf,
    /// Where `git push` sends the branch.
    pub push_url: String,
}

impl GitHub {
    /// The project's repository from `[github]`, with `gh` installed and
    /// signed in; otherwise an error that says how to fix that and then `again`.
    pub(crate) fn ready(settings: &GitHubConfig, again: &str) -> Result<Self, String> {
        Ok(Self {
            repo: settings.repo.clone(),
            base: settings.base.clone(),
            gh: super::ready_gh(again)?,
            push_url: format!("{PUSH_URL_START}{}.git", settings.repo),
        })
    }
}

/// Sends the finished task to GitHub when the project is on GitHub
/// (`[github]` in `config`); `Ok(None)` otherwise. Needs the GitHub CLI,
/// signed in. Pushes, opens a pull request and commits `state.json`.
pub fn publish(
    repo: &Repo,
    config: &Config,
    store: &TaskStore,
    state: &mut TaskState,
) -> Result<Option<PullRequest>, String> {
    let Some(settings) = &config.github else {
        return Ok(None);
    };
    let github = GitHub::ready(settings, "run the task again")?;
    let secrets = credentials::default_dir()
        .map(|dir| credentials::saved_secrets(&dir))
        .unwrap_or_default();
    let secrets: Vec<&str> = secrets.iter().map(credentials::Secret::expose).collect();
    publish_with(repo, store, state, &github, &secrets).map(Some)
}

/// [`publish`] with everything given: tests use a fake `gh` and a local
/// folder as GitHub. `secrets` are hidden in the pull request's text.
pub fn publish_with(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    github: &GitHub,
    secrets: &[&str],
) -> Result<PullRequest, String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let task = state.task_id.clone();
    if state.stage != Stage::Done {
        return Err(format!(
            "{task} is not done yet, so it is not sent to GitHub"
        ));
    }
    let branch = task_branch(&task);
    if repo.current_branch().map_err(|e| text(&e))?.as_deref() != Some(branch.as_str()) {
        return Err(format!(
            "{task} is not on its branch {branch} (a task created before [github] was set has \
             none), so it is not sent to GitHub; push it yourself if you want a pull request"
        ));
    }
    let state_file = store.dir().join("state.json");
    finish_saved_pull_request(repo, store, state)?;
    let remote = check_remote(repo, &github.push_url, &branch)?;
    if let Some(known) = &state.pull_request {
        known.page().ok_or_else(|| {
            format!("state.json of {task} has a pull request address that is not on GitHub")
        })?;
        // No branch there any more: the pull request was merged and its
        // branch deleted, and nothing new is left to send.
        if remote.is_some() {
            push(repo, github, &branch)?;
        }
        return Ok(known.clone());
    }
    push(repo, github, &branch)?;
    let pull_request = match pr::find(&github.gh, &github.repo, &branch)? {
        Some(found) => found,
        None => open(store, state, github, &branch, secrets)?,
    };
    store
        .save_pull_request(state, pull_request.clone())
        .map_err(|e| text(&e))?;
    repo.commit_paths(&[state_file.as_path()], &commit_message(state))
        .map_err(|e| text(&e))?;
    push(repo, github, &branch)?;
    Ok(pull_request)
}

/// Sends the task's branch to GitHub.
fn push(repo: &Repo, github: &GitHub, branch: &str) -> Result<(), String> {
    repo.push_branch(&github.push_url, branch)
        .map_err(|e| format!("cannot push {branch} to GitHub: {e}"))
}

/// The commit that records the pull request in `state.json`.
fn commit_message(state: &TaskState) -> String {
    let number = state.pull_request.as_ref().map_or(0, |p| p.number);
    format!("{}: pull request #{number}", state.task_id)
}

/// Refuses uncommitted changes, except the one an earlier try left behind:
/// `state.json` with the pull request saved but not committed. That one is
/// committed now, so trying again gets past it.
fn finish_saved_pull_request(
    repo: &Repo,
    store: &TaskStore,
    state: &TaskState,
) -> Result<(), String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let files = repo.changed_files().map_err(|e| text(&e))?;
    if files.is_empty() {
        return Ok(());
    }
    let state_file = store.dir().join("state.json");
    let shown = state_file
        .strip_prefix(repo.root())
        .map(harness_platform::path::slashed)
        .unwrap_or_default();
    if state.pull_request.is_some() && files == [shown] {
        repo.commit_paths(&[state_file.as_path()], &commit_message(state))
            .map_err(|e| text(&e))?;
        return Ok(());
    }
    Err(format!(
        "these files are not committed, so nothing is sent to GitHub: {}",
        files.join(", ")
    ))
}

/// The commit of the task's branch on GitHub, if it is there. Stops when
/// that commit is not in the local branch: the harness never made it, and
/// pushing would mix the two.
fn check_remote(repo: &Repo, url: &str, branch: &str) -> Result<Option<String>, String> {
    let remote = repo
        .remote_branch_head(url, branch)
        .map_err(|e| format!("cannot look at {branch} on GitHub: {e}"))?;
    match &remote {
        Some(commit) if !repo.has_in_history(commit) => Err(format!(
            "{branch} on GitHub has commit {commit}, which the harness did not make (an agent \
             may have pushed it); nothing was sent. Look at it on GitHub, then delete that \
             branch there or bring its commits in yourself"
        )),
        Some(_) | None => Ok(remote),
    }
}

/// Opens the task's pull request with its title and text.
fn open(
    store: &TaskStore,
    state: &TaskState,
    github: &GitHub,
    branch: &str,
    secrets: &[&str],
) -> Result<PullRequest, String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let description = store.description().map_err(|e| text(&e))?;
    let steps = store.steps().map_err(|e| text(&e))?;
    let title = pull_request::title(&state.task_id, &description, secrets);
    let body = pull_request::body(&state.task_id, &description, &steps, secrets);
    let new = NewPullRequest {
        repo: &github.repo,
        base: &github.base,
        branch,
        title: &title,
        body: &body,
    };
    pr::create(&github.gh, &new)
}

/// An empty repository that stands in for GitHub in tests.
#[cfg(test)]
pub(super) fn bare_repo(dir: &std::path::Path) -> String {
    let status = std::process::Command::new("git")
        .args(["init", "--quiet", "--bare"])
        .arg(dir)
        .status()
        .expect("git runs");
    assert!(status.success());
    dir.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use harness_core::task::handoff::Role;

    use super::*;

    const ON_GITHUB: &str = "[github]\nrepo = \"lisa/app\"\n";

    /// A project on GitHub with a finished task-001 on its branch, a folder
    /// standing in for GitHub, and a fake `gh` that lists no pull request
    /// and opens #7.
    struct Setup {
        dir: tempfile::TempDir,
        repo: Repo,
        store: TaskStore,
        state: TaskState,
        github: GitHub,
        /// The branch the project started on.
        base: String,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let repo = Repo::init(&project).unwrap();
        fs::write(project.join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        let base = repo.current_branch().unwrap().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();
        let (store, mut state) = harness_core::task::orchestrator::create_project_task(
            &repo,
            &config,
            "task-001",
            "Add a parser\nwith tests",
            harness_core::task::orchestrator::SameText::Refuse,
        )
        .unwrap();
        state.stage = Stage::Done;
        fs::write(
            store.dir().join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();
        repo.commit_all("task-001 done").unwrap();
        let bin = dir.path().join("bin");
        let script = format!(
            "save-stdin {}\nwhen-arg 2 list print []\nwhen-arg 2 create print https://github.com/lisa/app/pull/7\n",
            dir.path().join("body.md").display()
        );
        let gh = harness_fake::install(&bin, "gh", &script);
        let github = GitHub {
            repo: "lisa/app".into(),
            base: "main".into(),
            gh,
            push_url: bare_repo(&dir.path().join("github.git")),
        };
        Setup {
            dir,
            repo,
            store,
            state,
            github,
            base,
        }
    }

    #[test]
    fn a_finished_task_is_pushed_and_becomes_a_pull_request() {
        let mut s = setup();

        let made = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();

        assert_eq!(made.url, "https://github.com/lisa/app/pull/7");
        // Remembered in state.json, committed and pushed too.
        assert_eq!(s.state.pull_request, Some(made.clone()));
        let (_, saved) = TaskStore::open(&s.repo.runs_dir(), "task-001").unwrap();
        assert_eq!(saved.pull_request, Some(made));
        assert_eq!(s.repo.changed_files().unwrap(), Vec::<String>::new());
        let remote = s
            .repo
            .remote_branch_head(&s.github.push_url, "harness/task-001")
            .unwrap();
        assert_eq!(remote, s.repo.head().unwrap());
        let body = fs::read_to_string(s.dir.path().join("body.md")).unwrap();
        assert!(body.contains("Add a parser"), "{body}");
    }

    #[test]
    fn a_second_publish_only_pushes_again() {
        let mut s = setup();
        publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();
        let commits = s.repo.head().unwrap();
        // A `gh` that would fail: it must not be asked again.
        s.github.gh = s.dir.path().join("no-gh");

        let again = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();

        assert_eq!(again.number, 7);
        assert_eq!(s.repo.head().unwrap(), commits);
    }

    #[test]
    fn a_try_that_stopped_before_its_commit_is_finished_next_time() {
        let mut s = setup();
        let made = PullRequest {
            number: 7,
            url: "https://github.com/lisa/app/pull/7".into(),
        };
        // The pull request was saved, then the commit failed.
        s.store
            .save_pull_request(&mut s.state, made.clone())
            .unwrap();
        s.github.gh = s.dir.path().join("no-gh");

        let again = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();

        assert_eq!(again, made);
        assert_eq!(s.repo.changed_files().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_merged_pull_request_whose_branch_is_gone_is_not_pushed_again() {
        let mut s = setup();
        publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();
        // Merged on GitHub, and the branch deleted there.
        let status = std::process::Command::new("git")
            .current_dir(s.repo.root())
            .args([
                "push",
                "-q",
                &s.github.push_url,
                "--delete",
                "harness/task-001",
            ])
            .status()
            .unwrap();
        assert!(status.success());

        publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap();

        let remote = s
            .repo
            .remote_branch_head(&s.github.push_url, "harness/task-001")
            .unwrap();
        assert_eq!(remote, None);
    }

    #[test]
    fn a_strange_address_in_state_json_is_refused() {
        let mut s = setup();
        let strange = PullRequest {
            number: 7,
            url: "file:///tmp/x.exe".into(),
        };
        s.store.save_pull_request(&mut s.state, strange).unwrap();
        s.repo.commit_all("hand edit").unwrap();

        let error = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap_err();

        assert!(error.contains("not on GitHub"), "{error}");
    }

    #[test]
    fn commits_on_github_the_harness_did_not_make_stop_it() {
        let mut s = setup();
        // An agent pushed another commit to the task's branch.
        s.repo.create_branch("planted").unwrap();
        fs::write(s.repo.root().join("x.txt"), "x\n").unwrap();
        s.repo.commit_all("planted").unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(s.repo.root())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&[
            "push",
            "-q",
            &s.github.push_url,
            "planted:refs/heads/harness/task-001",
        ]);
        s.repo.switch_branch("harness/task-001").unwrap();

        let error = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap_err();

        assert!(error.contains("which the harness did not make"), "{error}");
        assert_eq!(s.state.pull_request, None);
    }

    #[test]
    fn only_a_done_task_on_its_branch_is_sent() {
        let mut s = setup();
        s.state.stage = Stage::Working(Role::Developer);
        let error = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap_err();
        assert!(error.contains("not done yet"), "{error}");

        s.state.stage = Stage::Done;
        s.repo.switch_branch(&s.base).unwrap();
        let error = publish_with(&s.repo, &s.store, &mut s.state, &s.github, &[]).unwrap_err();
        assert!(
            error.contains("is not on its branch harness/task-001"),
            "{error}"
        );
    }

    #[test]
    fn a_project_not_on_github_sends_nothing() {
        let mut s = setup();
        let config = Config::parse("").unwrap();

        let sent = publish(&s.repo, &config, &s.store, &mut s.state).unwrap();

        assert_eq!(sent, None);
    }
}
