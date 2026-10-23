//! The state of one task and the rules for moving it forward (the state machine).
//!
//! The folder holds the rest of running a task: `branch` (a task's own git
//! branch in a project on GitHub), `handoff` (what a role hands over),
//! `scenario` (the kind of project and the task's workflow), `routes`,
//! `prompt`, `permissions`, `agent` (the adapter interface), `orchestrator`
//! (the run loop), `store` (saving tasks to disk), `facts` and `weakened`
//! (what earlier steps really changed, read from git), `pull_request`
//! (the pull request a finished task becomes on GitHub), `tripwire` (what
//! `git status` does not show), `agent_stubs` (the empty files Claude Code
//! makes when it starts, which are not a reason to stop) and `stop_request`
//! (Lisa's «Stop after this step» and «Stop now»).

pub mod agent;
pub mod agent_stubs;
pub mod branch;
pub mod facts;
pub mod handoff;
pub mod manifest;
pub mod orchestrator;
pub mod permissions;
pub mod prompt;
pub mod pull_request;
pub mod routes;
pub mod scenario;
pub mod stop_request;
pub mod store;
pub mod tripwire;
pub mod weakened;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::task::handoff::{Handoff, NextStep, Role, Verdict, SCHEMA_VERSION};
use crate::task::pull_request::PullRequest;
use crate::task::scenario::Workflow;

/// Default limit on how many rounds a task may take before Lisa must step in.
pub const DEFAULT_MAX_ROUNDS: u32 = 5;

/// Why the task is waiting for Lisa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// The architect finished a design and it needs Lisa's approval.
    ApproveDesign,
    /// A role said `needs_human`.
    RoleAskedForHelp(Role),
    /// Work was sent back too many times.
    RoundLimitReached,
    /// Lisa stopped the run («Stop now» or «Stop after this step»). The
    /// role is the one that continues when she chooses «Continue»: the
    /// stopped one, or the one that would have run next. In `state.json`:
    /// `{"waiting_for_human":{"stopped_by_lisa":"developer"}}`.
    StoppedByLisa(Role),
}

/// Where the task is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// An AI role is working (never `Role::Human`; Lisa's turn is `WaitingForHuman`).
    Working(Role),
    /// The task waits for Lisa, for the reason given.
    WaitingForHuman(WaitReason),
    /// The task is finished.
    Done,
}

/// Everything the orchestrator needs to remember about a task. Saved as `state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskState {
    /// The task's id and folder name, for example `task-001`.
    pub task_id: String,
    /// The path the task takes through the roles; a task saved before
    /// workflows existed has none in `state.json` and reads as `task`.
    #[serde(default)]
    pub workflow: Workflow,
    /// The current round, starting at 1; each rejected handoff adds one.
    pub round: u32,
    /// After this many rounds the task stops and waits for Lisa.
    pub max_rounds: u32,
    /// Who works now, or why the task waits, or that it is done.
    pub stage: Stage,
    /// The pull request the finished task became on GitHub; missing in
    /// `state.json` until there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequest>,
}

