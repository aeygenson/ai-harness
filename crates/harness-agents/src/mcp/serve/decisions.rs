//! The server's tools for the user's decisions on a waiting task: `decide`
//! (approve a design, answer a role, send work back, more rounds) and
//! `finish_task` (accept the result).
//!
//! They save the decision like `harness approve` / `harness reject` and the
//! TUI's «To» box: a handoff with role `human`, committed. The roles do not
//! start by themselves; the agent calls `start_run` next. Every decision says
//! in its notes that it came through MCP, and finishing a task records the
//! user's own words, so the history shows who decided what.

use std::path::Path;

use harness_core::git::Repo;
use harness_core::task::branch;
use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{self, HumanDecision};
use harness_core::task::run_lock::RunLock;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};

/// The longest notes a decision takes: they go to the next role, which
/// reads them with the task; details belong in a file of the project.
const MAX_NOTES_CHARS: usize = 20_000;

/// The line added to every decision made through the server.
const VIA_MCP: &str = "(Sent through the harness MCP server.)";

/// What the agent asked for in `decide`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Decide<'a> {
    /// The task.
    pub(super) task: &'a str,
    /// How many steps the agent saw in `task_status`; the decision is
    /// refused when the task has moved on since.
    pub(super) seen_steps: usize,
    /// The role that continues; `None` for the usual one.
    pub(super) to: Option<&'a str>,
    /// The notes for that role.
    pub(super) notes: &'a str,
}

/// `decide`: the user's decision on a task waiting for them, except finishing it.
pub(super) fn decide(root: &Path, ask: Decide<'_>) -> Result<String, String> {
    check_notes(ask.notes)?;
    let (repo, _lock, store, mut state) = open_waiting(root, ask.task, ask.seen_steps)?;
    let Stage::WaitingForHuman(reason) = state.stage else {
        return Err(not_waiting(&state));
    };
    let role = match ask.to {
        Some(name) => parse_role(name, &state)?,
        None => usual_role(reason, &store, &state)?,
    };
    // Sending the design back to the architect rejects it, and so does
    // sending a result the user has not accepted to any role (as in the TUI).
    let rejected = match reason {
        WaitReason::ApproveDesign => role == Role::Architect,
        WaitReason::AcceptResult => true,
        WaitReason::RoleAskedForHelp(_)
        | WaitReason::RoundLimitReached
        | WaitReason::StoppedByUser(_) => false,
    };
    if rejected
        && !orchestrator::unfinished_left(&repo, &store)
            .map_err(|e| e.to_string())?
            .is_empty()
    {
        return Err(format!(
            "{} has unfinished work in the folder; give the notes to the role that \
             continues it instead",
            ask.task
        ));
    }
    if reason == WaitReason::RoundLimitReached {
        // The same as the TUI's «... · 5 more rounds».
        store
            .add_rounds(&mut state, DEFAULT_MAX_ROUNDS)
            .map_err(|e| e.to_string())?;
    }
    let notes = format!("{}\n\n{VIA_MCP}", ask.notes.trim());
    let verdict = if rejected {
        Verdict::Rejected
    } else {
        Verdict::Approved
    };
    let decision = HumanDecision {
        verdict,
        next: NextStep::To(role),
        notes: &notes,
    };
    orchestrator::record_human_decision(&repo, &store, &mut state, decision)
        .map_err(|e| e.to_string())?;
    let what = if rejected {
        "sent back to"
    } else {
        "sent on to"
    };
    Ok(format!(
        "Saved: {} {what} the {role}. Start the roles with start_run.",
        ask.task
    ))
}

/// `finish_task`: accepts the result and finishes the task, with the user's
/// own words, which are saved as the decision's summary.
pub(super) fn finish_task(
    root: &Path,
    task: &str,
    seen_steps: usize,
    user_words: &str,
) -> Result<String, String> {
    let words = user_words.trim();
    if words.is_empty() {
        return Err("finishing needs the user's own words, as they wrote them".into());
    }
    check_notes(words)?;
    let (repo, _lock, store, mut state) = open_waiting(root, task, seen_steps)?;
    let can_finish = matches!(
        state.stage,
        Stage::WaitingForHuman(
            WaitReason::AcceptResult | WaitReason::RoundLimitReached | WaitReason::StoppedByUser(_)
        )
    );
    if !can_finish {
        return Err(format!(
            "{task} cannot be finished now ({}); it finishes after the architect's final \
             review or when it waits for the user's acceptance",
            harness_core::retro::stage_text(state.stage)
        ));
    }
    // The words on one line: it is the summary of the decision.
    let one_line = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let notes = format!("Accepted by the user: \"{one_line}\"\n\n{VIA_MCP}");
    let decision = HumanDecision {
        verdict: Verdict::Approved,
        next: NextStep::Done,
        notes: &notes,
    };
    orchestrator::record_human_decision(&repo, &store, &mut state, decision)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{task} is done. In a project on GitHub, start_run sends it as a pull request."
    ))
}

