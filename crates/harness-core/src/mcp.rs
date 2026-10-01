//! MCP servers: extra tools a role may get, for example documentation search.
//!
//! A project describes each server once in `harness.toml` and lists it in the
//! roles that need it. An agent gets only the servers of its role and never the
//! ones from Lisa's own agent settings:
//!
//! ```toml
//! [mcp.context7]
//! command = "npx"
//! args = ["-y", "@upstash/context7-mcp"]
//! env = { CONTEXT7_API_KEY = "secret:context7" }
//!
//! [roles.developer]
//! agent = "codex"
//! mcp = ["context7"]
//! ```
//!
//! `"secret:context7"` means: the value is the secret that
//! `harness secret set context7` saved in `~/.harness/credentials/secrets/`.
//! The project keeps only the name, so the key never gets into git.
//!
//! Every adapter turns the same list into its agent's own settings.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::handoff::Role;
use crate::secret::Secret;

/// A value `"secret:<name>"` in `env` is read from a saved secret.
pub const SECRET_PREFIX: &str = "secret:";

/// One server as an agent starts it, with every secret already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// All values are kept as secrets, so none of them is printed by accident.
    pub env: BTreeMap<String, Secret>,
}

/// The servers of every role in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServers {
    roles: BTreeMap<Role, Vec<McpServer>>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error(
        "MCP server name {0:?} is not allowed; use lowercase letters, digits, '-' and '_', \
         for example \"context7\""
    )]
    BadServerName(String),
    #[error("the {role:?} role uses MCP server {name:?}, but harness.toml has no [mcp.{name}]")]
    UnknownServer { role: Role, name: String },
    #[error("MCP server {0:?} has an empty command")]
    EmptyCommand(String),
    #[error(
        "MCP server {server:?}: variable name {name:?} is not allowed; \
         use capital letters, digits and '_', for example API_KEY"
    )]
    BadVariable { server: String, name: String },
    #[error("MCP server {server:?}: variable {name:?} is reserved for the harness and the agents")]
    ReservedVariable { server: String, name: String },
    #[error(
        "the {role:?} role gets variable {name:?} from two MCP servers; \
         give them different names"
    )]
    VariableTwice { role: Role, name: String },
    #[error("MCP server {server:?}: secret name {name:?} is not allowed")]
    BadSecretName { server: String, name: String },
    #[error(
        "MCP server {server:?} needs the secret {name:?}; save it with `harness secret set {name}`"
    )]
    MissingSecret { server: String, name: String },
}

/// Variables an MCP server may not set: the agents read them themselves, so a
/// server setting could change the agent or leak into it.
const RESERVED_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];
const RESERVED_PREFIXES: &[&str] = &[
    "CLAUDE_",
    "ANTHROPIC_",
    "CODEX_",
    "OPENAI_",
    "DEEPSEEK_",
    "AGY_",
    "GEMINI_",
    "GOOGLE_",
];

impl McpServers {
    /// No servers for anyone.
    pub fn none() -> Self {
        Self::default()
    }

    /// Checks the servers every role lists and reads their secrets with
    /// `secret(name)`. Servers no role uses are not checked for secrets.
    pub fn load(
        config: &Config,
        secret: impl Fn(&str) -> Option<Secret>,
    ) -> Result<Self, McpError> {
        let mut roles = BTreeMap::new();
        for (&role, settings) in &config.roles {
            let mut servers = Vec::new();
            let mut seen: Vec<&str> = Vec::new();
            for name in &settings.mcp {
                check_server_name(name)?;
                let server_config =
                    config
                        .mcp
                        .get(name)
                        .ok_or_else(|| McpError::UnknownServer {
                            role,
                            name: name.clone(),
                        })?;
                for variable in server_config.env.keys() {
                    if seen.contains(&variable.as_str()) {
                        return Err(McpError::VariableTwice {
                            role,
                            name: variable.clone(),
                        });
                    }
                    seen.push(variable);
                }
                servers.push(resolve(name, server_config, &secret)?);
            }
            roles.insert(role, servers);
        }
        Ok(Self { roles })
    }

    /// The servers of one role; a role without servers gets an empty list.
    pub fn for_role(&self, role: Role) -> Vec<McpServer> {
        self.roles.get(&role).cloned().unwrap_or_default()
    }
}

fn resolve(
    name: &str,
    config: &crate::config::McpConfig,
    secret: &impl Fn(&str) -> Option<Secret>,
) -> Result<McpServer, McpError> {
    if config.command.trim().is_empty() {
        return Err(McpError::EmptyCommand(name.to_string()));
    }
    let mut env = BTreeMap::new();
    for (variable, value) in &config.env {
        check_variable(name, variable)?;
        let value = match value.strip_prefix(SECRET_PREFIX) {
            Some(secret_name) => {
                let secret_name = secret_name.trim();
                if !is_simple_name(secret_name) {
                    return Err(McpError::BadSecretName {
                        server: name.to_string(),
                        name: secret_name.to_string(),
                    });
                }
                secret(secret_name).ok_or_else(|| McpError::MissingSecret {
                    server: name.to_string(),
                    name: secret_name.to_string(),
                })?
            }
            None => Secret::new(value.clone()),
        };
        env.insert(variable.clone(), value);
    }
    Ok(McpServer {
        name: name.to_string(),
        command: config.command.clone(),
        args: config.args.clone(),
        env,
    })
}

