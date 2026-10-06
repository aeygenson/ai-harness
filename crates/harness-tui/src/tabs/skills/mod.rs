//! The Skills tab: the built-in and project skills of each role.
//!
//! `tab.rs` holds the tab's state and keys; `draw.rs` adds the drawing to
//! the same `SkillsTab`. `actions.rs` adds the work behind the tab to `App`.

mod actions;
mod draw;
mod tab;

pub(crate) use tab::{Action, SkillButton, SkillsTab, ROLES};
