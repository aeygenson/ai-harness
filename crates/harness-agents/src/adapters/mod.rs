//! Adapters: one file per console agent, each running one role with that agent.
//!
//! Every adapter implements `harness_core::task::agent::AgentRunner`. The mock follows a
//! script and lets tests run the whole loop without a real agent.

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod dsh;
pub mod mock;
