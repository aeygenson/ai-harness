//! Noticing that a task goes round in circles, and the architect's rethink.
//!
//! A task loops when one role keeps sending the work back: each round patches
//! the last problem, and the next run fails in a new way. More rounds of the
//! same approach rarely help then; a different approach does. So when one AI
//! role has sent the work back [`REJECTIONS_BEFORE_RETHINK`] times in a row,
//! the harness gives the next turn to the architect with the job
//! [`ArchitectJob::Rethink`]: it reads the whole history of rejections and
//! proposes another approach. The user decides: accept it (the architect then
//! rewrites the design, job [`ArchitectJob::NewApproach`]), or send the work
//! on as before.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::task::handoff::{Handoff, NextStep, Role, Severity, Verdict};
use crate::task::routes::ArchitectJob;
use crate::task::store::Step;
use crate::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use crate::text;

/// After this many rejections in a row from one role the architect rethinks
/// the approach. Three, because once is normal, twice can still be bad luck,
/// and a third time the same check fails it is a pattern; it also comes
/// before the round limit ([`DEFAULT_MAX_ROUNDS`]), while there is money and
/// time left for another approach.
pub const REJECTIONS_BEFORE_RETHINK: u32 = 3;

/// At most this many issues of one step are shown in the rethink's history.
const MAX_ISSUES_SHOWN: usize = 10;

/// A summary or an issue in the history is cut after this many characters.
const MAX_HISTORY_CHARS: usize = 400;

/// At most this many of the latest rejections are shown, so a long task does
/// not fill the whole prompt; the steps' own files hold the rest.
const MAX_REJECTIONS_SHOWN: usize = 20;

/// The notes of a rethink the user asked for without writing any.
pub const DEFAULT_RETHINK_NOTE: &str =
    "Rethink the approach: the task keeps going round in circles.";

/// How many times in a row each AI role has sent the work back.
///
/// Saved in `state.json` as `"rejected_in_a_row": {"tester": 2}`; left out
/// while it is empty. A role's count starts again when it approves the work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RejectionCount(BTreeMap<Role, u32>);

impl RejectionCount {
    /// True when no role has a rejection counted. It takes a reference
    /// because serde's `skip_serializing_if` passes one.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many times in a row `role` has sent the work back.
    pub fn of(&self, role: Role) -> u32 {
        self.0.get(&role).copied().unwrap_or(0)
    }

    /// Counts `handoff`: a rejection adds one for its role, an approval starts
    /// that role's count again. The user's decisions are not counted.
    fn note(&mut self, handoff: &Handoff) {
        if handoff.role == Role::Human {
            return;
        }
        match handoff.verdict {
            Verdict::Rejected => *self.0.entry(handoff.role).or_insert(0) += 1,
            Verdict::Approved => {
                self.0.remove(&handoff.role);
            }
            // A question is neither: the role has not finished its check.
            Verdict::NeedsHuman => {}
        }
    }
}

impl TaskState {
    /// The architect's job after `handoff`, which is being applied.
    ///
    /// Besides [`ArchitectJob::after`], the user's decisions on a rethink
    /// matter: on a new approach waiting for them, sending it to the
    /// architect accepts it (the design is rewritten along it) and sending
    /// it back asks for another rethink; elsewhere a rethink the user asked
    /// for (see `orchestrator::ask_for_rethink`) stays a rethink.
    pub(crate) fn next_architect_job(&self, handoff: &Handoff) -> ArchitectJob {
        let to_architect = handoff.next_role == NextStep::To(Role::Architect);
        let by_user = handoff.role == Role::Human;
        if !(by_user && to_architect && self.architect_job == ArchitectJob::Rethink) {
            return self.architect_job.after(handoff);
        }
        let on_new_approach = self.stage == Stage::WaitingForHuman(WaitReason::ApproveNewApproach);
        if on_new_approach && handoff.verdict == Verdict::Approved {
            ArchitectJob::NewApproach
        } else {
            ArchitectJob::Rethink
        }
    }

