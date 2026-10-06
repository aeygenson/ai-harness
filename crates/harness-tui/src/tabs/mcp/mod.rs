//! The MCP tab: the MCP servers each role gets, their secrets and the registry.
//!
//! `tab.rs` holds the tab itself (its state, keys and drawing); the other
//! files add the work behind it to `App`.

mod actions;
mod form;
mod tab;

pub(crate) use form::{form_values, server_from};
pub(crate) use tab::{Action, McpButton, McpCatalogButton, McpTab};
