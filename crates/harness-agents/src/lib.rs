//! Agent adapters: how to run each console agent for one role.
//! The mock, Claude Code, Codex CLI, Antigravity CLI and `DeepSeek` Harness.
//!
//! Layout: `adapters/` holds one file per agent, `mcp/` the MCP tools that run outside a
//! role (check, OAuth sign-in, the web bridge, the registry), and `install/` the agent
//! catalog, saved logins and model lists. The single files are shared by all of them.

pub mod adapters;
pub mod agent_home;
pub mod build;
pub mod install;
pub mod launcher;
pub mod mcp;
pub mod process;
pub mod role_settings;
pub mod team;

pub use adapters::antigravity::Antigravity;
pub use adapters::claude::ClaudeCode;
pub use adapters::codex::Codex;
pub use adapters::dsh::Dsh;
pub use adapters::mock::{MockAgent, MockStep};
pub use team::{AnyAgent, Team};
