//! The Plugins tab: plugins from catalogs, which role gets which, and updates.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
pub(crate) mod catalog;
mod jobs;
mod tab;

pub(crate) use tab::{Action, PluginsTab};
