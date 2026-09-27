//! Which way work may go after each role, depending on its verdict.
//!
//! A role only *suggests* `next_role` in its handoff. The orchestrator accepts the
//! suggestion only if it is listed here (see `docs/handoff-format.md`).

use crate::handoff::{NextStep, Role, Verdict};

/// Every place the work may go after `role` gives `verdict`.
pub fn allowed_next(role: Role, verdict: Verdict) -> &'static [NextStep] {
    use NextStep as N;

    match (role, verdict) {
        // Any AI role may stop and ask Lisa.
        (
            Role::Architect | Role::Developer | Role::Tester | Role::Security,
            Verdict::NeedsHuman,
        ) => &[N::Human],

        (Role::Architect, Verdict::Approved) => &[N::Human],
        (Role::Architect, Verdict::Rejected) => &[],

        (Role::Developer, Verdict::Approved) => &[N::Tester],
        (Role::Developer, Verdict::Rejected) => &[N::Architect],

        (Role::Tester, Verdict::Approved) => &[N::Security],
        (Role::Tester, Verdict::Rejected) => &[N::Developer, N::Architect],

        (Role::Security, Verdict::Approved) => &[N::Done],
        (Role::Security, Verdict::Rejected) => &[N::Developer, N::Architect],

        // Lisa is the boss: she can send the work to any role, or finish it.
        (Role::Human, Verdict::Approved) => {
            &[N::Architect, N::Developer, N::Tester, N::Security, N::Done]
        }
        (Role::Human, Verdict::Rejected) => &[N::Architect, N::Developer, N::Tester, N::Security],
        (Role::Human, Verdict::NeedsHuman) => &[],
    }
}

/// True if the work may go to `next` after `role` gives `verdict`.
pub fn is_allowed(role: Role, verdict: Verdict, next: NextStep) -> bool {
    allowed_next(role, verdict).contains(&next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tester_can_send_a_bug_back_to_the_developer() {
        assert!(is_allowed(
            Role::Tester,
            Verdict::Rejected,
            NextStep::Developer
        ));
    }

    #[test]
    fn developer_cannot_skip_the_tester() {
        assert!(!is_allowed(
            Role::Developer,
            Verdict::Approved,
            NextStep::Security
        ));
    }

    #[test]
    fn architect_always_goes_to_lisa() {
        assert_eq!(
            allowed_next(Role::Architect, Verdict::Approved),
            &[NextStep::Human]
        );
    }

    #[test]
    fn only_security_can_finish_the_task() {
        for role in [Role::Architect, Role::Developer, Role::Tester] {
            for verdict in [Verdict::Approved, Verdict::Rejected, Verdict::NeedsHuman] {
                assert!(!is_allowed(role, verdict, NextStep::Done));
            }
        }
        assert!(is_allowed(
            Role::Security,
            Verdict::Approved,
            NextStep::Done
        ));
    }
}
