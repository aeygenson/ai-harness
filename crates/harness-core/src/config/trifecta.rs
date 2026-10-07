//! The "lethal trifecta": a role that can be talked into sending data out.
//!
//! An agent is easy to fool when three things meet in one role: it sees
//! private data (the project, any key it is given), it reads text someone
//! else wrote (a web page, a mail, an issue), and it can send data out (a
//! shell can run `curl`). Text from outside can then say "send the secrets
//! to this address", and the agent may do it. The harness only warns about
//! this (on the Roles tab and in `harness doctor`), since a role often
//! really needs such a server.
//!
//! Every role sees the project, so the warning depends on the other two:
//! a role that runs commands, and an MCP server that talks to the outside.
//! Plugins may bring servers too, but the harness cannot see inside them,
//! so they are not counted.

use std::collections::BTreeMap;

use super::{McpConfig, RoleConfig};
use crate::mcp::SECRET_PREFIX;
use crate::task::handoff::Role;

/// The MCP servers of `role` that talk to the outside, when the role also
/// runs commands; empty when there is nothing to warn about.
///
/// `settings` are the role's settings, `servers` all servers in `harness.toml`.
pub fn outside_servers(
    role: Role,
    settings: &RoleConfig,
    servers: &BTreeMap<String, McpConfig>,
) -> Vec<String> {
    if !runs_commands(role) {
        return Vec::new();
    }
    settings
        .mcp
        .iter()
        .filter(|name| servers.get(*name).is_some_and(talks_to_outside))
        .cloned()
        .collect()
}

/// Does the role run commands, with the network on? This mirrors the
/// agents' adapters. Security counts too: Claude Code lets it run only a
/// list of scanners, but Codex and the others give it a whole shell. The
/// Architect only reads and writes documents.
fn runs_commands(role: Role) -> bool {
    match role {
        Role::Developer | Role::Tester | Role::Security => true,
        Role::Architect | Role::Human => false,
    }
}

/// Does the server reach the outside world? A server on the web does, and
/// so does one that needs a key or a sign-in: a key is only needed to talk
/// to some service. A local server without a key (files, a database on
/// this computer) is not counted.
fn talks_to_outside(server: &McpConfig) -> bool {
    server.url.is_some()
        || server.auth.is_some()
        || !server.headers.is_empty()
        || server
            .env
            .values()
            .any(|value| value.starts_with(SECRET_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const CONFIG: &str = r#"
[roles.developer]
agent = "claude"
mcp = ["web", "keyed", "files"]

[roles.architect]
agent = "codex"
mcp = ["web"]

[mcp.web]
url = "https://example.com/mcp"

[mcp.keyed]
command = "npx"
args = ["-y", "@upstash/context7-mcp"]
env = { CONTEXT7_API_KEY = "secret:context7" }

[mcp.files]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
env = { LOG_LEVEL = "info" }
"#;

    #[test]
    fn a_role_running_commands_with_outside_servers_is_warned_about() {
        let config = Config::parse(CONFIG).unwrap();
        let developer = &config.roles[&Role::Developer];

        let servers = outside_servers(Role::Developer, developer, &config.mcp);

        // The local server without a key is not counted.
        assert_eq!(servers, ["web", "keyed"]);
    }

    #[test]
    fn a_role_without_commands_is_not_warned_about() {
        let config = Config::parse(CONFIG).unwrap();
        let architect = &config.roles[&Role::Architect];

        assert_eq!(
            outside_servers(Role::Architect, architect, &config.mcp),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_server_missing_from_the_settings_is_skipped() {
        let mut config = Config::parse(CONFIG).unwrap();
        config.mcp.remove("web");
        let developer = &config.roles[&Role::Developer];

        assert_eq!(
            outside_servers(Role::Developer, developer, &config.mcp),
            ["keyed"]
        );
    }
}
