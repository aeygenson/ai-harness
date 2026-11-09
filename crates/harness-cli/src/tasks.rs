//! `harness task new`, `run`, `approve`, `reject`, `stop` and `status`: moving
//! a task through the roles.
//!
//! Every command that changes a task first holds the project's run lock
//! (`harness_core::task::run_lock`), so it never works under the feet of the
//! TUI or of another terminal.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_agents::build::build_team;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::skills::Skills;
use harness_core::task::branch;
use harness_core::task::handoff::{AnswerTo, Role, MAX_REASON_CHARS};
use harness_core::task::orchestrator::{self, GitAfterRole, Project, SameText, StopReason};
use harness_core::task::run_lock::{self, RunLock};
use harness_core::task::stop_request::{StopRequest, StopWhen};
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason};
use harness_platform::stop::StopSignals;

use crate::decide::step_name;
use crate::{open_repo, open_task};

/// `harness task new`: creates the task folder and commits it. Refuses a
/// description an unfinished task already has, unless `allow_duplicate`.
pub(crate) fn new_task(
    project: &Path,
    task_id: &str,
    description: &str,
    allow_duplicate: bool,
) -> Result<()> {
    let repo = open_repo(project)?;
    let mut lock = RunLock::hold(&repo)?;
    lock.set_task(task_id)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let same_text = if allow_duplicate {
        SameText::Allow
    } else {
        SameText::Refuse
    };
    let (store, _) =
        orchestrator::create_project_task(&repo, &config, task_id, description, same_text)?;
    println!(
        "Created {}. Next: harness run {task_id}",
        store.dir().display()
    );
    if config.github.is_some() {
        println!(
            "It works on its own branch {} in its own folder: {}",
            branch::task_branch(task_id),
            branch::task_folder(&repo, task_id)?.display()
        );
    }
    Ok(())
}

/// Waits for `work` (which runs agents) until it ends or the harness is asked
/// to stop: Ctrl+C, the terminal window closed, `kill` (see `harness_platform::stop`).
///
/// Each agent runs in its own process group, so these signals reach only the
/// harness. Here the harness stops waiting, which drops `work`, and dropping
/// it stops the running agent with everything it started (see
/// `harness_agents::process::run`).
pub(crate) async fn until_stopped(
    work: impl std::future::Future<Output = Result<()>>,
) -> Result<()> {
    let mut signals = StopSignals::listen().context("cannot listen for Ctrl+C")?;
    // `select!` waits for whichever finishes first and drops the other one.
    tokio::select! {
        result = work => result,
        () = signals.recv() => {
            bail!("stopped (Ctrl+C or the terminal closed); the agent and everything it started were stopped")
        }
    }
}

/// `harness run`: runs the roles one after another until the task is done
/// or waits for the user, then says what to do next. A task the user stopped in the
/// TUI continues with the role they stopped (or the one that was next).
pub(crate) async fn run(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    // Held until the run ends: `harness stop` reaches this run through it.
    let mut lock = RunLock::hold(&repo)?;
    lock.set_task(task_id)?;
    // The settings and skills are the project folder's; the work is done in
    // the task's own folder.
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = Config::load(&harness_dir)?;
    let skills = Skills::load(&harness_dir, &config)?;
    let agent = build_team(&config, repo.root())?;
    let repo = enter_task(&repo, task_id)?;
    let (store, mut state) = open_task(&repo, task_id)?;
    if orchestrator::continue_stopped(&repo, &store, &mut state)? {
        println!("You stopped {task_id}; it continues now.");
    }
    println!("Running {task_id} (round {})...", state.round);
    // The command line has no stop buttons: Ctrl+C stops it (`until_stopped`),
    // and `harness stop` from another terminal through the lock's stop file.
    let stop = StopRequest::new();
    lock.connect(&stop)?;
    let project = Project {
        config: &config,
        skills: &skills,
        agent_versions: agent.versions(),
        stop: &stop,
    };
    let stop = orchestrator::run_project(&repo, &store, &mut state, &agent, project).await?;
    let suggested = suggestion(&store, &state, &stop);
    println!("{}", explain(&stop, task_id, suggested.as_ref()));
    if stop == StopReason::Done {
        // Running a done task again sends it again: the way to retry after a
        // failed push.
        match harness_agents::github::publish(&repo, &config, &store, &mut state) {
            Ok(Some(pull_request)) => println!("Pull request: {}", pull_request.url),
            Ok(None) => {}
            // The error can hold what GitHub's server printed: no terminal codes.
            Err(error) => bail!(
                "{task_id} is done, but it was not sent to GitHub: {}\n\
                 Once that is fixed, send it with: harness run {task_id}",
                harness_core::text::safe(&error)
            ),
        }
    }
    Ok(())
}

