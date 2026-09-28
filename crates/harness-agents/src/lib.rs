//! Agent adapters: how to run each console agent for one role.
//! For now only the mock; Claude Code, Codex and Gemini CLI come next.

pub mod mock;

pub use mock::{MockAgent, MockStep};
