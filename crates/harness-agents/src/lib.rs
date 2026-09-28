//! Agent adapters: how to run each console agent for one role.
//! The mock, Claude Code, Codex CLI and Gemini CLI.

pub mod claude;
pub mod codex;
pub mod credentials;
pub mod gemini;
pub mod mock;
pub mod process;
pub mod team;

pub use claude::ClaudeCode;
pub use codex::Codex;
pub use gemini::Gemini;
pub use mock::{MockAgent, MockStep};
pub use team::{AnyAgent, Team};
