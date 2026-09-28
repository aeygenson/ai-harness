//! The interface every agent adapter implements: Claude Code, Codex, Antigravity CLI, mock.

use std::future::Future;
use std::path::PathBuf;

use crate::handoff::Role;

/// Everything an agent needs to do one role once.
#[derive(Debug, Clone)]
pub struct RoleJob {
    pub task_id: String,
    pub round: u32,
    pub role: Role,
    /// The project folder: the agent works here.
    pub project_dir: PathBuf,
    /// The full prompt: role instructions, task, previous handoff.
    pub prompt: String,
    /// Where the agent must write `handoff.json` and `notes.md`.
    pub output_dir: PathBuf,
}

/// What happened when the agent ran. The handoff itself is read from `output_dir`.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    /// False if the agent crashed or exited with an error.
    pub success: bool,
    /// True if the subscription's usage limit stopped the agent.
    pub usage_limit_reached: bool,
    /// What the agent printed; saved as `agent.log`.
    pub log: String,
    /// If it failed: one short line for Lisa, for example "timed out after 1800 s".
    pub message: String,
}

/// Runs one role with some agent.
///
/// The method returns a `Future`: calling it starts nothing until the orchestrator
/// `.await`s it. Writing the return type out (instead of `async fn`) lets us promise
/// the future is `Send`, so it can later run on any Tokio thread. Implementations
/// may still simply write `async fn run(...)`.
pub trait AgentRunner {
    fn run(&self, job: &RoleJob) -> impl Future<Output = AgentOutcome> + Send;
}
