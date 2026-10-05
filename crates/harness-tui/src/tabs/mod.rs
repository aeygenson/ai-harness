//! One module per tab of the TUI. A tab with more than one file has its own
//! folder: `tab.rs` is what is drawn and how it reacts to keys, the rest is the
//! work behind it.

pub(crate) mod agents;
pub(crate) mod mcp;
pub(crate) mod plugins;
pub(crate) mod projects;
pub(crate) mod retro;
pub(crate) mod roles;
pub(crate) mod skills;
pub(crate) mod tasks;
