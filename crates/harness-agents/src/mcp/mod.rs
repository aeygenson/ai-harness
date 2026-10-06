//! MCP servers outside the role run: checking a server, signing in with OAuth, the bridge
//! to servers on the web and searching the official registry.

pub mod check;
pub mod oauth;
pub mod registry;
pub mod remote;