/// The repository the task works in (its own folder in a project on GitHub,
/// see `branch::enter`); says where that is when it is not the project folder.
pub(crate) fn enter_task(repo: &Repo, task_id: &str) -> Result<Repo> {
    let task_repo = branch::enter(repo, task_id)?;
    // Git may spell the same folder differently (`/private/tmp` on macOS).
    let same = match (task_repo.root().canonicalize(), repo.root().canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => task_repo.root() == repo.root(),
    };
    if !same {
        println!("{task_id} works in {}", task_repo.root().display());
    }
    Ok(task_repo)
}

/// Whom the role that asked for help suggests for the user's answer (its
/// handoff's `answer_to`); `None` when the run stopped for another reason.
///
/// It is only a hint: if the history cannot be read, the stop message is
/// still printed, just without it (`ok()` on purpose).
fn suggestion(store: &TaskStore, state: &TaskState, stop: &StopReason) -> Option<AnswerTo> {
    if !matches!(
        stop,
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(_))
    ) {
        return None;
    }
    // The last handoff is the question of the role that asked.
    let last = store.history().ok()?.pop()?;
    last.suggested_answer(state.workflow.scenario()).cloned()
}

/// What to say when `role` asked for help: where its questions are, whom it
/// suggests for the answer, and the command that answers.
fn help_request_text(role: Role, task_id: &str, suggested: Option<&AnswerTo>) -> String {
    let Some(answer) = suggested else {
        return format!(
            "The {role} needs your help; see its notes.md. Answer with:\n  \
             harness approve {task_id} --notes \"...\""
        );
    };
    // The reason is the agent's text: one clean line.
    let reason = harness_core::text::safe_line(&answer.reason, MAX_REASON_CHARS);
    let to = answer.role;
    let without_to = if to == role {
        String::new()
    } else {
        format!("\n(without --to, the {role} gets it)")
    };
    format!(
        "The {role} needs your help; see its notes.md.\n\
         It suggests that the {to} gets your answer: {reason}\n\
         Answer with:\n  harness approve {task_id} --to {to} --notes \"...\"{without_to}"
    )
}

