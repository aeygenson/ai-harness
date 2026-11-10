//! What goes into a role's temporary Cursor config folder: `cli-config.json`
//! with the role's deny rules, and `mcp.json` with the role's MCP servers.

use harness_core::mcp::McpServer;
use harness_core::task::handoff::Role;
use harness_core::task::permissions::Commands;

use harness_core::task::commands::{DENIED_FOR_ALL, DENIED_FOR_READERS};

/// Folders inside the project no role may write with Cursor's own file tool.
/// The git check after the role catches the rest of the forbidden files.
const DENIED_WRITES: &[&str] = &[".git/**", ".cursor/**"];

/// The role's `cli-config.json`. With `--force` Cursor runs every command
/// unless a `deny` rule refuses it, so the deny list is what limits a role,
/// as with Antigravity.
pub(super) fn cli_config(role: Role, servers: &[McpServer]) -> String {
    let spec = role.spec();
    let mut commands: Vec<&str> = DENIED_FOR_ALL.to_vec();
    if spec.commands != Commands::Any {
        commands.extend(DENIED_FOR_READERS);
    }
    let mut deny: Vec<String> = commands.iter().map(|c| shell_rule(c)).collect();
    deny.extend(DENIED_WRITES.iter().map(|path| format!("Write({path})")));
    if !spec.network {
        deny.push("WebFetch(*)".into());
    }
    // Only the role's own servers exist in its `mcp.json`, so all their tools may run.
    let allow: Vec<String> = servers
        .iter()
        .map(|server| format!("Mcp({}:*)", server.name))
        .collect();
    let config = serde_json::json!({
        "version": 1,
        "editor": { "vimMode": false },
        "permissions": { "allow": allow, "deny": deny },
        // The harness makes the commits; no "Made with Cursor" lines.
        "attribution": {
            "attributeCommitsToAgent": false,
            "attributePRsToAgent": false,
        },
    });
    serde_json::to_string_pretty(&config).expect("the config is plain JSON")
}

/// Cursor's rule for a command: `Shell(rm)` for a program, and
/// `Shell(git:commit*)` for a program with its first word (`git commit`).
fn shell_rule(command: &str) -> String {
    match command.split_once(' ') {
        Some((program, rest)) => format!("Shell({program}:{rest}*)"),
        None => format!("Shell({command})"),
    }
}

/// `{"mcpServers": {"<name>": {"command", "args", "env"}}}`, the format of
/// Cursor's `mcp.json`. It holds the servers' secrets, so it is written only
/// into the private temporary folder.
pub(super) fn mcp_config(servers: &[McpServer]) -> String {
    let servers: serde_json::Map<String, serde_json::Value> = servers
        .iter()
        .map(|server| {
            let env: serde_json::Map<String, serde_json::Value> = server
                .env
                .iter()
                .map(|(name, value)| (name.clone(), value.expose().into()))
                .collect();
            let spec = serde_json::json!({
                "command": server.command,
                "args": server.args,
                "env": env,
            });
            (server.name.clone(), spec)
        })
        .collect();
    serde_json::json!({ "mcpServers": servers }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::secret::Secret;

    fn deny(role: Role) -> Vec<String> {
        let config: serde_json::Value = serde_json::from_str(&cli_config(role, &[])).unwrap();
        serde_json::from_value(config["permissions"]["deny"].clone()).unwrap()
    }

    #[test]
    fn a_command_with_a_word_becomes_a_rule_with_arguments() {
        assert_eq!(shell_rule("rm"), "Shell(rm)");
        assert_eq!(shell_rule("git commit"), "Shell(git:commit*)");
        assert_eq!(shell_rule("dotnet build"), "Shell(dotnet:build*)");
    }

    #[test]
    fn no_role_commits_pushes_or_uses_gh() {
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            let deny = deny(role);
            for rule in ["Shell(git:commit*)", "Shell(git:push*)", "Shell(gh)"] {
                assert!(deny.contains(&rule.to_string()), "{role:?}: {deny:?}");
            }
            assert!(deny.contains(&"Write(.git/**)".to_string()), "{role:?}");
            assert!(deny.contains(&"Write(.cursor/**)".to_string()), "{role:?}");
        }
    }

    #[test]
    fn only_roles_without_any_command_lose_rm_and_curl() {
        assert!(deny(Role::Security).contains(&"Shell(rm)".to_string()));
        assert!(deny(Role::Architect).contains(&"Shell(curl)".to_string()));
        assert!(!deny(Role::Developer).contains(&"Shell(rm)".to_string()));
        assert!(!deny(Role::Tester).contains(&"Shell(cargo:build*)".to_string()));
    }

    #[test]
    fn a_role_without_network_cannot_fetch_pages() {
        assert!(deny(Role::Architect).contains(&"WebFetch(*)".to_string()));
        assert!(!deny(Role::Developer).contains(&"WebFetch(*)".to_string()));
    }

    #[test]
    fn commits_are_not_signed_by_cursor() {
        let config: serde_json::Value =
            serde_json::from_str(&cli_config(Role::Developer, &[])).unwrap();
        assert_eq!(config["attribution"]["attributeCommitsToAgent"], false);
        assert_eq!(config["attribution"]["attributePRsToAgent"], false);
    }

    #[test]
    fn the_roles_servers_are_allowed_and_written_with_their_env() {
        let server = McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "ctx".into()],
            env: [("KEY".to_string(), Secret::new("abc"))].into(),
        };
        let config: serde_json::Value =
            serde_json::from_str(&cli_config(Role::Tester, std::slice::from_ref(&server))).unwrap();
        assert_eq!(config["permissions"]["allow"][0], "Mcp(context7:*)");
        let mcp: serde_json::Value = serde_json::from_str(&mcp_config(&[server])).unwrap();
        assert_eq!(mcp["mcpServers"]["context7"]["command"], "npx");
        assert_eq!(mcp["mcpServers"]["context7"]["env"]["KEY"], "abc");
    }
}
