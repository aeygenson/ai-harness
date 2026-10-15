//! MCP servers outside the role run: checking a server, signing in with OAuth, the bridge
//! to servers on the web, searching the official registry, and the harness's own
//! read-only server (`serve`) that lets an agent watch the tasks.

pub mod check;
pub(crate) mod http_head;
pub mod oauth;
pub mod registry;
pub mod remote;
pub mod serve;
