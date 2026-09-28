//! Agent adapters: how to run each console agent for one role.
//! The mock, Claude Code and Codex CLI for now; Gemini CLI comes next.

pub mod claude;
pub mod codex;
pub mod credentials;
pub mod mock;
pub mod process;
pub mod team;

pub use claude::ClaudeCode;
pub use codex::Codex;
pub use mock::{MockAgent, MockStep};
pub use team::{AnyAgent, Team};
