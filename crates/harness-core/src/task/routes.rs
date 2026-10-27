//! Which way work may go after each role, depending on its verdict.
//!
//! A role only *suggests* `next_role` in its handoff. The orchestrator accepts the
//! suggestion only if it is listed here (see `docs/handoff-format.md`).
//! Each workflow has its own table. The architect has three jobs: writing the
//! design, checking the developer's work against it, and the final check after
//! security. Each job has its own routes.

use serde::{Deserialize, Serialize};

use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
use crate::task::scenario::Workflow;
use crate::task::TaskState;

/// What the architect does when it is its turn. Saved in `state.json` as
/// `architect_job`; a task saved without it reads as `design`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectJob {
    /// Writes or fixes the design; the user must approve it.
    #[default]
    Design,
    /// Checks the developer's finished work against the approved design.
    Review,
    /// The last step of a task: after security, checks the whole result
    /// against the design, including the security findings and fixes.
    FinalReview,
}

impl ArchitectJob {
    /// True for [`ArchitectJob::Design`]: `state.json` leaves the default out.
    /// It takes a reference because serde's `skip_serializing_if` passes one.
    pub fn is_design(&self) -> bool {
        *self == ArchitectJob::Design
    }

    /// The architect's job after `handoff` is accepted.
    ///
    /// A review starts when the developer (or security, for the final one)
    /// hands over finished work, and lasts while the work stays with the
    /// architect or the user (the architect may ask the user a question).
    /// Once the work goes to any other role, the architect's next turn is a
    /// design job again: for example a tester who finds a problem in the
    /// design sends the work back to be redesigned.
    pub fn after(self, handoff: &Handoff) -> ArchitectJob {
        match (handoff.role, handoff.verdict, handoff.next_role) {
            (Role::Developer, Verdict::Approved, NextStep::To(Role::Architect)) => {
                ArchitectJob::Review
            }
            (Role::Security, Verdict::Approved, NextStep::To(Role::Architect)) => {
                ArchitectJob::FinalReview
            }
            // The user sends the design back to be reworked.
            (Role::Human, Verdict::Rejected, NextStep::To(Role::Architect)) => ArchitectJob::Design,
            (_, _, NextStep::To(Role::Architect | Role::Human)) => self,
            _ => ArchitectJob::Design,
        }
    }
}

/// Every place the work may go after `role` gives `verdict` in this task.
pub fn allowed_next(state: &TaskState, role: Role, verdict: Verdict) -> &'static [NextStep] {
    match state.workflow {
        Workflow::Task => task_routes(role, state.architect_job, verdict),
    }
}

/// True if the work may go to `next` after `role` gives `verdict` in this task.
pub fn is_allowed(state: &TaskState, role: Role, verdict: Verdict, next: NextStep) -> bool {
    allowed_next(state, role, verdict).contains(&next)
}

