//! `harness approve` and `harness reject`: the user's decision on a task,
//! which can first bring the code back to an earlier step (`--restore`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use harness_core::task::handoff::{NextStep, Verdict};
use harness_core::task::orchestrator::{self, HumanDecision};
use harness_core::task::run_lock::RunLock;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, WaitReason};

use crate::tasks::enter_task;
use crate::{open_repo, open_task};

/// The user's decision as `harness approve` and `harness reject` take it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Decision<'a> {
    pub(crate) verdict: Verdict,
    /// Who continues; `None` lets the task's stage choose.
    pub(crate) to: Option<NextStep>,
    pub(crate) notes: &'a str,
    /// `reject --restore`: a step of the task such as `round-01/01-architect`
    /// whose code the project gets back first.
    pub(crate) restore: Option<&'a str>,
}

/// `harness approve` / `harness reject`: writes the user's decision as a handoff
/// from the role `human`. Without `--to`, approved work goes where the task's
/// workflow says (a design goes to the developer), a result waiting for the
/// user's acceptance is finished, and a role that asked for help gets the
/// task back. With `--restore` the code outside `.harness/` is first made as
/// it was after that step, with its own commit.
pub(crate) fn decide(project: &Path, task_id: &str, decision: Decision<'_>) -> Result<()> {
    let Decision {
        verdict,
        to,
        notes,
        restore,
    } = decision;
    let repo = open_repo(project)?;
    let mut lock = RunLock::hold(&repo)?;
    lock.set_task(task_id)?;
    // The user's decision is a commit too: it goes on the task's branch, in its folder.
    let repo = enter_task(&repo, task_id)?;
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
            Stage::WaitingForHuman(WaitReason::AcceptResult) => NextStep::Done,
            Stage::WaitingForHuman(
                WaitReason::RoleAskedForHelp(role) | WaitReason::StoppedByUser(role),
            ) => NextStep::To(role),
            Stage::WaitingForHuman(WaitReason::RoundLimitReached)
            | Stage::Working(_)
            | Stage::Done => bail!("say who continues with --to <role|done>"),
        },
    };
    let restore_to = restore
        .map(|step| step_dir(&store, task_id, step))
        .transpose()?;
    orchestrator::record_human_decision(
        &repo,
        &store,
        &mut state,
        HumanDecision {
            verdict,
            next,
            notes,
            restore_to: restore_to.as_deref(),
        },
    )?;
    if let Some(step) = restore {
        println!("The code outside .harness/ is as after {step}; the history is kept.");
    }
    if state.stage == Stage::Done {
        println!("Saved. {task_id} is done.");
    } else {
        println!("Saved. Next: harness run {task_id}");
    }
    Ok(())
}

/// The folder of `step` (`round-01/01-architect`) in the task; an error that
/// lists the task's steps when there is no such step.
fn step_dir(store: &TaskStore, task_id: &str, step: &str) -> Result<PathBuf> {
    let steps = store.steps()?;
    let names: Vec<String> = steps.iter().map(|s| step_name(store, &s.dir)).collect();
    match names.iter().position(|name| name == step) {
        Some(index) => Ok(steps[index].dir.clone()),
        None => bail!(
            "{task_id} has no step {step:?}; its steps are: {}",
            names.join(", ")
        ),
    }
}

/// A step's folder as `harness status` and `--restore` name it:
/// `round-01/01-architect`, with `/` on every system.
pub(crate) fn step_name(store: &TaskStore, dir: &Path) -> String {
    let relative = dir.strip_prefix(store.dir()).unwrap_or(dir);
    harness_platform::path::slashed(relative)
}