/// The message telling the user why the run stopped and which command comes next;
/// `suggested` is whom a role that asked for help suggests for the answer.
fn explain(stop: &StopReason, task_id: &str, suggested: Option<&AnswerTo>) -> String {
    match stop {
        StopReason::Done => "Done: the work is approved.".into(),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => format!(
            "The architect's design is ready. Read it, then:\n  \
             harness approve {task_id}   or   harness reject {task_id} --to architect --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::AcceptResult) => format!(
            "The final review passed; the task asks you to accept the result yourself. \
             See the architect's notes.md for what to check, then:\n  \
             harness approve {task_id}   or   harness reject {task_id} --to <role> --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => {
            help_request_text(*role, task_id, suggested)
        }
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => format!(
            "The round limit is reached. Decide who continues:\n  \
             harness approve {task_id} --to <role|done> --notes \"...\""
        ),
        StopReason::WaitingForHuman(WaitReason::StoppedByUser(role)) => {
            stopped_by_user_text(*role, task_id)
        }
        StopReason::UsageLimitReached(role) => format!(
            "The subscription limit stopped the {role}. Its unfinished work is kept and comes \
             back when you run again later:\n  harness run {task_id}"
        ),
        StopReason::TimedOut(role) => format!(
            "The time limit (agent_timeout_minutes in harness.toml) stopped the {role}. Its \
             unfinished work stays in the folder for the next run. If the step needs more time, raise \
             the limit and commit harness.toml (any other uncommitted change stops the run), \
             then:\n  harness run {task_id}"
        ),
        StopReason::RoleFailed { role, problem } => {
            format!("The {role} failed twice. Last problem:\n{problem}")
        }
        StopReason::StepLimitReached => "Stopped after too many steps in one run.".into(),
        StopReason::DirtyWorkingTree(files) => format!(
            "The project has uncommitted changes. Commit or remove them first:\n  {}",
            files.join("\n  ")
        ),
        StopReason::ForbiddenChanges { role, files } => format!(
            "The {role} changed files it may not touch (left uncommitted for you to check):\n  {}",
            files.join("\n  ")
        ),
        StopReason::TooLarge { role, files } => format!(
            "The {role} made files too big to commit (left uncommitted). Usually this is build \
             output: add it to .gitignore or delete it, commit or remove the rest, then run again:\n  {}",
            files
                .iter()
                .map(|(path, bytes)| format!("{path} ({} MB)", bytes.div_ceil(1024 * 1024)))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
        StopReason::AgentCommitted(role) => format!(
            "While the {role} was working, a new git commit appeared or the git branches \
             changed. The role may have done it, or someone else working in this repository \
             (you, another program, a git command). Before running again, check with \
             `git log`, `git branch` and `git status` that the commits and branches are the \
             ones you expect and that the task's branch is checked out."
        ),
        StopReason::ProtectedFilesChanged { role, files, log } => format!(
            "The {role} changed files no role may change (agent instructions and settings, \
             the harness's own files). They are put back; what it wrote is in {}. Its other \
             changes are left uncommitted for you to check:\n  {}",
            log.display(),
            files.join("\n  ")
        ),
        StopReason::HiddenChanges {
            role,
            put_back,
            reported,
            not_put_back,
            config_put_back,
            git,
        } => {
            let mut text = hidden_changes_text(*role, put_back, reported, not_put_back);
            text.push_str(&also_changed_text(*config_put_back, git));
            text
        }
        StopReason::GitConfigChanged(role) => format!(
            "While the {role} was working, the project's git settings (.git/config) \
             changed. The role may have done it, or someone else working in this repository \
             (you, another program, a git command such as `git push -u`). The old settings \
             are back: if the change was yours, make it again after the role. Before running \
             again, check the project changes, which are left uncommitted."
        ),
    }
}

/// What to say about a task the user stopped, with `role` the one that continues.
fn stopped_by_user_text(role: Role, task_id: &str) -> String {
    format!(
        "You stopped {task_id}; the {role} continues next. Its unfinished changes, if any, \
         stay in the folder for it. Continue with:\n  harness run {task_id}\n\
         or give it a note first:\n  harness approve {task_id} --notes \"...\"\n\
         A note to another role (--to) needs those changes committed or removed first."
    )
}

/// What `StopReason::HiddenChanges` says: git's files that were put back or
/// could not be, and files outside the project the user should look at.
fn hidden_changes_text(
    role: Role,
    put_back: &[PathBuf],
    reported: &[PathBuf],
    not_put_back: &[PathBuf],
) -> String {
    // The names come from the agent, which could hide terminal commands in them.
    let list = |files: &[PathBuf]| -> String {
        let lines: Vec<String> = files
            .iter()
            .map(|f| harness_core::text::safe(&f.display().to_string()))
            .collect();
        format!("\n  {}", lines.join("\n  "))
    };
    let mut text = format!(
        "While the {role} was working, files that git does not show changed. The role \
         may have done it, or someone else working in this repository or on this computer."
    );
    if !put_back.is_empty() {
        text.push_str("\nGit's own files (hooks, info/), now put back as they were:");
        text.push_str(&list(put_back));
    }
    if !not_put_back.is_empty() {
        text.push_str(
            "\nGit's own files that could NOT be put back (removed where possible); \
             fix them by hand before running any git command, and check that the commits, \
             branches and .git/config are the ones you expect:",
        );
        text.push_str(&list(not_put_back));
    }
    if !reported.is_empty() {
        text.push_str(
            "\nFiles outside the project, NOT put back; check them yourself before \
             running again (if you changed them during the run, all is well):",
        );
        text.push_str(&list(reported));
    }
    text.push_str("\nThe project changes are left uncommitted for you to check.");
    text
}

/// What else the tripwire's stop message says: that `.git/config` was put
/// back, and what git said about the commits and branches.
fn also_changed_text(config_put_back: bool, git: &GitAfterRole) -> String {
    let mut text = String::new();
    if config_put_back {
        text.push_str(
            "\nThe project's git settings (.git/config) changed too; the old ones are back: \
             if the change was yours, make it again after the role.",
        );
    }
    let git_text = match git {
        GitAfterRole::Same | GitAfterRole::NotChecked => String::new(),
        GitAfterRole::Changed => "\nA new git commit appeared or the git branches changed too. \
             Check with `git log`, `git branch` and `git status` that the commits and branches \
             are the ones you expect and that the task's branch is checked out."
            .to_string(),
        // Git's message can hold names the agent chose.
        GitAfterRole::Failed(error) => format!(
            "\nGit could not run after the role ({}); a changed file above may be the \
             cause. Once git works again, check with `git log` and `git branch` that the \
             commits and branches are the ones you expect.",
            harness_core::text::safe(error)
        ),
    };
    text + &git_text
}

/// `harness stop`: asks the harness running the roles of `task_id` to stop
/// (at once with `now`, else once the running role has finished); that
/// harness says so in its own window.
pub(crate) fn stop(project: &Path, task_id: &str, now: bool) -> Result<()> {
    let repo = open_repo(project)?;
    let when = if now {
        StopWhen::Now
    } else {
        StopWhen::AfterStep
    };
    run_lock::ask_to_stop(&repo, task_id, when)?;
    if now {
        println!("Asked {task_id} to stop now. Continue it later with: harness run {task_id}");
    } else {
        println!(
            "Asked {task_id} to stop once the running role has finished. \
             Continue it later with: harness run {task_id}"
        );
    }
    Ok(())
}

/// `harness status`: the task's round and stage, and one line per handoff.
pub(crate) fn status(project: &Path, task_id: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let (store, state) = open_task(&repo, task_id)?;
    println!(
        "{task_id}: round {} of {}, {:?}",
        state.round, state.max_rounds, state.stage
    );
    if let Stage::WaitingForHuman(WaitReason::StoppedByUser(role)) = state.stage {
        println!("  stopped by the user; `harness run {task_id}` continues the {role}");
    }
    if let Some(page) = state.pull_request.as_ref().and_then(|p| p.page()) {
        println!("  pull request: {page}");
    }
    // The step's folder comes first: `harness reject --restore` takes it.
    for step in store.steps()? {
        let handoff = &step.handoff;
        println!(
            "  {} {:?} {:?} -> {:?}: {}",
            step_name(&store, &step.dir),
            handoff.role,
            handoff.verdict,
            handoff.next_role,
            handoff.summary
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use harness_core::config::projects;
    use harness_core::config::CONFIG_FILE;

    use harness_core::task::handoff::{NextStep, Verdict};

    use super::*;
    use crate::decide::{decide, Decision};

    /// `harness approve` with `--to` and `--notes`.
    fn approve(to: Option<NextStep>, notes: &str) -> Decision<'_> {
        Decision {
            verdict: Verdict::Approved,
            to,
            notes,
            restore: None,
        }
    }

    #[test]
    fn a_decision_lands_in_the_task_folder_and_the_project_folder_stays() {
        let dir = tempfile::tempdir().unwrap();
        // In a subfolder, so the tasks' folders (`app.tasks`) are inside `dir` too.
        let project = &dir.path().join("app");
        fs::create_dir(project).unwrap();
        projects::init(project).unwrap();
        let config = project.join(HARNESS_DIR).join(CONFIG_FILE);
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, format!("{text}\n[github]\nrepo = \"lisa/app\"\n")).unwrap();
        let repo = open_repo(project).unwrap();
        repo.commit_all("on GitHub").unwrap();
        let base = repo.current_branch().unwrap();
        let base_commit = repo.head().unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        // The architect is done and the design waits for the user.
        let task = branch::enter(&repo, "task-001").unwrap();
        assert_ne!(task.root(), repo.root());
        let (store, mut state) = open_task(&repo, "task-001").unwrap();
        state.stage = Stage::WaitingForHuman(WaitReason::ApproveDesign);
        let json = serde_json::to_string(&state).unwrap();
        fs::write(store.dir().join("state.json"), json).unwrap();
        task.commit_all("design ready").unwrap();

        decide(project, "task-001", approve(None, "ok")).unwrap();

        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        assert_eq!(task.changed_files().unwrap(), Vec::<String>::new());
        assert_eq!(
            task.current_branch().unwrap(),
            Some(branch::task_branch("task-001"))
        );
        // The project folder never left its branch and has nothing of the task.
        assert_eq!(repo.current_branch().unwrap(), base);
        assert_eq!(repo.head().unwrap(), base_commit);
        assert!(!repo.runs_dir().join("task-001").exists());
    }

    /// Saves `next` as where `task-001` stands, with a commit.
    fn set_stage(repo: &Repo, next: Stage) {
        let (store, mut state) = open_task(repo, "task-001").unwrap();
        state.stage = next;
        let json = serde_json::to_string(&state).unwrap();
        fs::write(store.dir().join("state.json"), json).unwrap();
        repo.commit_all("stage").unwrap();
    }

    #[test]
    fn reject_with_restore_brings_back_the_code_of_a_named_step() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        projects::init(project).unwrap();
        let repo = open_repo(project).unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        set_stage(&repo, Stage::WaitingForHuman(WaitReason::ApproveDesign));
        decide(project, "task-001", approve(None, "ok")).unwrap();
        // The developer's code, then the result waits for the user.
        fs::write(project.join("parser.rs"), "fn parse() {}").unwrap();
        set_stage(&repo, Stage::WaitingForHuman(WaitReason::AcceptResult));
        let reject = |restore| Decision {
            verdict: Verdict::Rejected,
            to: Some(NextStep::To(Role::Developer)),
            notes: "Again",
            restore: Some(restore),
        };

        let error = decide(project, "task-001", reject("round-09/01-tester")).unwrap_err();
        decide(project, "task-001", reject("round-01/01-human")).unwrap();

        assert!(
            error
                .to_string()
                .contains("its steps are: round-01/01-human"),
            "{error}"
        );
        assert!(!project.join("parser.rs").exists());
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn hidden_changes_say_what_was_put_back_and_what_the_user_must_check() {
        let text = hidden_changes_text(
            Role::Developer,
            &[PathBuf::from("/p/.git/hooks/pre-commit")],
            &[PathBuf::from("/home/lisa/.bashrc")],
            &[],
        );

        assert!(text.contains("put back as they were:\n  /p/.git/hooks/pre-commit"));
        assert!(text.contains("NOT put back"));
        assert!(text.contains("\n  /home/lisa/.bashrc"));
        let only_hooks = hidden_changes_text(Role::Tester, &[PathBuf::from("h")], &[], &[]);
        assert!(!only_hooks.contains("outside the project"));
        assert!(!only_hooks.contains("could NOT"));
    }

    #[test]
    fn the_tripwire_message_also_tells_about_the_settings_and_git() {
        assert_eq!(also_changed_text(false, &GitAfterRole::Same), "");

        let config = also_changed_text(true, &GitAfterRole::Same);
        let committed = also_changed_text(false, &GitAfterRole::Changed);
        let failed = also_changed_text(false, &GitAfterRole::Failed("bad\u{1b}[2J".into()));

        assert!(config.contains(".git/config) changed too"), "{config}");
        assert!(committed.contains("`git log`"), "{committed}");
        assert!(
            failed.contains("Git could not run") && failed.contains("bad"),
            "{failed}"
        );
        assert!(!failed.contains('\u{1b}'), "{failed}");
    }

    #[test]
    fn hooks_not_put_back_are_named_without_terminal_commands() {
        let stop = StopReason::HiddenChanges {
            role: Role::Developer,
            put_back: Vec::new(),
            reported: Vec::new(),
            not_put_back: vec![PathBuf::from("hooks/pre-commit\u{1b}[2J")],
            config_put_back: false,
            git: GitAfterRole::NotChecked,
        };

        let text = explain(&stop, "task-001", None);

        assert!(text.contains("could NOT be put back"), "{text}");
        assert!(text.contains("hooks/pre-commit"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
    }

    #[test]
    fn git_stops_say_what_changed_without_blaming_the_role() {
        // Another program or the user's own git command can make these changes
        // while a role runs, so the message must not say the role did it.
        let stops = [
            StopReason::GitConfigChanged(Role::Architect),
            StopReason::AgentCommitted(Role::Architect),
            StopReason::HiddenChanges {
                role: Role::Architect,
                put_back: vec![PathBuf::from("h")],
                reported: Vec::new(),
                not_put_back: Vec::new(),
                config_put_back: false,
                git: GitAfterRole::Same,
            },
        ];

        for stop in &stops {
            let text = explain(stop, "task-001", None);
            assert!(
                text.starts_with("While the architect was working"),
                "{text}"
            );
            assert!(!text.contains("The architect changed"), "{text}");
            assert!(!text.contains("The architect made"), "{text}");
            assert!(
                text.contains("or someone else working in this repository"),
                "{text}"
            );
            assert!(!text.contains("The role's"), "{text}");
            assert!(text.contains("check"), "{text}");
        }
    }

    #[test]
    fn a_help_request_names_the_suggested_role_and_how_to_answer_it() {
        let stop = StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester));
        let answer = AnswerTo {
            role: Role::Developer,
            reason: "the parser\nmust \u{1b}[2Jchange".into(),
        };

        let text = explain(&stop, "task-001", Some(&answer));

        assert!(
            text.contains(
                "It suggests that the developer gets your answer: the parser must change\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("harness approve task-001 --to developer --notes \"...\""),
            "{text}"
        );
        assert!(text.contains("without --to, the tester gets it"), "{text}");
    }

    #[test]
    fn a_help_request_without_a_suggestion_answers_the_asking_role() {
        let stop = StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester));

        let text = explain(&stop, "task-001", None);

        assert!(text.contains("harness approve task-001 --notes"), "{text}");
        assert!(!text.contains("suggests"), "{text}");
    }

    #[test]
    fn a_role_that_suggests_itself_gets_no_note_about_another_role() {
        let stop = StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester));
        let answer = AnswerTo {
            role: Role::Tester,
            reason: "only a decision".into(),
        };

        let text = explain(&stop, "task-001", Some(&answer));

        assert!(text.contains("--to tester --notes"), "{text}");
        assert!(!text.contains("without --to"), "{text}");
    }

    #[test]
    fn the_cli_takes_the_suggestion_from_the_last_question_only() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        projects::init(project).unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        let repo = open_repo(project).unwrap();
        let (store, mut state) = open_task(&repo, "task-001").unwrap();
        let asking = StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Architect));
        assert_eq!(suggestion(&store, &state, &asking), None);
        let asked = harness_core::task::handoff::Handoff::from_json(
            r#"{"schema_version": 1, "task_id": "task-001", "round": 1,
                "role": "architect", "verdict": "needs_human", "next_role": "human",
                "summary": "JSON or TOML?",
                "answer_to": {"role": "developer", "reason": "code"}}"#,
        )
        .unwrap();
        store.record(&mut state, &asked, "notes").unwrap();

        let found = suggestion(&store, &state, &asking);

        assert_eq!(found.map(|a| a.role), Some(Role::Developer));
        assert_eq!(suggestion(&store, &state, &StopReason::Done), None);
    }

    #[test]
    fn a_stopped_task_says_how_to_continue_or_give_a_note() {
        let stop = StopReason::WaitingForHuman(WaitReason::StoppedByUser(Role::Developer));

        let text = explain(&stop, "task-001", None);

        assert!(text.contains("the developer continues next"), "{text}");
        assert!(text.contains("harness run task-001"), "{text}");
        assert!(text.contains("harness approve task-001 --notes"), "{text}");
    }

    #[test]
    fn approve_without_to_accepts_a_result_left_for_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        projects::init(project).unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        let repo = open_repo(project).unwrap();
        let (store, mut state) = open_task(&repo, "task-001").unwrap();
        state.stage = Stage::WaitingForHuman(WaitReason::AcceptResult);
        fs::write(store.state_file(), serde_json::to_string(&state).unwrap()).unwrap();
        repo.commit_all("waiting for acceptance").unwrap();

        decide(project, "task-001", approve(None, "Checked live")).unwrap();

        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Done);
    }

    #[test]
    fn approve_without_to_sends_a_stopped_task_back_to_its_role() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        projects::init(project).unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        let repo = open_repo(project).unwrap();
        let (store, mut state) = open_task(&repo, "task-001").unwrap();
        state.stage = Stage::WaitingForHuman(WaitReason::StoppedByUser(Role::Developer));
        fs::write(store.state_file(), serde_json::to_string(&state).unwrap()).unwrap();
        // «Stop now» left the developer's half-done file, recorded.
        let record = r#"{"round":1,"role":"developer","files":[["half.rs",null]]}"#;
        fs::write(store.dir().join("unfinished.json"), record).unwrap();
        repo.commit_all("stopped").unwrap();
        fs::write(project.join("half.rs"), "fn half() {}").unwrap();

        // Rejecting would start a new round and strand the unfinished work.
        let reject = Decision {
            verdict: Verdict::Rejected,
            to: Some(NextStep::To(Role::Developer)),
            notes: "No",
            restore: None,
        };
        let error = decide(project, "task-001", reject).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("harness approve task-001 --notes"),
            "{error}"
        );

        decide(project, "task-001", approve(None, "Use serde")).unwrap();

        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        assert_eq!(state.round, 1);
    }
}
