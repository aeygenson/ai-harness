//! The Retro tab: retrospectives of finished tasks and the skill changes they propose.
//!
//! `tab.rs` holds the tab's state and keys; `run.rs` (generating) and
//! `draw.rs` (drawing) add to the same `RetroTab`. `actions.rs` adds the
//! work behind the tab to `App`.

mod actions;
mod draw;
mod run;
mod tab;

pub(crate) use tab::{Action, Focus, RetroBuilder, RetroButton, RetroTab};
