//! The Agents tab: which agents are installed and signed in, and installing,
//! updating or removing them.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
mod tab;

pub(crate) use tab::{check, install, AgentChecker, AgentsTab, Installer, Job, JobEvent};
