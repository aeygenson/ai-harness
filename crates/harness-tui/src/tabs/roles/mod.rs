//! The Roles tab: which agent, model and effort level each role runs on, and
//! which skills, MCP servers and plugins it gets.
//!
//! `tab.rs` holds the tab's state, loading and saving; the other files add to
//! the same `RolesTab` by topic: `choice.rs` (agent, model and level),
//! `events.rs` (the rows and the keys and mouse on them), `draw.rs` and
//! `lines.rs` (drawing).

mod choice;
mod draw;
mod events;
mod lines;
mod tab;

pub(crate) use tab::{Action, RolesTab};