/// The routes of [`Workflow::Task`]: architect, the user, developer, the
/// architect's review, tester, security, the architect's final review.
#[expect(
    clippy::match_same_arms,
    reason = "a routing table: one arm per role and verdict is easier to check than merged arms"
)]
fn task_routes(role: Role, job: ArchitectJob, verdict: Verdict) -> &'static [NextStep] {
    use NextStep as N;

    match (role, verdict) {
        // Any AI role may stop and ask the user.
        (
            Role::Architect | Role::Developer | Role::Tester | Role::Security,
            Verdict::NeedsHuman,
        ) => &[N::To(Role::Human)],

        // A design always goes to the user for approval: the architect may not
        // skip them and hand its own design to the developer.
        (Role::Architect, Verdict::Approved) => match job {
            ArchitectJob::Design => &[N::To(Role::Human)],
            ArchitectJob::Review => &[N::To(Role::Tester)],
            // The user accepts the result when the task asked for a step only
            // they can do, such as a live check; otherwise the task is done.
            ArchitectJob::FinalReview => &[N::Done, N::To(Role::Human)],
        },
        (Role::Architect, Verdict::Rejected) => match job {
            ArchitectJob::Design => &[],
            ArchitectJob::Review | ArchitectJob::FinalReview => &[N::To(Role::Developer)],
        },

        // Finished code first goes to the architect, who checks it against the design.
        (Role::Developer, Verdict::Approved) => &[N::To(Role::Architect)],
        (Role::Developer, Verdict::Rejected) => &[N::To(Role::Architect)],

        (Role::Tester, Verdict::Approved) => &[N::To(Role::Security)],
        (Role::Tester, Verdict::Rejected) => &[N::To(Role::Developer), N::To(Role::Architect)],

        // The architect's final review, not security, finishes the task.
        (Role::Security, Verdict::Approved) => &[N::To(Role::Architect)],
        (Role::Security, Verdict::Rejected) => &[N::To(Role::Developer), N::To(Role::Architect)],

        // The user is the boss: they can send the work to any role, or finish it.
        (Role::Human, Verdict::Approved) => &[
            N::To(Role::Architect),
            N::To(Role::Developer),
            N::To(Role::Tester),
            N::To(Role::Security),
            N::Done,
        ],
        (Role::Human, Verdict::Rejected) => &[
            N::To(Role::Architect),
            N::To(Role::Developer),
            N::To(Role::Tester),
            N::To(Role::Security),
        ],
        (Role::Human, Verdict::NeedsHuman) => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::DEFAULT_MAX_ROUNDS;

    fn task(job: ArchitectJob) -> TaskState {
        let mut state = TaskState::new("task-001", Workflow::Task, DEFAULT_MAX_ROUNDS);
        state.architect_job = job;
        state
    }

    fn handoff(role: Role, verdict: Verdict, next: NextStep) -> Handoff {
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round: 1,
            role,
            verdict,
            next_role: next,
            summary: "test".into(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
            answer_to: None,
        }
    }

    #[test]
    fn tester_can_send_a_bug_back_to_the_developer() {
        assert!(is_allowed(
            &task(ArchitectJob::Design),
            Role::Tester,
            Verdict::Rejected,
            NextStep::To(Role::Developer)
        ));
    }

    #[test]
    fn finished_code_goes_to_the_architect_not_the_tester() {
        let state = task(ArchitectJob::Design);
        assert_eq!(
            allowed_next(&state, Role::Developer, Verdict::Approved),
            &[NextStep::To(Role::Architect)]
        );
        assert!(!is_allowed(
            &state,
            Role::Developer,
            Verdict::Approved,
            NextStep::To(Role::Tester)
        ));
    }

    #[test]
    fn a_design_always_goes_to_the_user() {
        assert_eq!(
            allowed_next(
                &task(ArchitectJob::Design),
                Role::Architect,
                Verdict::Approved
            ),
            &[NextStep::To(Role::Human)]
        );
    }

    #[test]
    fn a_review_sends_good_work_to_the_tester_and_bad_work_back() {
        let state = task(ArchitectJob::Review);
        assert_eq!(
            allowed_next(&state, Role::Architect, Verdict::Approved),
            &[NextStep::To(Role::Tester)]
        );
        assert_eq!(
            allowed_next(&state, Role::Architect, Verdict::Rejected),
            &[NextStep::To(Role::Developer)]
        );
    }

    #[test]
    fn only_the_final_review_can_finish_the_task() {
        let all = [
            ArchitectJob::Design,
            ArchitectJob::Review,
            ArchitectJob::FinalReview,
        ];
        for job in all {
            let state = task(job);
            for role in [Role::Developer, Role::Tester, Role::Security] {
                for verdict in [Verdict::Approved, Verdict::Rejected, Verdict::NeedsHuman] {
                    assert!(!is_allowed(&state, role, verdict, NextStep::Done));
                }
            }
            let finishes = is_allowed(&state, Role::Architect, Verdict::Approved, NextStep::Done);
            assert_eq!(finishes, job == ArchitectJob::FinalReview, "{job:?}");
        }
    }

    #[test]
    fn only_the_final_review_may_hand_approved_work_to_the_user_for_acceptance() {
        let to_user = NextStep::To(Role::Human);
        let final_review = task(ArchitectJob::FinalReview);
        let review = task(ArchitectJob::Review);

        assert!(is_allowed(
            &final_review,
            Role::Architect,
            Verdict::Approved,
            to_user
        ));
        assert!(!is_allowed(
            &review,
            Role::Architect,
            Verdict::Approved,
            to_user
        ));
        for role in [Role::Developer, Role::Tester, Role::Security] {
            assert!(!is_allowed(&final_review, role, Verdict::Approved, to_user));
        }
    }

    #[test]
    fn security_hands_the_work_to_the_final_review() {
        let state = task(ArchitectJob::Design);
        assert_eq!(
            allowed_next(&state, Role::Security, Verdict::Approved),
            &[NextStep::To(Role::Architect)]
        );
        let approved = handoff(
            Role::Security,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        );
        assert_eq!(
            ArchitectJob::Design.after(&approved),
            ArchitectJob::FinalReview
        );
        // A final review that finds a gap sends the work back to the developer.
        assert_eq!(
            allowed_next(
                &task(ArchitectJob::FinalReview),
                Role::Architect,
                Verdict::Rejected
            ),
            &[NextStep::To(Role::Developer)]
        );
    }

    #[test]
    fn the_review_starts_when_the_developer_finishes() {
        let done = handoff(
            Role::Developer,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        );
        assert_eq!(ArchitectJob::Design.after(&done), ArchitectJob::Review);

        // A developer who finds the design unclear asks for a design, not a review.
        let unclear = handoff(
            Role::Developer,
            Verdict::Rejected,
            NextStep::To(Role::Architect),
        );
        assert_eq!(ArchitectJob::Design.after(&unclear), ArchitectJob::Design);
    }

    #[test]
    fn the_review_lasts_through_a_question_to_the_user() {
        let question = handoff(
            Role::Architect,
            Verdict::NeedsHuman,
            NextStep::To(Role::Human),
        );
        let answer = handoff(
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Architect),
        );
        let job = ArchitectJob::Review.after(&question);
        assert_eq!(job.after(&answer), ArchitectJob::Review);

        // But a user who rejects sends the design back to be reworked.
        let rework = handoff(
            Role::Human,
            Verdict::Rejected,
            NextStep::To(Role::Architect),
        );
        assert_eq!(
            ArchitectJob::FinalReview.after(&rework),
            ArchitectJob::Design
        );
    }

    #[test]
    fn the_review_ends_when_the_work_leaves_the_architect() {
        let to_tester = handoff(
            Role::Architect,
            Verdict::Approved,
            NextStep::To(Role::Tester),
        );
        let to_developer = handoff(
            Role::Human,
            Verdict::Approved,
            NextStep::To(Role::Developer),
        );
        assert_eq!(ArchitectJob::Review.after(&to_tester), ArchitectJob::Design);
        assert_eq!(
            ArchitectJob::Review.after(&to_developer),
            ArchitectJob::Design
        );
    }
}