/// Holds the run lock, goes to the task's branch and opens the task, if it
/// still has the `seen_steps` steps the agent saw.
fn open_waiting(
    root: &Path,
    task: &str,
    seen_steps: usize,
) -> Result<(Repo, RunLock, TaskStore, TaskState), String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let mut lock = RunLock::hold(&repo).map_err(|e| e.to_string())?;
    lock.set_task(task).map_err(|e| e.to_string())?;
    // The decision is a commit too: it goes on the task's branch.
    branch::enter(&repo, task).map_err(|e| e.to_string())?;
    let (store, state) = TaskStore::open(&repo.runs_dir(), task).map_err(|e| e.to_string())?;
    let steps = store.steps().map_err(|e| e.to_string())?.len();
    if steps != seen_steps {
        return Err(format!(
            "{task} has {steps} steps now, not {seen_steps}: it has moved on. \
             Read task_status again and decide on what is there now."
        ));
    }
    Ok((repo, lock, store, state))
}

/// Who continues when the agent does not say: the developer after a design,
/// the role a question suggests (or the one that asked), the stopped role.
fn usual_role(reason: WaitReason, store: &TaskStore, state: &TaskState) -> Result<Role, String> {
    match reason {
        WaitReason::ApproveDesign => Ok(state.workflow.after_approval()),
        WaitReason::RoleAskedForHelp(asked) => {
            let history = store.history().map_err(|e| e.to_string())?;
            let suggested = history
                .last()
                .and_then(|last| last.suggested_answer(state.workflow.scenario()))
                .map(|answer| answer.role);
            Ok(suggested.unwrap_or(asked))
        }
        WaitReason::StoppedByUser(role) => Ok(role),
        WaitReason::RoundLimitReached | WaitReason::AcceptResult => Err(format!(
            "{} waits for a choice of who continues: give `to` (or finish it with finish_task)",
            state.task_id
        )),
    }
}

