//! The Skills tab: the built-in and project skills of each role.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
mod tab;

pub(crate) use tab::{Action, SkillsTab, ROLES};
