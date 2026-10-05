//! The Tasks tab: the tasks of the open project, their rounds and the message box.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); `runner.rs`
//! runs the roles in the background while the tab keeps working.

pub(crate) mod runner;
mod tab;

pub(crate) use tab::{draw_list, Menu, TasksTab, Zoom};
// Only the tests pick a menu entry by name.
#[cfg(test)]
pub(crate) use tab::Choice;
