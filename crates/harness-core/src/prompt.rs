//! Builds the prompt an agent receives for one role.
//!
//! Stage 3 version: the role's own `prompt.md` and skills come in a later stage.

use std::path::Path;

use crate::handoff::{Handoff, Role};
use crate::task::TaskState;

pub fn build(
    role: Role,
    task_description: &str,
    state: &TaskState,
    previous: Option<&Handoff>,
    output_dir: &Path,
) -> String {
    let mut prompt = format!(
        "You are the {role:?} in a team of AI roles.\n\
         Task {task} (round {round}):\n{task_description}\n\n",
        task = state.task_id,
        round = state.round,
    );
    if let Some(previous) = previous {
        let json = serde_json::to_string_pretty(previous).unwrap_or_default();
        prompt.push_str(&format!(
            "The previous step was done by the {:?}. Its handoff:\n{json}\n\n",
            previous.role
        ));
    }
    prompt.push_str(&format!(
        "When you finish, write two files into {dir}:\n\
         - notes.md: short notes for Lisa.\n\
         - handoff.json: your result in the format of docs/handoff-format.md, \
         with task_id \"{task}\", round {round} and role \"{role}\".\n\
         Do not commit to git; the harness does that.\n",
        dir = output_dir.display(),
        task = state.task_id,
        round = state.round,
        role = format!("{role:?}").to_lowercase(),
    ));
    prompt
}