/// Why a handoff was refused. The task state is left unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// The task is done, so no more handoffs are accepted.
    TaskAlreadyDone,
    /// The handoff says it is written in a format version this harness does not know.
    UnknownSchemaVersion(u32),
    /// A text in the handoff is longer than the format allows.
    TooLong {
        /// The name of the field that is too long, for example `summary`.
        field: &'static str,
        /// The most characters that field may have.
        max_chars: usize,
    },
    /// The handoff was written by a different role than the one whose turn it is.
    WrongAuthor {
        /// The role whose turn it is.
        expected: Role,
        /// The role named in the handoff.
        got: Role,
    },
    /// The handoff names a different task.
    WrongTask {
        /// The id of this task.
        expected: String,
        /// The task id written in the handoff.
        got: String,
    },
    /// The handoff names a different round than the task is in.
    WrongRound {
        /// The round the task is in.
        expected: u32,
        /// The round written in the handoff.
        got: u32,
    },
    /// The work was rejected, but the handoff lists no issues.
    RejectedWithoutIssues,
    /// A question to Lisa suggests that a role outside the task's scenario
    /// (or Lisa herself) gets her answer (`answer_to.role`).
    AnswerToNotARole {
        /// The role named in `answer_to`.
        role: Role,
        /// The roles that may get the answer: those of the task's scenario.
        allowed: &'static [Role],
    },
    /// The routes do not allow this role, with this verdict, to send work there.
    RouteNotAllowed {
        /// The role that wrote the handoff.
        role: Role,
        /// The verdict it gave.
        verdict: Verdict,
        /// Where it wanted the work to go next.
        next: NextStep,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskAlreadyDone => write!(f, "the task is already done"),
            Self::UnknownSchemaVersion(version) => write!(
                f,
                "schema_version is {version}, but this harness knows only {SCHEMA_VERSION}"
            ),
            Self::TooLong { field, max_chars } => write!(
                f,
                "{field} is longer than {max_chars} characters; keep it short and put details in notes.md"
            ),
            Self::WrongAuthor { expected, got } => {
                write!(
                    f,
                    "expected a handoff from {expected:?}, got one from {got:?}"
                )
            }
            Self::WrongTask { expected, got } => {
                write!(f, "handoff is for task {got}, but this is task {expected}")
            }
            Self::WrongRound { expected, got } => {
                write!(
                    f,
                    "handoff is for round {got}, but the task is in round {expected}"
                )
            }
            Self::RejectedWithoutIssues => write!(f, "a rejected handoff must list its issues"),
            Self::AnswerToNotARole { role, allowed } => {
                let names: Vec<&str> = allowed.iter().map(|r| r.as_str()).collect();
                write!(
                    f,
                    "answer_to.role is {role}, but Lisa's answer can go only to a role of this \
                     task: {}",
                    names.join(", ")
                )
            }
            Self::RouteNotAllowed {
                role,
                verdict,
                next,
            } => {
                write!(
                    f,
                    "{role:?} with verdict {verdict:?} may not send work to {next:?}"
                )
            }
        }
    }
}

impl std::error::Error for TransitionError {}

impl TaskState {
    /// A new task starts in round 1 with the first role of its workflow.
    pub fn new(task_id: impl Into<String>, workflow: Workflow, max_rounds: u32) -> Self {
        Self {
            task_id: task_id.into(),
            workflow,
            round: 1,
            max_rounds,
            stage: Stage::Working(workflow.start()),
            pull_request: None,
        }
    }

    /// Gives the task `extra` more rounds than it has used so far.
    ///
    /// Counting from the rounds already used, not from the old limit, means a
    /// task that went past its limit with Lisa's help still gets all `extra`
    /// rounds. Only the limit changes; the stage stays as it is.
    pub fn add_rounds(&mut self, extra: u32) {
        let used = self.round.saturating_sub(1);
        self.max_rounds = self.max_rounds.max(used).saturating_add(extra);
    }

    /// Who must write the next handoff, or `None` if the task is done.
    pub fn expected_author(&self) -> Option<Role> {
        match self.stage {
            Stage::Working(role) => Some(role),
            Stage::WaitingForHuman(_) => Some(Role::Human),
            Stage::Done => None,
        }
    }

    /// Checks a handoff and, if it is valid, moves the task to its next stage.
    pub fn apply(&mut self, handoff: &Handoff) -> Result<(), TransitionError> {
        let expected = self
            .expected_author()
            .ok_or(TransitionError::TaskAlreadyDone)?;
        if handoff.schema_version != SCHEMA_VERSION {
            return Err(TransitionError::UnknownSchemaVersion(
                handoff.schema_version,
            ));
        }
        if let Some((field, max_chars)) = handoff.too_long_field() {
            return Err(TransitionError::TooLong { field, max_chars });
        }
        if handoff.role != expected {
            return Err(TransitionError::WrongAuthor {
                expected,
                got: handoff.role,
            });
        }
        if handoff.task_id != self.task_id {
            return Err(TransitionError::WrongTask {
                expected: self.task_id.clone(),
                got: handoff.task_id.clone(),
            });
        }
        if handoff.round != self.round {
            return Err(TransitionError::WrongRound {
                expected: self.round,
                got: handoff.round,
            });
        }
        if handoff.verdict == Verdict::Rejected && handoff.issues.is_empty() {
            return Err(TransitionError::RejectedWithoutIssues);
        }
        self.check_answer_to(handoff)?;
        if !routes::is_allowed(
            self.workflow,
            handoff.role,
            handoff.verdict,
            handoff.next_role,
        ) {
            return Err(TransitionError::RouteNotAllowed {
                role: handoff.role,
                verdict: handoff.verdict,
                next: handoff.next_role,
            });
        }

        // Sending work back starts a new round.
        if handoff.verdict == Verdict::Rejected {
            self.round += 1;
        }

        self.stage = match handoff.next_role {
            NextStep::Done => Stage::Done,
            NextStep::To(Role::Human) => Stage::WaitingForHuman(match handoff.verdict {
                Verdict::NeedsHuman => WaitReason::RoleAskedForHelp(handoff.role),
                _ => WaitReason::ApproveDesign,
            }),
            // Lisa may always continue past the limit; AI roles may not.
            NextStep::To(_) if self.round > self.max_rounds && handoff.role != Role::Human => {
                Stage::WaitingForHuman(WaitReason::RoundLimitReached)
            }
            NextStep::To(role) => Stage::Working(role),
        };
        Ok(())
    }

