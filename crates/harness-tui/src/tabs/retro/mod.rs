//! The Retro tab: retrospectives of finished tasks and the skill changes they propose.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
mod tab;

pub(crate) use tab::{Action, Focus, RetroBuilder, RetroButton, RetroTab};
