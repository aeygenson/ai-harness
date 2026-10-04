//! Agent adapters: how to run each console agent for one role.
//! The mock, Claude Code, Codex CLI, Antigravity CLI and DeepSeek Harness.

pub mod antigravity;
pub mod build;
pub mod catalog;
pub mod claude;
pub mod codex;
pub mod credentials;
pub mod dsh;
pub mod launcher;
pub mod mcp_check;
pub mod mcp_oauth;
pub mod mcp_registry;
pub mod mcp_remote;
pub mod mock;
pub mod models;
pub mod process;
pub mod team;

pub use antigravity::Antigravity;
pub use claude::ClaudeCode;
pub use codex::Codex;
pub use dsh::Dsh;
pub use mock::{MockAgent, MockStep};
pub use team::{AnyAgent, Team};