    /// Updates the rejection counts with `handoff`. A fresh start for the
    /// counts when the user approves a design; when they accept a new
    /// approach, also at least the rounds a new task gets.
    ///
    /// Called by [`TaskState::apply`] after the architect's job is updated,
    /// while `stage` is still the old one.
    pub(crate) fn count_for_rethink(&mut self, handoff: &Handoff) {
        self.rejected_in_a_row.note(handoff);
        let approved_by_user = handoff.role == Role::Human && handoff.verdict == Verdict::Approved;
        let design_waits = self.stage == Stage::WaitingForHuman(WaitReason::ApproveDesign);
        let accepted = self.architect_job == ArchitectJob::NewApproach
            && self.stage == Stage::WaitingForHuman(WaitReason::ApproveNewApproach);
        if approved_by_user && (design_waits || accepted) {
            self.rejected_in_a_row = RejectionCount::default();
        }
        if accepted {
            // Counted from the rounds used so far.
            let used = self.round.saturating_sub(1);
            self.max_rounds = self.max_rounds.max(used + DEFAULT_MAX_ROUNDS);
        }
    }

    /// True when `handoff` is the rejection that makes its role's count
    /// reach [`REJECTIONS_BEFORE_RETHINK`]: the task loops. Only an AI
    /// role's rejection counts; approvals and the user's decisions never
    /// start a rethink.
    pub(crate) fn starts_rethink(&self, handoff: &Handoff) -> bool {
        handoff.role != Role::Human
            && handoff.verdict == Verdict::Rejected
            && self.rejected_in_a_row.of(handoff.role) >= REJECTIONS_BEFORE_RETHINK
    }

    /// Gives the next turn to the architect's rethink and starts the counts again.
    pub(crate) fn begin_rethink(&mut self) -> Stage {
        self.rejected_in_a_row = RejectionCount::default();
        self.architect_job = ArchitectJob::Rethink;
        Stage::Working(Role::Architect)
    }
}

/// Every time the work was sent back in this task, oldest first, for the
/// architect's rethink: round, role, summary and issues.
///
/// The texts come from agents, so control characters are removed and each is
/// cut to one line.
pub fn history_text(steps: &[Step]) -> String {
    let mut out = String::from(
        "History of this task: every time the work was sent back (by an AI role or \
         the user), oldest first. Look for what repeats.\n",
    );
    let rejected: Vec<&Step> = steps
        .iter()
        .filter(|step| step.handoff.verdict == Verdict::Rejected)
        .collect();
    if rejected.is_empty() {
        out.push_str("- nothing was sent back yet: the user asked for this rethink.\n");
    }
    let skipped = rejected.len().saturating_sub(MAX_REJECTIONS_SHOWN);
    if skipped > 0 {
        let _ = writeln!(out, "- {skipped} older ones are left out here");
    }
    for step in rejected.into_iter().skip(skipped) {
        let handoff = &step.handoff;
        // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
        let _ = writeln!(
            out,
            "- round {}, {}: {}",
            handoff.round,
            handoff.role,
            text::safe_line(&handoff.summary, MAX_HISTORY_CHARS)
        );
        for issue in handoff.issues.iter().take(MAX_ISSUES_SHOWN) {
            let place = issue.location.as_deref().unwrap_or("-");
            let _ = writeln!(
                out,
                "  - {} at {}: {}",
                severity_name(issue.severity),
                text::safe_line(place, MAX_HISTORY_CHARS),
                text::safe_line(&issue.description, MAX_HISTORY_CHARS)
            );
        }
        if handoff.issues.len() > MAX_ISSUES_SHOWN {
            let _ = writeln!(
                out,
                "  - and {} more",
                handoff.issues.len() - MAX_ISSUES_SHOWN
            );
        }
    }
    out.push('\n');
    out
}

