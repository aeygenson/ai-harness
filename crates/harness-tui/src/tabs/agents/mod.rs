//! The Agents tab: which agents are installed and signed in, and installing,
//! updating or removing them; below the list, the other programs the harness
//! needs (the same check as `harness doctor`).
//!
//! `tab.rs` holds the tab's state and keys; `draw.rs` adds the drawing to the
//! same `AgentsTab`. `actions.rs` adds the work behind the tab to `App`.

mod actions;
mod computer;
mod draw;
mod tab;

pub(crate) use tab::{
    check, install, AgentChecker, AgentsTab, Installer, Job, JobEvent, ToolChecker,
};
