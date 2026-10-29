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
use harness_core::retro::stage_text;
use harness_core::task::branch;
use harness_core::task::handoff::{NextStep, Role, Verdict, MAX_TEXT_CHARS};
use harness_core::task::orchestrator::{self, HumanDecision};
use harness_core::task::run_lock::RunLock;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use harness_core::text::safe_line;

/// The longest notes a decision takes, the user's words included: they go
/// to the next role with the task; details belong in a file of the project.
/// The first line becomes the handoff's summary, which has its own, shorter
/// limit ([`MAX_TEXT_CHARS`]).
const MAX_NOTES_CHARS: usize = 20_000;

/// How much of the user's words goes into the summary of `finish_task`;
/// all of them are in its notes.
const WORDS_IN_SUMMARY: usize = 300;

/// The line added to every decision made through the server.
const VIA_MCP: &str = "(Sent through the harness MCP server.)";

/// Where the agent saw the task: the decision is refused when it has moved on.
#[derive(Debug, Clone, Copy)]
pub(super) struct Seen<'a> {
    /// How many steps `task_status` showed.
    pub(super) steps: usize,
    /// The stage `task_status` showed, as text (for example "waiting: approve design").
    pub(super) stage: &'a str,
}

/// What the agent asked for in `decide`.
#[derive(Debug, Clone, Copy)]
pub(super) struct Decide<'a> {
    /// The task.
    pub(super) task: &'a str,
    /// Where the agent saw it.
    pub(super) seen: Seen<'a>,
    /// The role that continues; `None` for the usual one.
    pub(super) to: Option<&'a str>,
    /// The notes for that role.
    pub(super) notes: &'a str,
}

/// A waiting task opened for a decision, under the run lock.
struct Opened {
    repo: Repo,
    store: TaskStore,
    state: TaskState,
    reason: WaitReason,
    // Held until the decision is saved.
    _lock: RunLock,
}

/// `decide`: the user's decision on a task waiting for them, except finishing it.
pub(super) fn decide(root: &Path, ask: Decide<'_>) -> Result<String, String> {
    check_notes(ask.notes)?;
    with_branch_note(root, ask.task, |repo, lock| {
        let opened = open_waiting(repo, lock, ask.task, ask.seen)?;
        record(opened, ask)
    })
}

