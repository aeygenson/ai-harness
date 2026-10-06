//! The MCP tab: the MCP servers each role gets, their secrets and the registry.
//!
//! `tab.rs` holds the tab's state; `events.rs`, `draw.rs`, `details.rs`,
//! `catalog.rs` and `catalog_draw.rs` add to the same `McpTab` by topic.
//! `actions.rs` adds the work behind the tab to `App`, and `form.rs` turns
//! the server form into settings and back.

mod actions;
mod catalog;
mod catalog_draw;
mod details;
mod draw;
mod events;
mod form;
mod tab;

pub(crate) use form::{form_values, server_from};
pub(crate) use tab::{Action, McpButton, McpCatalogButton, McpTab};