/// Checks one server as `harness.toml` describes it, the way a run would,
/// without reading its secrets: its name, command and variables.
pub fn check_server(name: &str, config: &crate::config::McpConfig) -> Result<(), McpError> {
    check_server_name(name)?;
    resolve(name, config, &|_| Some(Secret::new("unused"))).map(|_| ())
}

/// One server ready to start, its secrets read with `secret(name)`.
pub fn server(
    name: &str,
    config: &crate::config::McpConfig,
    secret: impl Fn(&str) -> Option<Secret>,
) -> Result<McpServer, McpError> {
    check_server_name(name)?;
    resolve(name, config, &secret)
}

/// Can `agent` start this server? Every agent of the harness starts a
/// program server (`stdio`) the same way, so for now this is any known
/// agent; servers some agent cannot start would be refused here.
pub fn agent_runs(agent: &str, _server: &crate::config::McpConfig) -> bool {
    crate::config::AGENTS.contains(&agent)
}

/// Lowercase letters, digits, `-` and `_`: safe in file names, in Codex's
/// `-c mcp_servers.<name>...` and in Claude Code's `mcp__<name>` rules.
pub fn is_simple_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn check_server_name(name: &str) -> Result<(), McpError> {
    // Claude Code joins names with "__" (`mcp__server__tool`).
    if is_simple_name(name) && !name.contains("__") {
        Ok(())
    } else {
        Err(McpError::BadServerName(name.to_string()))
    }
}

fn check_variable(server: &str, name: &str) -> Result<(), McpError> {
    let well_formed = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !well_formed {
        return Err(McpError::BadVariable {
            server: server.to_string(),
            name: name.to_string(),
        });
    }
    if RESERVED_VARIABLES.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Err(McpError::ReservedVariable {
            server: server.to_string(),
            name: name.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVERS: &str = r#"
        [mcp.context7]
        command = "npx"
        args = ["-y", "@upstash/context7-mcp"]
        env = { CONTEXT7_API_KEY = "secret:context7", MODE = "fast" }

        [mcp.unused]
        command = "x"
        env = { KEY = "secret:not-saved" }
    "#;

    fn load(roles: &str) -> Result<McpServers, McpError> {
        let config = Config::parse(&format!("{roles}\n{SERVERS}")).unwrap();
        McpServers::load(&config, |name| {
            (name == "context7").then(|| Secret::new("ctx-secret"))
        })
    }

    #[test]
    fn each_role_gets_only_its_servers_with_secrets_read() {
        let servers = load(
            "[roles.developer]\nagent = \"codex\"\nmcp = [\"context7\"]\n\
             [roles.tester]\nagent = \"claude\"\n",
        )
        .unwrap();
        let developer = servers.for_role(Role::Developer);
        assert_eq!(developer.len(), 1);
        let context7 = &developer[0];
        assert_eq!(context7.command, "npx");
        assert_eq!(context7.args, ["-y", "@upstash/context7-mcp"]);
        assert_eq!(context7.env["CONTEXT7_API_KEY"].expose(), "ctx-secret");
        assert_eq!(context7.env["MODE"].expose(), "fast");
        assert!(servers.for_role(Role::Tester).is_empty());
        assert!(servers.for_role(Role::Security).is_empty());
        // The secret never shows in a debug print.
        assert!(!format!("{servers:?}").contains("ctx-secret"));
    }

    #[test]
    fn an_unknown_server_names_the_role() {
        let error = load("[roles.tester]\nagent = \"claude\"\nmcp = [\"github\"]\n")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Tester") && error.contains("[mcp.github]"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_secret_says_how_to_save_it() {
        let error = load("[roles.tester]\nagent = \"claude\"\nmcp = [\"unused\"]\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("harness secret set not-saved"), "{error}");
    }

    #[test]
    fn bad_names_are_refused() {
        for (server, env) in [
            ("Big", "A = \"1\""),
            ("a__b", "A = \"1\""),
            ("a.b", "A = \"1\""),
            ("ok", "lower = \"1\""),
            ("ok", "PATH = \"/tmp\""),
            ("ok", "OPENAI_API_KEY = \"x\""),
            ("ok", "CLAUDE_CODE_OAUTH_TOKEN = \"x\""),
            ("ok", "A = \"secret:../x\""),
        ] {
            let toml = format!(
                "[roles.tester]\nagent = \"claude\"\nmcp = [{server:?}]\n\
                 [mcp.{server:?}]\ncommand = \"x\"\nenv = {{ {env} }}\n"
            );
            let config = Config::parse(&toml).unwrap();
            assert!(
                McpServers::load(&config, |_| Some(Secret::new("s"))).is_err(),
                "{server} {env}"
            );
        }
    }

    #[test]
    fn one_role_cannot_get_the_same_variable_from_two_servers() {
        let config = Config::parse(
            "[roles.tester]\nagent = \"claude\"\nmcp = [\"a\", \"b\"]\n\
             [mcp.a]\ncommand = \"x\"\nenv = { KEY = \"1\" }\n\
             [mcp.b]\ncommand = \"y\"\nenv = { KEY = \"2\" }\n",
        )
        .unwrap();
        assert!(matches!(
            McpServers::load(&config, |_| None),
            Err(McpError::VariableTwice { .. })
        ));
    }
}
