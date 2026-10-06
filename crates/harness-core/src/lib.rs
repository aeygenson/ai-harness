//! The harness engine: everything except the user interface.
//!
//! Topics with several files have their own folder: `task/` (a task, its
//! handoffs, routes and the run loop), `config/` (`harness.toml` and the
//! project list), `mcp/`, `plugins/` and `retro/`. A folder's `mod.rs` holds
//! the topic's main types, so `harness_core::config::Config` reads the same
//! as before; the other files are its parts, such as `config::edit`.

pub mod config;
pub mod git;
pub mod mcp;
pub mod models;
pub mod plugins;
pub mod retro;
pub mod secret;
pub mod skills;
pub mod task;
pub mod text;
