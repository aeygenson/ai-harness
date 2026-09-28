//! The loop that runs roles one after another until the task has to stop.

use std::path::PathBuf;

use crate::agent::{AgentRunner, RoleJob};
use crate::handoff::{Handoff, NextStep, Role, Verdict};
use crate::prompt;
use crate::store::{StoreError, TaskStore};
use crate::task::{Stage, TaskState, WaitReason};

/// How many times one role may try before the harness gives up and asks Lisa.
pub const ATTEMPTS_PER_ROLE: u32 = 2;

/// Safety net: never run more than this many roles in one call to `run`.
pub const MAX_STEPS_PER_RUN: u32 = 50;

/// Why `run` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Done,
    WaitingForHuman(WaitReason),
    UsageLimitReached(Role),
    /// The role did not produce an acceptable handoff after every attempt.
    RoleFailed {
        role: Role,
        problem: String,
    },
    StepLimitReached,
}

/// Runs roles until the task is done or someone has to look at it.
///
/// Returns `Err` only for real problems of the harness itself (for example the
/// disk is full). Mistakes of an agent are a `StopReason`, not an error.
pub async fn run<A: AgentRunner>(
    store: &TaskStore,
    state: &mut TaskState,
    agent: &A,
) -> Result<StopReason, StoreError> {
    for _ in 0..MAX_STEPS_PER_RUN {
        let role = match state.stage {
            Stage::Done => return Ok(StopReason::Done),
            Stage::WaitingForHuman(reason) => return Ok(StopReason::WaitingForHuman(reason)),
            Stage::Working(role) => role,
        };

        let mut problem = String::new();
        let mut accepted = false;
        for _ in 0..ATTEMPTS_PER_ROLE {
            let job = prepare_job(store, state, role)?;
            let outcome = agent.run(&job).await;
            if outcome.usage_limit_reached {
                return Ok(StopReason::UsageLimitReached(role));
            }
            if !outcome.success {
                problem = format!("the agent exited with an error: {}", outcome.log);
                continue;
            }
            match accept_inbox(store, state)? {
                Ok(()) => {
                    accepted = true;
                    break;
                }
                Err(why) => problem = why,
            }
        }
        if !accepted {
            return Ok(StopReason::RoleFailed { role, problem });
        }
    }
    Ok(StopReason::StepLimitReached)
}

/// Lisa's decision (approve the design, send work back, answer a question),
/// saved like any other handoff with role `human`.
pub fn record_human_decision(
    store: &TaskStore,
    state: &mut TaskState,
    verdict: Verdict,
    next: NextStep,
    notes: &str,
) -> Result<PathBuf, StoreError> {
    let summary = notes
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .unwrap_or("Lisa's decision")
        .to_string();
    let issues = if verdict == Verdict::Rejected {
        vec![crate::handoff::Issue {
            severity: crate::handoff::Severity::Medium,
            location: None,
            description: summary.clone(),
        }]
    } else {
        vec![]
    };
    let handoff = Handoff {
        schema_version: 1,
        task_id: state.task_id.clone(),
        round: state.round,
        role: Role::Human,
        verdict,
        next_role: next,
        summary,
        skills_used: vec![],
        files: vec![],
        issues,
    };
    store.record(state, &handoff, notes)
}

fn prepare_job(store: &TaskStore, state: &TaskState, role: Role) -> Result<RoleJob, StoreError> {
    let output_dir = store.prepare_inbox()?;
    let history = store.history()?;
    let prompt = prompt::build(
        role,
        &store.description()?,
        state,
        history.last(),
        &output_dir,
    );
    Ok(RoleJob {
        task_id: state.task_id.clone(),
        round: state.round,
        role,
        prompt,
        output_dir,
    })
}

/// Outer `Result`: a real harness error. Inner `Result`: was the agent's work accepted?
fn accept_inbox(
    store: &TaskStore,
    state: &mut TaskState,
) -> Result<Result<(), String>, StoreError> {
    let (handoff, notes) = match store.read_inbox() {
        Ok(found) => found,
        Err(e) => return Ok(Err(format!("no valid handoff: {e}"))),
    };
    match store.record(state, &handoff, &notes) {
        Ok(_) => Ok(Ok(())),
        Err(StoreError::Refused(e)) => Ok(Err(format!("handoff refused: {e}"))),
        Err(e) => Err(e),
    }
}
