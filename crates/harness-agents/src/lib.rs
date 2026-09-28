//! Agent adapters: how to run each console agent for one role.
//! The mock and Claude Code for now; Codex and Gemini CLI come next.

pub mod claude;
pub mod credentials;
pub mod mock;

pub use claude::ClaudeCode;
pub use mock::{MockAgent, MockStep};
