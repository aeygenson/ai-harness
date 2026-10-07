//! Getting a role's job ready: its prompt, and the manifest saved with the step.

use crate::git::Repo;
use crate::task::agent::RoleJob;
use crate::task::facts;
use crate::task::handoff::Role;
use crate::task::manifest::{self, Manifest};
use crate::task::prompt;
use crate::task::store::{StoreError, TaskStore};
use crate::task::TaskState;

use super::RunSetup;

/// The job for `role` (an empty inbox, the full prompt) and the manifest
/// that is saved with the step if the role's work is accepted.
pub(super) fn prepare_job(
    repo: &Repo,
    store: &TaskStore,
    state: &TaskState,
    role: Role,
    setup: RunSetup<'_>,
) -> Result<(RoleJob, Manifest), StoreError> {
    let output_dir = store.prepare_inbox()?;
    let steps = store.steps()?;
    let facts = facts::text(repo, &steps);
    let skills = setup.skills.for_role(role);
    let context = prompt::Context {
        description: &store.description()?,
        previous: steps.last().map(|step| &step.handoff),
        facts: &facts,
        skills: &skills,
    };
    let prompt = prompt::build(role, state, context, &output_dir);
    let manifest = manifest::build(role, setup.agents, &skills, &prompt);
    let job = RoleJob {
        task_id: state.task_id.clone(),
        round: state.round,
        role,
        project_dir: repo.root().to_path_buf(),
        prompt,
        output_dir,
    };
    Ok((job, manifest))
}
