//! The Tasks tab: the tasks of the open project, their rounds and the message box.
//!
//! `tab.rs` holds the tab's state and loads the tasks; the other files add to
//! the same `TasksTab` by topic: `choice.rs` (whom the message goes to, model
//! and level), `events.rs` (keys and mouse), `run.rs` (sending the message),
//! `draw.rs` and `message_box.rs` (drawing), `steps.rs` (a step's text and
//! files), `step_rows.rs` (the rows of the steps list), `github.rs`
//! (sending a done task and merging its pull request),
//! `stop.rs` (the user's stop buttons while roles work), `live_log.rs` and
//! `log_panel.rs` (the scrollable live log), `times.rs` (how long a task
//! and its steps took).
//! `runner.rs` runs the roles in the background while the tab keeps
//! working.

pub(crate) mod agent_output;
mod choice;
mod draw;
mod events;
mod github;
mod labels;
pub(crate) mod live_log;
mod log_panel;
mod message_box;
mod run;
pub(crate) mod runner;
mod step_rows;
mod steps;
mod stop;
mod tab;
mod times;

pub(crate) use choice::Menu;
pub(crate) use tab::{TasksTab, Zoom};
// Only the tests pick a menu entry by name.
#[cfg(test)]
pub(crate) use choice::Choice;
