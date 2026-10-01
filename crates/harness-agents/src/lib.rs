//! Agent adapters: how to run each console agent for one role.
//! The mock, Claude Code, Codex CLI and Antigravity CLI.

pub mod antigravity;
pub mod build;
pub mod claude;
pub mod codex;
pub mod credentials;
pub mod launcher;
pub mod mock;
pub mod process;
pub mod team;

pub use antigravity::Antigravity;
pub use claude::ClaudeCode;
pub use codex::Codex;
pub use mock::{MockAgent, MockStep};
pub use team::{AnyAgent, Team};
