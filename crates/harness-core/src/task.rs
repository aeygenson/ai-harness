//! The state of one task and the rules for moving it forward (the state machine).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::handoff::{Handoff, NextStep, Role, Verdict};
use crate::routes;

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
}

/// Where the task is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// An AI role is working (never `Role::Human`; Lisa's turn is `WaitingForHuman`).
    Working(Role),
    WaitingForHuman(WaitReason),
    Done,
}

/// Everything the orchestrator needs to remember about a task. Saved as `state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskState {
    pub task_id: String,
    pub round: u32,
    pub max_rounds: u32,
    pub stage: Stage,
}

/// Why a handoff was refused. The task state is left unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    TaskAlreadyDone,
    WrongAuthor {
        expected: Role,
        got: Role,
    },
    WrongTask {
        expected: String,
        got: String,
    },
    WrongRound {
        expected: u32,
        got: u32,
    },
    RejectedWithoutIssues,
    RouteNotAllowed {
        role: Role,
        verdict: Verdict,
        next: NextStep,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskAlreadyDone => write!(f, "the task is already done"),
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
    /// A new task always starts with the architect in round 1.
    pub fn new(task_id: impl Into<String>, max_rounds: u32) -> Self {
        Self {
            task_id: task_id.into(),
            round: 1,
            max_rounds,
            stage: Stage::Working(Role::Architect),
        }
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
        if !routes::is_allowed(handoff.role, handoff.verdict, handoff.next_role) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{Issue, Severity};

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
        }
    }

    fn new_task() -> TaskState {
        TaskState::new("task-001", DEFAULT_MAX_ROUNDS)
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
}