/// The part of `decide` after the task is opened.
fn record(opened: Opened, ask: Decide<'_>) -> Result<String, String> {
    let Opened {
        repo,
        store,
        mut state,
        reason,
        _lock,
    } = opened;
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
    // As `harness reject`: a new round could not continue that work any more.
    let unfinished = orchestrator::unfinished_left(&repo, &store).map_err(|e| e.to_string())?;
    if rejected && !unfinished.is_empty() {
        return Err(format!(
            "{} has unfinished work of a stopped role in the folder ({}); sending the \
             work back would leave it behind. The user commits or removes it first.",
            ask.task,
            unfinished.join(", ")
        ));
    }
    if reason == WaitReason::RoundLimitReached {
        // The same as the TUI's «... · 5 more rounds». Saved with the
        // decision's commit; the notes were checked before, so it fits.
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

/// `finish_task`: accepts the result and finishes the task (whenever it
/// waits for the user, as the TUI's «Approve and finish»), with the user's
/// own words: all of them in the notes, the start in the summary.
pub(super) fn finish_task(
    root: &Path,
    task: &str,
    seen: Seen<'_>,
    user_words: &str,
) -> Result<String, String> {
    let words = user_words.trim();
    if words.is_empty() {
        return Err("finishing needs the user's own words, as they wrote them".into());
    }
    check_notes(words)?;
    with_branch_note(root, task, |repo, lock| {
        let Opened {
            repo,
            store,
            mut state,
            _lock,
            ..
        } = open_waiting(repo, lock, task, seen)?;
        let summary = safe_line(words, WORDS_IN_SUMMARY);
        let notes = format!(
            "Accepted by the user: \"{summary}\"\n\n\
             The user's words, as they wrote them:\n\n{words}\n\n{VIA_MCP}"
        );
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
    })
}

/// Holds the run lock and goes to the task's branch, then runs `work`; the
/// answer, good or not, says when the project folder moved to that branch.
fn with_branch_note(
    root: &Path,
    task: &str,
    work: impl FnOnce(Repo, RunLock) -> Result<String, String>,
) -> Result<String, String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let mut lock = RunLock::hold(&repo).map_err(|e| e.to_string())?;
    lock.set_task(task).map_err(|e| e.to_string())?;
    // The decision is a commit too: it goes on the task's branch.
    let switched = branch::enter(&repo, task).map_err(|e| e.to_string())?;
    let note = |text: String| {
        if switched {
            format!(
                "{text}\n(The project folder is now on the task's branch {}.)",
                branch::task_branch(task)
            )
        } else {
            text
        }
    };
    work(repo, lock).map(note).map_err(note)
}

/// Opens the task if it waits for the user where the agent saw it.
fn open_waiting(repo: Repo, lock: RunLock, task: &str, seen: Seen<'_>) -> Result<Opened, String> {
    let (store, state) = TaskStore::open(&repo.runs_dir(), task).map_err(|e| e.to_string())?;
    let steps = store.steps().map_err(|e| e.to_string())?.len();
    let stage_now = stage_text(state.stage);
    if steps != seen.steps || stage_now != seen.stage {
        return Err(format!(
            "{task} has moved on: it has {steps} steps and is \"{stage_now}\" now, you saw \
             {} steps and \"{}\". Read task_status again and decide on what is there now.",
            seen.steps,
            safe_line(seen.stage, 100)
        ));
    }
    let Stage::WaitingForHuman(reason) = state.stage else {
        return Err(format!("{task} does not wait for a decision ({stage_now})"));
    };
    Ok(Opened {
        repo,
        store,
        state,
        reason,
        _lock: lock,
    })
}

/// Who continues when the agent does not say: the developer after a design,
/// the role a question names in `answer_to`, the stopped role. Elsewhere the
/// agent must say: the TUI's guess there depends on more than the stage.
fn usual_role(reason: WaitReason, store: &TaskStore, state: &TaskState) -> Result<Role, String> {
    let give_to = |why: &str| {
        Err(format!(
            "{}: {why}; give `to` (one of: {})",
            state.task_id,
            role_names(state)
        ))
    };
    match reason {
        WaitReason::ApproveDesign => Ok(state.workflow.after_approval()),
        WaitReason::StoppedByUser(role) => Ok(role),
        WaitReason::RoleAskedForHelp(asked) => {
            let history = store.history().map_err(|e| e.to_string())?;
            let suggested = history
                .last()
                .and_then(|last| last.suggested_answer(state.workflow.scenario()))
                .map(|answer| answer.role);
            match suggested {
                Some(role) => Ok(role),
                None => give_to(&format!("the {asked} did not suggest who gets the answer")),
            }
        }
        WaitReason::RoundLimitReached => give_to("say who continues with 5 more rounds"),
        WaitReason::AcceptResult => {
            give_to("say who gets the result back, or finish the task with finish_task")
        }
    }
}

/// A role of the task's workflow, by name; not `human`.
fn parse_role(name: &str, state: &TaskState) -> Result<Role, String> {
    match name.parse::<Role>() {
        Ok(role) if state.workflow.scenario().roles().contains(&role) => Ok(role),
        _ => Err(format!(
            "{:?} is not a role of this task; use one of: {}",
            safe_line(name, 40),
            role_names(state)
        )),
    }
}

/// The roles of the task's workflow, for messages.
fn role_names(state: &TaskState) -> String {
    let roles = state.workflow.scenario().roles();
    roles
        .iter()
        .map(|role| role.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refuses notes too long for a decision, or whose first line (the summary)
/// is longer than a handoff's summary may be.
fn check_notes(notes: &str) -> Result<(), String> {
    if notes.chars().count() > MAX_NOTES_CHARS {
        return Err(format!(
            "the notes are longer than {MAX_NOTES_CHARS} characters; put the details in \
             a file of the project and refer to it"
        ));
    }
    let first_line = notes.trim().lines().next().unwrap_or_default();
    if first_line.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "the first line of the notes (the summary) is longer than {MAX_TEXT_CHARS} \
             characters; start with a short line and put the rest below it"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use harness_core::git::HARNESS_DIR;
    use harness_core::task::handoff::{AnswerTo, Handoff, SCHEMA_VERSION};

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

    /// Moves `task-001` to `stage` and commits it.
    fn move_to(repo: &Repo, stage: Stage) {
        let (store, mut state_now) = state(repo);
        store.save_stage(&mut state_now, stage).unwrap();
        repo.commit_all("stage").unwrap();
    }

    /// Where the agent saw `task-001`: `steps` steps at stage `stage`.
    fn seen(steps: usize, stage: &str) -> Seen<'_> {
        Seen { steps, stage }
    }

    const DESIGN: &str = "waiting: approve design";

    fn ask<'a>(seen: Seen<'a>, to: Option<&'a str>, notes: &'a str) -> Decide<'a> {
        Decide {
            task: "task-001",
            seen,
            to,
            notes,
        }
    }

    #[test]
    fn approving_the_design_sends_it_to_the_developer_marked_as_through_mcp() {
        let (dir, repo) = design_waiting();

        let answer = decide(dir.path(), ask(seen(1, DESIGN), None, "Looks good")).unwrap();

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

        decide(
            dir.path(),
            ask(seen(1, DESIGN), Some("architect"), "Use serde"),
        )
        .unwrap();

        let steps = state(&repo).0.steps().unwrap();
        assert_eq!(steps[1].handoff.verdict, Verdict::Rejected);
    }

    #[test]
    fn a_decision_on_a_task_that_moved_on_is_refused() {
        let (dir, repo) = design_waiting();

        let fewer_steps = decide(dir.path(), ask(seen(0, DESIGN), None, "Ok")).unwrap_err();
        let other_stage = seen(1, "waiting: accept the result");
        let wrong_stage = decide(dir.path(), ask(other_stage, None, "Ok")).unwrap_err();

        assert!(fewer_steps.contains("moved on"), "{fewer_steps}");
        assert!(wrong_stage.contains("moved on"), "{wrong_stage}");
        assert_eq!(state(&repo).0.steps().unwrap().len(), 1);
    }

    #[test]
    fn wrong_roles_and_tasks_that_do_not_wait_are_refused() {
        let (dir, _repo) = design_waiting();
        let human = decide(dir.path(), ask(seen(1, DESIGN), Some("human"), "x")).unwrap_err();
        assert!(human.contains("not a role of this task"), "{human}");

        decide(dir.path(), ask(seen(1, DESIGN), None, "Looks good")).unwrap();
        let working = seen(2, "working: developer");
        let working = decide(dir.path(), ask(working, None, "again")).unwrap_err();
        assert!(
            working.contains("does not wait for a decision"),
            "{working}"
        );
    }

    #[test]
    fn at_the_round_limit_the_task_gets_more_rounds() {
        let (dir, repo) = design_waiting();
        move_to(&repo, Stage::WaitingForHuman(WaitReason::RoundLimitReached));
        let limit = seen(1, "waiting: round limit reached");
        let no_role = decide(dir.path(), ask(limit, None, "Go on")).unwrap_err();
        assert!(no_role.contains("give `to`"), "{no_role}");

        decide(dir.path(), ask(limit, Some("developer"), "Go on")).unwrap();

        assert_eq!(state(&repo).1.max_rounds, 10);
    }

    #[test]
    fn a_too_long_summary_is_refused_before_anything_changes() {
        let (dir, repo) = design_waiting();
        move_to(&repo, Stage::WaitingForHuman(WaitReason::RoundLimitReached));
        let long_line = "x".repeat(MAX_TEXT_CHARS + 1);
        let limit = seen(1, "waiting: round limit reached");

        let refused = decide(dir.path(), ask(limit, Some("developer"), &long_line)).unwrap_err();

        assert!(refused.contains("first line"), "{refused}");
        assert_eq!(state(&repo).1.max_rounds, 5);
    }

    /// The developer's question, with `answer_to` naming `answer_to`, waits for the user.
    fn question_waiting(answer_to: Option<Role>) -> (tempfile::TempDir, Repo) {
        let (dir, repo) = design_waiting();
        move_to(&repo, Stage::Working(Role::Developer));
        let (store, mut state_now) = state(&repo);
        let handoff = Handoff {
            schema_version: SCHEMA_VERSION,
            task_id: "task-001".into(),
            round: 1,
            role: Role::Developer,
            verdict: Verdict::NeedsHuman,
            next_role: NextStep::To(Role::Human),
            summary: "Which format?".into(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
            answer_to: answer_to.map(|role| AnswerTo {
                role,
                reason: "It is about the design".into(),
            }),
        };
        store.record(&mut state_now, &handoff, "").unwrap();
        repo.commit_all("question").unwrap();
        move_to(
            &repo,
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Developer)),
        );
        (dir, repo)
    }

    const QUESTION: &str = "waiting: developer asked for help";

    #[test]
    fn an_answer_goes_to_the_role_the_question_suggests() {
        let (dir, repo) = question_waiting(Some(Role::Architect));

        decide(dir.path(), ask(seen(2, QUESTION), None, "CSV")).unwrap();

        assert_eq!(state(&repo).1.stage, Stage::Working(Role::Architect));
    }

    #[test]
    fn an_answer_without_a_suggestion_needs_to() {
        let (dir, repo) = question_waiting(None);

        let refused = decide(dir.path(), ask(seen(2, QUESTION), None, "CSV")).unwrap_err();

        assert!(refused.contains("did not suggest"), "{refused}");
        assert_eq!(state(&repo).0.steps().unwrap().len(), 2);
    }

    #[test]
    fn a_stopped_task_goes_on_with_the_stopped_role() {
        let (dir, repo) = design_waiting();
        move_to(
            &repo,
            Stage::WaitingForHuman(WaitReason::StoppedByUser(Role::Tester)),
        );
        let stopped = seen(1, "waiting: stopped by the user, tester next");

        decide(dir.path(), ask(stopped, None, "Go on")).unwrap();

        assert_eq!(state(&repo).1.stage, Stage::Working(Role::Tester));
    }

    #[test]
    fn finishing_needs_the_user_s_words_and_keeps_them_exactly() {
        let (dir, repo) = design_waiting();
        move_to(&repo, Stage::WaitingForHuman(WaitReason::AcceptResult));
        let accept = seen(1, "waiting: accept the result");
        let no_words = finish_task(dir.path(), "task-001", accept, "  ").unwrap_err();
        assert!(no_words.contains("own words"), "{no_words}");

        let words = "Принимаю task-001,\nвсё работает";
        finish_task(dir.path(), "task-001", accept, words).unwrap();

        let (store, state_after) = state(&repo);
        assert_eq!(state_after.stage, Stage::Done);
        let step = &store.steps().unwrap()[1];
        assert_eq!(
            step.handoff.summary,
            "Accepted by the user: \"Принимаю task-001, всё работает\""
        );
        assert!(step.notes.contains(words), "{}", step.notes);
        assert!(step.notes.contains(VIA_MCP), "{}", step.notes);
    }

    #[test]
    fn a_task_can_be_finished_at_any_waiting_stage() {
        let (dir, repo) = design_waiting();

        finish_task(dir.path(), "task-001", seen(1, DESIGN), "Good enough, done").unwrap();

        assert_eq!(state(&repo).1.stage, Stage::Done);
    }
}
