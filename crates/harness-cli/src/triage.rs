//! `harness triage`: starts the user's own agent in this terminal to find out
//! why a task went wrong (see `harness_agents::triage`). The TUI's Tasks tab
//! has the same as a button.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use harness_agents::triage;
use harness_core::config::AgentKind;
use harness_core::git::Repo;
use harness_core::task::branch;

use crate::{open_repo, open_task};

/// Runs `harness triage`: checks the task, picks the agent, starts it in this
/// terminal and waits until the user closes it.
pub(crate) fn triage(
    project: &Path,
    task_id: Option<&str>,
    agent: Option<AgentKind>,
) -> Result<()> {
    let mut repo = open_repo(project)?;
    if let Some(task_id) = task_id {
        // A wrong id stops here, before an agent starts and searches in vain.
        open_task(&repo, task_id)?;
        // The agent looks in the task's own folder, where its files are. A
        // folder that is gone is not made again: triage only reads.
        let place = branch::task_places(&repo)?
            .into_iter()
            .find(|place| place.id == task_id);
        if let Some(place) = place {
            repo = Repo::open(&place.root)?;
        }
    }
    let (agent, program) =
        triage::pick_agent(agent, harness_platform::program::find).map_err(|e| anyhow!(e))?;
    match task_id {
        Some(id) => println!("Starting {agent} to look into {id}..."),
        None => println!("Starting {agent}; it will ask which task to look into..."),
    }
    let status = triage::command(&program, repo.root(), task_id)
        .status()
        .with_context(|| format!("cannot start {}", program.display()))?;
    if !status.success() {
        bail!("{agent} ended with an error ({status})");
    }
    Ok(())
}
