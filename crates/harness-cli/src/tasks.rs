//! `harness task new`, `run`, `approve`, `reject` and `status`: moving a task
//! through the roles.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_agents::build::build_team;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::skills::Skills;
use harness_core::task::branch;
use harness_core::task::handoff::{AnswerTo, NextStep, Role, Verdict, MAX_REASON_CHARS};
use harness_core::task::orchestrator::{self, HumanDecision, Project, SameText, StopReason};
use harness_core::task::stop_request::StopRequest;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason};
use harness_platform::stop::StopSignals;

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
            "It works on its own branch: {}",
            branch::task_branch(task_id)
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
    // First the task's branch: its settings and skills are the ones to use.
    enter_branch(&repo, task_id)?;
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config = Config::load(&harness_dir)?;
    let skills = Skills::load(&harness_dir, &config)?;
    let agent = build_team(&config, repo.root())?;
    let (store, mut state) = open_task(&repo, task_id)?;
    if orchestrator::continue_stopped(&repo, &store, &mut state)? {
        println!("You stopped {task_id}; it continues now.");
    }
    println!("Running {task_id} (round {})...", state.round);
    // The command line has no stop buttons; Ctrl+C stops it (`until_stopped`).
    let no_stop_buttons = StopRequest::new();
    let project = Project {
        config: &config,
        skills: &skills,
        agent_versions: agent.versions(),
        stop: &no_stop_buttons,
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

/// Checks out the task's branch if it has one, and says so when it switched.
fn enter_branch(repo: &Repo, task_id: &str) -> Result<()> {
    if branch::enter(repo, task_id)? {
        println!(
            "Switched to the task's branch {}.",
            branch::task_branch(task_id)
        );
    }
    Ok(())
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
        StopReason::Done => "Done: security approved the work.".into(),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => format!(
            "The architect's design is ready. Read it, then:\n  \
             harness approve {task_id}   or   harness reject {task_id} --to architect --notes \"...\""
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
        } => hidden_changes_text(*role, put_back, reported),
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

/// What `StopReason::HiddenChanges` says: git's files that were put back,
/// and files outside the project the user should look at themselves.
fn hidden_changes_text(role: Role, put_back: &[PathBuf], reported: &[PathBuf]) -> String {
    let list = |files: &[PathBuf]| -> String {
        let lines: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
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

/// `harness approve` / `harness reject`: writes the user's decision as a handoff
/// from the role `human`. Without `--to`, approved work goes where the task's
/// workflow says (a design goes to the developer) and a role that asked for
/// help gets the task back.
pub(crate) fn decide(
    project: &Path,
    task_id: &str,
    verdict: Verdict,
    to: Option<NextStep>,
    notes: &str,
) -> Result<()> {
    let repo = open_repo(project)?;
    // The user's decision is a commit too: it goes on the task's branch.
    enter_branch(&repo, task_id)?;
    let (store, mut state) = open_task(&repo, task_id)?;
    // Rejecting starts a new round, and recorded unfinished work belongs to
    // this one: it could not be continued any more.
    if verdict == Verdict::Rejected && !orchestrator::unfinished_left(&repo, &store)?.is_empty() {
        bail!(
            "{task_id} has unfinished work in the folder; give your note to the role \
             that continues it with:\n  harness approve {task_id} --notes \"...\""
        );
    }
    let next = match to {
        Some(next) => next,
        None => match state.stage {
            Stage::WaitingForHuman(WaitReason::ApproveDesign) => {
                NextStep::To(state.workflow.after_approval())
            }
            Stage::WaitingForHuman(
                WaitReason::RoleAskedForHelp(role) | WaitReason::StoppedByUser(role),
            ) => NextStep::To(role),
            Stage::WaitingForHuman(WaitReason::RoundLimitReached)
            | Stage::Working(_)
            | Stage::Done => bail!("say who continues with --to <role|done>"),
        },
    };
    orchestrator::record_human_decision(
        &repo,
        &store,
        &mut state,
        HumanDecision {
            verdict,
            next,
            notes,
        },
    )?;
    println!("Saved. Next: harness run {task_id}");
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
    for handoff in store.history()? {
        println!(
            "  round {} {:?} {:?} -> {:?}: {}",
            handoff.round, handoff.role, handoff.verdict, handoff.next_role, handoff.summary
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use harness_core::config::projects;
    use harness_core::config::CONFIG_FILE;

    use super::*;

    #[test]
    fn a_decision_lands_on_the_task_branch_after_the_user_switched_away() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        projects::init(project).unwrap();
        let config = project.join(HARNESS_DIR).join(CONFIG_FILE);
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, format!("{text}\n[github]\nrepo = \"lisa/app\"\n")).unwrap();
        let repo = open_repo(project).unwrap();
        repo.commit_all("on GitHub").unwrap();
        let base = repo.current_branch().unwrap().unwrap();
        new_task(project, "task-001", "Add a parser", false).unwrap();
        // The architect is done and the design waits for the user.
        let (store, mut state) = open_task(&repo, "task-001").unwrap();
        state.stage = Stage::WaitingForHuman(WaitReason::ApproveDesign);
        let json = serde_json::to_string(&state).unwrap();
        fs::write(store.dir().join("state.json"), json).unwrap();
        repo.commit_all("design ready").unwrap();
        repo.switch_branch(&base).unwrap();
        let base_commit = repo.head().unwrap();

        decide(project, "task-001", Verdict::Approved, None, "ok").unwrap();

        assert_eq!(
            repo.current_branch().unwrap(),
            Some(branch::task_branch("task-001"))
        );
        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        repo.switch_branch(&base).unwrap();
        assert_eq!(repo.head().unwrap(), base_commit);
    }

    #[test]
    fn hidden_changes_say_what_was_put_back_and_what_the_user_must_check() {
        let text = hidden_changes_text(
            Role::Developer,
            &[PathBuf::from("/p/.git/hooks/pre-commit")],
            &[PathBuf::from("/home/lisa/.bashrc")],
        );

        assert!(text.contains("put back as they were:\n  /p/.git/hooks/pre-commit"));
        assert!(text.contains("NOT put back"));
        assert!(text.contains("\n  /home/lisa/.bashrc"));
        let only_hooks = hidden_changes_text(Role::Tester, &[PathBuf::from("h")], &[]);
        assert!(!only_hooks.contains("outside the project"));
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
        let error = decide(
            project,
            "task-001",
            Verdict::Rejected,
            Some(NextStep::To(Role::Developer)),
            "No",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("harness approve task-001 --notes"),
            "{error}"
        );

        decide(project, "task-001", Verdict::Approved, None, "Use serde").unwrap();

        let (_, state) = open_task(&repo, "task-001").unwrap();
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        assert_eq!(state.round, 1);
    }
}
