//! Which way work may go after each role, depending on its verdict.
//!
//! A role only *suggests* `next_role` in its handoff. The orchestrator accepts the
//! suggestion only if it is listed here (see `docs/handoff-format.md`).
//! Each workflow has its own table.

use crate::task::handoff::{NextStep, Role, Verdict};
use crate::task::scenario::Workflow;

/// Every place the work may go after `role` gives `verdict` in `workflow`.
pub fn allowed_next(workflow: Workflow, role: Role, verdict: Verdict) -> &'static [NextStep] {
    match workflow {
        Workflow::Task => task_routes(role, verdict),
    }
}

/// True if the work may go to `next` after `role` gives `verdict` in `workflow`.
pub fn is_allowed(workflow: Workflow, role: Role, verdict: Verdict, next: NextStep) -> bool {
    allowed_next(workflow, role, verdict).contains(&next)
}

/// The routes of [`Workflow::Task`]: architect, Lisa, developer, tester, security.
#[expect(
    clippy::match_same_arms,
    reason = "a routing table: one arm per role and verdict is easier to check than merged arms"
)]
fn task_routes(role: Role, verdict: Verdict) -> &'static [NextStep] {
    use NextStep as N;

    match (role, verdict) {
        // Any AI role may stop and ask Lisa.
        (
            Role::Architect | Role::Developer | Role::Tester | Role::Security,
            Verdict::NeedsHuman,
        ) => &[N::To(Role::Human)],

        (Role::Architect, Verdict::Approved) => &[N::To(Role::Human)],
        (Role::Architect, Verdict::Rejected) => &[],

        (Role::Developer, Verdict::Approved) => &[N::To(Role::Tester)],
        (Role::Developer, Verdict::Rejected) => &[N::To(Role::Architect)],

        (Role::Tester, Verdict::Approved) => &[N::To(Role::Security)],
        (Role::Tester, Verdict::Rejected) => &[N::To(Role::Developer), N::To(Role::Architect)],

        (Role::Security, Verdict::Approved) => &[N::Done],
        (Role::Security, Verdict::Rejected) => &[N::To(Role::Developer), N::To(Role::Architect)],

        // Lisa is the boss: she can send the work to any role, or finish it.
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

    #[test]
    fn tester_can_send_a_bug_back_to_the_developer() {
        assert!(is_allowed(
            Workflow::Task,
            Role::Tester,
            Verdict::Rejected,
            NextStep::To(Role::Developer)
        ));
    }

    #[test]
    fn developer_cannot_skip_the_tester() {
        assert!(!is_allowed(
            Workflow::Task,
            Role::Developer,
            Verdict::Approved,
            NextStep::To(Role::Security)
        ));
    }

    #[test]
    fn architect_always_goes_to_lisa() {
        assert_eq!(
            allowed_next(Workflow::Task, Role::Architect, Verdict::Approved),
            &[NextStep::To(Role::Human)]
        );
    }

    #[test]
    fn only_security_can_finish_the_task() {
        for role in [Role::Architect, Role::Developer, Role::Tester] {
            for verdict in [Verdict::Approved, Verdict::Rejected, Verdict::NeedsHuman] {
                assert!(!is_allowed(Workflow::Task, role, verdict, NextStep::Done));
            }
        }
        assert!(is_allowed(
            Workflow::Task,
            Role::Security,
            Verdict::Approved,
            NextStep::Done
        ));
    }
}