/// The severity as `handoff.json` writes it: `high`.
fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::task::handoff::Issue;

    fn handoff(role: Role, verdict: Verdict) -> Handoff {
        let issues = if verdict == Verdict::Rejected {
            vec![Issue {
                severity: Severity::High,
                location: Some("src/a.rs:1".into()),
                description: "the check failed again".into(),
            }]
        } else {
            vec![]
        };
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round: 2,
            role,
            verdict,
            next_role: NextStep::To(Role::Developer),
            summary: "sent back".into(),
            skills_used: vec![],
            files: vec![],
            issues,
            answer_to: None,
        }
    }

    #[test]
    fn an_approval_starts_the_roles_count_again() {
        // Arrange
        let mut count = RejectionCount::default();
        count.note(&handoff(Role::Tester, Verdict::Rejected));
        count.note(&handoff(Role::Tester, Verdict::Rejected));

        // Act
        count.note(&handoff(Role::Tester, Verdict::Approved));

        // Assert
        assert_eq!(count.of(Role::Tester), 0);
        assert!(count.is_empty());
    }

    #[test]
    fn the_third_rejection_of_one_role_is_a_loop_but_not_of_three_roles() {
        let mut count = RejectionCount::default();
        for role in [Role::Tester, Role::Security, Role::Architect] {
            count.note(&handoff(role, Verdict::Rejected));
        }
        let mut state = task_at_developer();
        state.rejected_in_a_row = count.clone();
        assert!(!state.starts_rethink(&handoff(Role::Tester, Verdict::Rejected)));

        count.note(&handoff(Role::Tester, Verdict::Rejected));
        count.note(&handoff(Role::Tester, Verdict::Rejected));
        state.rejected_in_a_row = count;

        assert!(state.starts_rethink(&handoff(Role::Tester, Verdict::Rejected)));
        assert!(!state.starts_rethink(&handoff(Role::Tester, Verdict::Approved)));
    }

    #[test]
    fn the_users_decisions_and_questions_are_not_counted() {
        let mut count = RejectionCount::default();
        count.note(&handoff(Role::Human, Verdict::Rejected));
        count.note(&handoff(Role::Tester, Verdict::NeedsHuman));
        assert!(count.is_empty());
    }

    #[test]
    fn the_count_is_saved_by_role_name() {
        let mut count = RejectionCount::default();
        count.note(&handoff(Role::Tester, Verdict::Rejected));
        let json = serde_json::to_string(&count).unwrap();
        assert_eq!(json, r#"{"tester":1}"#);
        assert_eq!(
            serde_json::from_str::<RejectionCount>(&json).unwrap(),
            count
        );
    }

    #[test]
    fn the_history_lists_only_rejections_with_their_issues_on_one_line() {
        let mut rejected = handoff(Role::Tester, Verdict::Rejected);
        rejected.summary = "line one\nline two\u{1b}[31m".into();
        let steps = vec![
            Step {
                dir: PathBuf::from("round-01/01-architect"),
                handoff: handoff(Role::Architect, Verdict::Approved),
                notes: String::new(),
            },
            Step {
                dir: PathBuf::from("round-02/05-tester"),
                handoff: rejected,
                notes: String::new(),
            },
        ];

        let text = history_text(&steps);

        assert!(text.contains("- round 2, tester: line one"), "{text}");
        assert!(
            text.contains("high at src/a.rs:1: the check failed again"),
            "{text}"
        );
        assert!(!text.contains("architect"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
    }

    /// A task at the developer's turn, with an approved design.
    fn task_at_developer() -> TaskState {
        let mut state = TaskState::new(
            "task-001",
            crate::task::scenario::Workflow::Task,
            DEFAULT_MAX_ROUNDS,
        );
        state.stage = Stage::Working(Role::Developer);
        state
    }

    fn step(state: &TaskState, role: Role, verdict: Verdict, next: NextStep) -> Handoff {
        let mut handoff = handoff(role, verdict);
        handoff.round = state.round;
        handoff.next_role = next;
        handoff
    }

    /// Developer, architect's review and a tester who rejects: one loop round.
    fn tester_rejects(state: &mut TaskState) {
        let to = |role| NextStep::To(role);
        state
            .apply(&step(
                state,
                Role::Developer,
                Verdict::Approved,
                to(Role::Architect),
            ))
            .unwrap();
        state
            .apply(&step(
                state,
                Role::Architect,
                Verdict::Approved,
                to(Role::Tester),
            ))
            .unwrap();
        state
            .apply(&step(
                state,
                Role::Tester,
                Verdict::Rejected,
                to(Role::Developer),
            ))
            .unwrap();
    }

    #[test]
    fn the_third_rejection_by_the_tester_goes_to_the_architects_rethink() {
        // Arrange: the tester sent the work back twice.
        let mut state = task_at_developer();
        tester_rejects(&mut state);
        tester_rejects(&mut state);
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        assert_eq!(state.rejected_in_a_row.of(Role::Tester), 2);

        // Act
        tester_rejects(&mut state);

        // Assert
        assert_eq!(state.stage, Stage::Working(Role::Architect));
        assert_eq!(state.architect_job, ArchitectJob::Rethink);
        assert!(state.rejected_in_a_row.is_empty());
    }

    /// A task whose rethink waits for the user.
    fn rethink_waiting() -> TaskState {
        let mut state = task_at_developer();
        for _ in 0..REJECTIONS_BEFORE_RETHINK {
            tester_rejects(&mut state);
        }
        state
            .apply(&step(
                &state,
                Role::Architect,
                Verdict::Approved,
                NextStep::To(Role::Human),
            ))
            .unwrap();
        state
    }

    #[test]
    fn a_rethink_goes_only_to_the_user_who_sees_a_new_approach() {
        let mut state = task_at_developer();
        for _ in 0..REJECTIONS_BEFORE_RETHINK {
            tester_rejects(&mut state);
        }
        let to_developer = step(
            &state,
            Role::Architect,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        );
        assert!(state.clone().apply(&to_developer).is_err());

        let state = rethink_waiting();

        assert_eq!(
            state.stage,
            Stage::WaitingForHuman(WaitReason::ApproveNewApproach)
        );
    }

    #[test]
    fn accepting_the_new_approach_has_the_design_rewritten_with_fresh_rounds() {
        // Arrange: round 4 of 5 is used up to the rethink.
        let mut state = rethink_waiting();
        assert_eq!(state.round, 4);

        // Act
        let accept = step(
            &state,
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        );
        state.apply(&accept).unwrap();

        // Assert
        assert_eq!(state.stage, Stage::Working(Role::Architect));
        assert_eq!(state.architect_job, ArchitectJob::NewApproach);
        assert_eq!(state.max_rounds, 3 + DEFAULT_MAX_ROUNDS);
        // The rewritten design goes to the user like any design.
        let design = step(
            &state,
            Role::Architect,
            Verdict::Approved,
            NextStep::To(Role::Human),
        );
        state.apply(&design).unwrap();
        assert_eq!(
            state.stage,
            Stage::WaitingForHuman(WaitReason::ApproveDesign)
        );
    }

    #[test]
    fn sending_the_rethink_back_asks_for_another_and_going_on_keeps_the_old_way() {
        let mut again = rethink_waiting();
        let back = step(
            &again,
            Role::Human,
            Verdict::Rejected,
            NextStep::To(Role::Architect),
        );
        again.apply(&back).unwrap();
        assert_eq!(again.architect_job, ArchitectJob::Rethink);

        let mut as_before = rethink_waiting();
        let rounds = as_before.max_rounds;
        let on = step(
            &as_before,
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        );
        as_before.apply(&on).unwrap();
        assert_eq!(as_before.stage, Stage::Working(Role::Developer));
        assert_eq!(as_before.architect_job, ArchitectJob::Design);
        assert_eq!(as_before.max_rounds, rounds);
    }

    #[test]
    fn the_users_own_rejections_never_start_a_rethink() {
        let mut state = task_at_developer();
        state.stage = Stage::WaitingForHuman(WaitReason::ApproveDesign);
        for _ in 0..REJECTIONS_BEFORE_RETHINK {
            let back = step(
                &state,
                Role::Human,
                Verdict::Rejected,
                NextStep::To(Role::Developer),
            );
            state.apply(&back).unwrap();
            state.stage = Stage::WaitingForHuman(WaitReason::ApproveDesign);
        }
        assert!(state.rejected_in_a_row.is_empty());
    }

    #[test]
    fn at_the_round_limit_the_users_choice_stands_over_a_rethink() {
        // Arrange: the limit and the third rejection come together.
        let mut state = task_at_developer();
        state.max_rounds = REJECTIONS_BEFORE_RETHINK;
        for _ in 0..REJECTIONS_BEFORE_RETHINK {
            tester_rejects(&mut state);
        }
        assert_eq!(
            state.stage,
            Stage::WaitingForHuman(WaitReason::RoundLimitReached)
        );

        // Act: the user sends the work on to the developer.
        let on = step(
            &state,
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        );
        state.apply(&on).unwrap();

        // Assert: the user's choice stands, and the developer's finished
        // work still gets the architect's review, not a rethink.
        assert_eq!(state.stage, Stage::Working(Role::Developer));
        let done = step(
            &state,
            Role::Developer,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        );
        state.apply(&done).unwrap();
        assert_eq!(state.architect_job, ArchitectJob::Review);
    }

    #[test]
    fn a_history_without_rejections_says_the_user_asked() {
        let text = history_text(&[]);
        assert!(text.contains("nothing was sent back yet"), "{text}");
    }
}
