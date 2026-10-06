//! The Tasks tab: the tasks of the open project, their rounds and the message box.
//!
//! `tab.rs` holds the tab's state and loads the tasks; the other files add to
//! the same `TasksTab` by topic: `choice.rs` (whom the message goes to, model
//! and level), `events.rs` (keys and mouse), `run.rs` (sending the message),
//! `draw.rs` and `message_box.rs` (drawing), `steps.rs` (a step's text and
//! files). `runner.rs` runs the roles in the background while the tab keeps
//! working.

mod choice;
mod draw;
mod events;
mod labels;
mod message_box;
mod run;
pub(crate) mod runner;
mod steps;
mod tab;

pub(crate) use choice::Menu;
pub(crate) use draw::draw_list;
pub(crate) use tab::{TasksTab, Zoom};
// Only the tests pick a menu entry by name.
#[cfg(test)]
pub(crate) use choice::Choice;
