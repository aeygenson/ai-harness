//! The Projects tab: the list of projects, opening one and creating a new one.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
mod github;
pub(crate) mod picker;
mod tab;

pub(crate) use github::{GitHubConnector, GitHubJob};
pub(crate) use tab::{has_config, ProjectsTab};