    /// A question to Lisa may suggest who gets her answer, but only a role of
    /// the task's scenario. With another verdict `answer_to` is ignored
    /// (see [`Handoff::suggested_answer`]).
    fn check_answer_to(&self, handoff: &Handoff) -> Result<(), TransitionError> {
        let Some(answer) = &handoff.answer_to else {
            return Ok(());
        };
        let allowed = self.workflow.scenario().roles();
        if handoff.verdict == Verdict::NeedsHuman && !allowed.contains(&answer.role) {
            return Err(TransitionError::AnswerToNotARole {
                role: answer.role,
                allowed,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::handoff::{AnswerTo, Issue, Severity};

    /// A minimal handoff for tests; rejected handoffs get one issue.
    fn handoff(role: Role, round: u32, verdict: Verdict, next: NextStep) -> Handoff {
        let issues = if verdict == Verdict::Rejected {
            vec![Issue {
                severity: Severity::High,
                location: None,
                description: "something is wrong".into(),
            }]
        } else {
            vec![]
        };
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round,
            role,
            verdict,
            next_role: next,
            summary: "test".into(),
            skills_used: vec![],
            files: vec![],
            issues,
            answer_to: None,
        }
    }

    fn new_task() -> TaskState {
        TaskState::new("task-001", Workflow::Task, DEFAULT_MAX_ROUNDS)
    }

    #[test]
    fn happy_path_goes_through_every_role_to_done() {
        let mut task = new_task();
        let steps = [
            (Role::Architect, NextStep::To(Role::Human)),
            (Role::Human, NextStep::To(Role::Developer)),
            (Role::Developer, NextStep::To(Role::Tester)),
            (Role::Tester, NextStep::To(Role::Security)),
            (Role::Security, NextStep::Done),
        ];
        for (role, next) in steps {
            task.apply(&handoff(role, 1, Verdict::Approved, next))
                .unwrap();
        }
        assert_eq!(task.stage, Stage::Done);
        assert_eq!(task.round, 1);
    }

    #[test]
    fn a_new_task_starts_with_the_first_role_of_its_workflow() {
        let task = new_task();
        assert_eq!(task.workflow, Workflow::Task);
        assert_eq!(task.stage, Stage::Working(Workflow::Task.start()));
    }

    #[test]
    fn a_state_saved_before_workflows_reads_as_the_task_workflow() {
        // Arrange: a state.json as older versions wrote it, without `workflow`.
        let mut old = serde_json::to_value(new_task()).unwrap();
        old.as_object_mut().unwrap().remove("workflow").unwrap();

        // Act
        let state: TaskState = serde_json::from_value(old).unwrap();

        // Assert
        assert_eq!(state, new_task());
    }

    #[test]
    fn architect_design_waits_for_approval() {
        let mut task = new_task();
        task.apply(&handoff(
            Role::Architect,
            1,
            Verdict::Approved,
            NextStep::To(Role::Human),
        ))
        .unwrap();
        assert_eq!(
            task.stage,
            Stage::WaitingForHuman(WaitReason::ApproveDesign)
        );
        assert_eq!(task.expected_author(), Some(Role::Human));
    }

    #[test]
    fn tester_rejection_goes_back_to_developer_in_a_new_round() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Tester);
        task.apply(&handoff(
            Role::Tester,
            1,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        ))
        .unwrap();
        assert_eq!(task.stage, Stage::Working(Role::Developer));
        assert_eq!(task.round, 2);
    }

    #[test]
    fn refuses_a_handoff_from_the_wrong_role() {
        let mut task = new_task();
        let err = task
            .apply(&handoff(
                Role::Developer,
                1,
                Verdict::Approved,
                NextStep::To(Role::Tester),
            ))
            .unwrap_err();
        assert_eq!(
            err,
            TransitionError::WrongAuthor {
                expected: Role::Architect,
                got: Role::Developer
            }
        );
        assert_eq!(task, new_task(), "state must not change on error");
    }

    #[test]
    fn refuses_a_route_that_is_not_in_the_table() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Developer);
        let err = task
            .apply(&handoff(
                Role::Developer,
                1,
                Verdict::Approved,
                NextStep::To(Role::Security),
            ))
            .unwrap_err();
        assert!(matches!(err, TransitionError::RouteNotAllowed { .. }));
    }

    #[test]
    fn refuses_a_rejection_without_issues() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Tester);
        let mut h = handoff(
            Role::Tester,
            1,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        );
        h.issues.clear();
        assert_eq!(task.apply(&h), Err(TransitionError::RejectedWithoutIssues));
    }

    #[test]
    fn refuses_a_handoff_from_an_old_round() {
        let mut task = new_task();
        task.round = 3;
        let err = task
            .apply(&handoff(
                Role::Architect,
                2,
                Verdict::Approved,
                NextStep::To(Role::Human),
            ))
            .unwrap_err();
        assert_eq!(
            err,
            TransitionError::WrongRound {
                expected: 3,
                got: 2
            }
        );
    }

    #[test]
    fn needs_human_stops_and_remembers_who_asked() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Security);
        task.apply(&handoff(
            Role::Security,
            1,
            Verdict::NeedsHuman,
            NextStep::To(Role::Human),
        ))
        .unwrap();
        assert_eq!(
            task.stage,
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Security))
        );
    }

    /// A tester's handoff with `verdict`, suggesting `role` for Lisa's answer.
    fn tester_suggesting(verdict: Verdict, next: NextStep, role: Role) -> Handoff {
        let mut asked = handoff(Role::Tester, 1, verdict, next);
        asked.answer_to = Some(AnswerTo {
            role,
            reason: "needs a change".into(),
        });
        asked
    }

    #[test]
    fn a_question_may_suggest_any_role_of_the_scenario() {
        for role in [Role::Developer, Role::Architect, Role::Tester] {
            let mut task = new_task();
            task.stage = Stage::Working(Role::Tester);
            let asked = tester_suggesting(Verdict::NeedsHuman, NextStep::To(Role::Human), role);
            task.apply(&asked).unwrap();
            assert_eq!(
                task.stage,
                Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester))
            );
        }
    }

    #[test]
    fn a_question_may_not_suggest_lisa_herself() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Tester);
        let before = task.clone();
        let asked = tester_suggesting(Verdict::NeedsHuman, NextStep::To(Role::Human), Role::Human);

        let err = task.apply(&asked).unwrap_err();

        assert!(matches!(
            err,
            TransitionError::AnswerToNotARole {
                role: Role::Human,
                ..
            }
        ));
        assert!(err
            .to_string()
            .contains("architect, developer, tester, security"));
        assert_eq!(task, before);
    }

    #[test]
    fn answer_to_with_another_verdict_is_ignored_not_refused() {
        let mut task = new_task();
        task.stage = Stage::Working(Role::Tester);
        let approved =
            tester_suggesting(Verdict::Approved, NextStep::To(Role::Security), Role::Human);
        task.apply(&approved).unwrap();
        assert_eq!(task.stage, Stage::Working(Role::Security));
    }

    #[test]
    fn too_many_rounds_stops_for_lisa() {
        let mut task = new_task();
        task.round = DEFAULT_MAX_ROUNDS;
        task.stage = Stage::Working(Role::Tester);
        task.apply(&handoff(
            Role::Tester,
            DEFAULT_MAX_ROUNDS,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        ))
        .unwrap();
        assert_eq!(
            task.stage,
            Stage::WaitingForHuman(WaitReason::RoundLimitReached)
        );

        // Lisa decides to give the developer one more try.
        let round = task.round;
        task.apply(&handoff(
            Role::Human,
            round,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        ))
        .unwrap();
        assert_eq!(task.stage, Stage::Working(Role::Developer));
    }

    #[test]
    fn more_rounds_let_the_roles_go_on_past_the_old_limit() {
        // Arrange: the tester rejected the work in the last allowed round.
        let mut task = new_task();
        task.round = DEFAULT_MAX_ROUNDS;
        task.stage = Stage::Working(Role::Tester);
        task.apply(&handoff(
            Role::Tester,
            DEFAULT_MAX_ROUNDS,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        ))
        .unwrap();
        assert_eq!(
            task.stage,
            Stage::WaitingForHuman(WaitReason::RoundLimitReached)
        );

        // Act: Lisa adds rounds and sends the work back to the developer.
        task.add_rounds(5);
        let round = task.round;
        task.apply(&handoff(
            Role::Human,
            round,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        ))
        .unwrap();
        task.apply(&handoff(
            Role::Developer,
            round,
            Verdict::Approved,
            NextStep::To(Role::Tester),
        ))
        .unwrap();

        // Assert: the developer's handoff goes on to the tester.
        assert_eq!(task.max_rounds, DEFAULT_MAX_ROUNDS + 5);
        assert_eq!(task.stage, Stage::Working(Role::Tester));
    }

    #[test]
    fn more_rounds_count_from_the_rounds_already_used() {
        // A task Lisa pushed well past its limit still gets all the new rounds.
        let mut task = new_task();
        task.round = 9;
        task.add_rounds(5);
        assert_eq!(task.max_rounds, 13);

        // A task far from its limit keeps the rounds it had, plus the new ones.
        let mut task = new_task();
        task.add_rounds(5);
        assert_eq!(task.max_rounds, DEFAULT_MAX_ROUNDS + 5);
    }

    #[test]
    fn nothing_is_accepted_after_done() {
        let mut task = new_task();
        task.stage = Stage::Done;
        let err = task
            .apply(&handoff(
                Role::Security,
                1,
                Verdict::Approved,
                NextStep::Done,
            ))
            .unwrap_err();
        assert_eq!(err, TransitionError::TaskAlreadyDone);
    }

    #[test]
    fn state_survives_a_round_trip_through_json() {
        let mut task = new_task();
        task.stage = Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester));
        let json = serde_json::to_string_pretty(&task).unwrap();
        let again: TaskState = serde_json::from_str(&json).unwrap();
        assert_eq!(task, again);
    }

    #[test]
    fn a_task_stopped_by_lisa_survives_a_round_trip_through_json() {
        let mut task = new_task();
        task.stage = Stage::WaitingForHuman(WaitReason::StoppedByLisa(Role::Developer));

        let json = serde_json::to_string(&task).unwrap();
        let again: TaskState = serde_json::from_str(&json).unwrap();

        assert!(
            json.contains(r#""stage":{"waiting_for_human":{"stopped_by_lisa":"developer"}}"#),
            "{json}"
        );
        assert_eq!(again, task);
    }

    #[test]
    fn an_old_state_file_still_reads() {
        // Written by a harness from before the stop buttons.
        let old = r#"{"task_id":"task-001","workflow":"task","round":2,"max_rounds":5,
            "stage":{"waiting_for_human":{"role_asked_for_help":"tester"}}}"#;

        let state: TaskState = serde_json::from_str(old).unwrap();

        assert_eq!(
            state.stage,
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester))
        );
    }

    #[test]
    fn lisa_may_send_a_stopped_task_to_any_role() {
        let mut task = new_task();
        task.stage = Stage::WaitingForHuman(WaitReason::StoppedByLisa(Role::Developer));
        task.apply(&handoff(
            Role::Human,
            1,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        ))
        .unwrap();
        assert_eq!(task.stage, Stage::Working(Role::Architect));
        assert_eq!(task.round, 1, "a note does not start a new round");
    }

    #[test]
    fn a_handoff_in_an_unknown_format_version_is_refused() {
        let mut task = new_task();
        let mut h = handoff(
            Role::Architect,
            1,
            Verdict::Approved,
            NextStep::To(Role::Human),
        );
        h.schema_version = 2;
        assert_eq!(
            task.apply(&h),
            Err(TransitionError::UnknownSchemaVersion(2))
        );
        assert_eq!(task.stage, Stage::Working(Role::Architect));
    }

    #[test]
    fn a_handoff_with_a_huge_summary_is_refused() {
        let mut task = new_task();
        let mut h = handoff(
            Role::Architect,
            1,
            Verdict::Approved,
            NextStep::To(Role::Human),
        );
        h.summary = "x".repeat(crate::task::handoff::MAX_TEXT_CHARS + 1);
        assert_eq!(
            task.apply(&h),
            Err(TransitionError::TooLong {
                field: "summary",
                max_chars: crate::task::handoff::MAX_TEXT_CHARS
            })
        );
    }
}
