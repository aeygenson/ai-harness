//! Taking the agent's handoff into the task, or saying why not.

use std::path::PathBuf;

use crate::task::handoff::Handoff;
use crate::task::store::{StoreError, TaskStore};
use crate::task::TaskState;

/// Was the agent's handoff taken into the task?
pub(super) enum Acceptance {
    /// Yes: the saved handoff and its step folder.
    Accepted { handoff: Handoff, step_dir: PathBuf },
    /// No: why, in one line for the next attempt's problem text.
    Refused(String),
}

/// Reads the agent's handoff and saves it as the next step. `Err` is only a
/// real harness error (for example the disk is full).
pub(super) fn accept_inbox(
    store: &TaskStore,
    state: &mut TaskState,
) -> Result<Acceptance, StoreError> {
    let (handoff, notes) = match store.read_inbox() {
        Ok(found) => found,
        Err(e) => return Ok(Acceptance::Refused(format!("no valid handoff: {e}"))),
    };
    match store.record(state, &handoff, &notes) {
        Ok(step_dir) => Ok(Acceptance::Accepted { handoff, step_dir }),
        Err(StoreError::Refused(e)) => Ok(Acceptance::Refused(format!("handoff refused: {e}"))),
        Err(e) => Err(e),
    }
}