/// A role of the task's workflow, by name; not `human`.
fn parse_role(name: &str, state: &TaskState) -> Result<Role, String> {
    let roles = state.workflow.scenario().roles();
    match name.parse::<Role>() {
        Ok(role) if roles.contains(&role) => Ok(role),
        _ => Err(format!(
            "{name:?} is not a role of this task; use one of: {}",
            roles
                .iter()
                .map(|role| role.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn check_notes(notes: &str) -> Result<(), String> {
    if notes.chars().count() > MAX_NOTES_CHARS {
        return Err(format!(
            "the notes are longer than {MAX_NOTES_CHARS} characters; put the details in \
             a file of the project and refer to it"
        ));
    }
    Ok(())
}

fn not_waiting(state: &TaskState) -> String {
    format!(
        "{} does not wait for a decision ({})",
        state.task_id,
        harness_core::retro::stage_text(state.stage)
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use harness_core::git::HARNESS_DIR;
    use harness_core::task::handoff::{Handoff, SCHEMA_VERSION};

    use super::*;

    /// A git project with `task-001`, where the architect's design waits for the user.
    fn design_waiting() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let harness_dir = dir.path().join(HARNESS_DIR);
        fs::create_dir_all(&harness_dir).unwrap();
        fs::write(harness_dir.join("harness.toml"), "max_rounds = 5\n").unwrap();
        repo.commit_all("settings").unwrap();
        super::super::actions::create_task(dir.path(), None, "Build a parser", false).unwrap();
        let (store, mut state) = TaskStore::open(&repo.runs_dir(), "task-001").unwrap();
        let handoff = Handoff {
            schema_version: SCHEMA_VERSION,
            task_id: "task-001".into(),
            round: 1,
            role: Role::Architect,
            verdict: Verdict::Approved,
            next_role: NextStep::To(Role::Human),
            summary: "Design is ready".into(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
            answer_to: None,
        };
        store.record(&mut state, &handoff, "See design.md").unwrap();
        repo.commit_all("design").unwrap();
        (dir, repo)
    }

    fn state(repo: &Repo) -> (TaskStore, TaskState) {
        TaskStore::open(&repo.runs_dir(), "task-001").unwrap()
    }

    fn ask<'a>(seen_steps: usize, to: Option<&'a str>, notes: &'a str) -> Decide<'a> {
        Decide {
            task: "task-001",
            seen_steps,
            to,
            notes,
        }
    }

    #[test]
    fn approving_the_design_sends_it_to_the_developer_marked_as_through_mcp() {
        let (dir, repo) = design_waiting();

        let answer = decide(dir.path(), ask(1, None, "Looks good")).unwrap();

        assert!(answer.contains("sent on to the developer"), "{answer}");
        let (store, state) = state(&repo);
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        let steps = store.steps().unwrap();
        let decision = &steps[1];
        assert_eq!(decision.handoff.role, Role::Human);
        assert_eq!(decision.handoff.verdict, Verdict::Approved);
        assert_eq!(decision.handoff.summary, "Looks good");
        assert!(decision.notes.contains(VIA_MCP), "{}", decision.notes);
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn sending_the_design_back_to_the_architect_rejects_it() {
        let (dir, repo) = design_waiting();

        decide(dir.path(), ask(1, Some("architect"), "Use serde")).unwrap();

        let steps = state(&repo).0.steps().unwrap();
        assert_eq!(steps[1].handoff.verdict, Verdict::Rejected);
    }

    #[test]
    fn a_decision_on_a_task_that_moved_on_is_refused() {
        let (dir, repo) = design_waiting();

        let stale = decide(dir.path(), ask(0, None, "Looks good")).unwrap_err();

        assert!(stale.contains("moved on"), "{stale}");
        assert_eq!(state(&repo).0.steps().unwrap().len(), 1);
    }

    #[test]
    fn wrong_roles_and_tasks_that_do_not_wait_are_refused() {
        let (dir, _repo) = design_waiting();
        let human = decide(dir.path(), ask(1, Some("human"), "x")).unwrap_err();
        assert!(human.contains("not a role of this task"), "{human}");

        decide(dir.path(), ask(1, None, "Looks good")).unwrap();
        let working = decide(dir.path(), ask(2, None, "again")).unwrap_err();
        assert!(
            working.contains("does not wait for a decision"),
            "{working}"
        );
    }

    #[test]
    fn at_the_round_limit_the_task_gets_more_rounds() {
        let (dir, repo) = design_waiting();
        let (store, mut state_now) = state(&repo);
        store
            .save_stage(
                &mut state_now,
                Stage::WaitingForHuman(WaitReason::RoundLimitReached),
            )
            .unwrap();
        repo.commit_all("limit").unwrap();
        let no_role = decide(dir.path(), ask(1, None, "Go on")).unwrap_err();
        assert!(no_role.contains("give `to`"), "{no_role}");

        decide(dir.path(), ask(1, Some("developer"), "Go on")).unwrap();

        assert_eq!(state(&repo).1.max_rounds, 10);
    }

    #[test]
    fn finishing_needs_the_user_s_words_and_a_task_ready_to_finish() {
        let (dir, repo) = design_waiting();
        let design = finish_task(dir.path(), "task-001", 1, "accept").unwrap_err();
        assert!(design.contains("cannot be finished now"), "{design}");

        let (store, mut state_now) = state(&repo);
        store
            .save_stage(
                &mut state_now,
                Stage::WaitingForHuman(WaitReason::AcceptResult),
            )
            .unwrap();
        repo.commit_all("accept").unwrap();
        let no_words = finish_task(dir.path(), "task-001", 1, "  ").unwrap_err();
        assert!(no_words.contains("own words"), "{no_words}");

        finish_task(
            dir.path(),
            "task-001",
            1,
            "Принимаю task-001,\nвсё работает",
        )
        .unwrap();

        let (store, state_after) = state(&repo);
        assert_eq!(state_after.stage, Stage::Done);
        let summary = &store.steps().unwrap()[1].handoff.summary;
        assert_eq!(
            summary,
            "Accepted by the user: \"Принимаю task-001, всё работает\""
        );
    }
}
