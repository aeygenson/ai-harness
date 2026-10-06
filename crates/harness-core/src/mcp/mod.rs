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
//! A server on the web is described by its address instead of a command:
//!
//! ```toml
//! [mcp.github]
//! url = "https://api.githubcopilot.com/mcp/"
//! headers = { Authorization = "Bearer secret:github" }
//! ```
//!
//! A server that wants a sign-in in the browser (OAuth) says `auth = "oauth"`
//! instead of a key in its headers; `harness mcp login <name>` signs in once
//! and the harness keeps the tokens (see [`OAUTH_SECRET`]).
//!
//! The agents start it like any other server, as the program
//! `harness mcp-remote`, which talks to the address and adds the headers
//! with the secrets itself (see [`BRIDGE_COMMAND`]).
//!
//! Every adapter turns the same list into its agent's own settings.
//!
//! The folder also holds `registry` (the official MCP registry) and `tools`
//! (the tools a server offers).

pub mod registry;
pub mod tools;

mod resolve;

pub use resolve::is_allowed_url;

use std::collections::BTreeMap;

use crate::config::{AgentKind, Config};
use crate::secret::Secret;
use crate::task::handoff::Role;
use resolve::resolve;

/// A value `"secret:<name>"` in `env` is read from a saved secret.
pub const SECRET_PREFIX: &str = "secret:";

/// A web server becomes this program with the argument [`BRIDGE_ARG`]: the
/// adapters replace it with the path of the running `harness`.
pub const BRIDGE_COMMAND: &str = "harness";
/// The argument after [`BRIDGE_COMMAND`] that starts the web bridge (`harness mcp-remote`).
pub const BRIDGE_ARG: &str = "mcp-remote";
/// The bridge reads the address from this variable and each header, as
/// `Name: value`, from `HARNESS_MCP_HEADER_1`, `_2`, ...
pub const BRIDGE_URL: &str = "HARNESS_MCP_URL";
/// The start of the variable names that pass headers to the bridge; a number follows.
pub const BRIDGE_HEADER: &str = "HARNESS_MCP_HEADER_";
/// A server with `auth = "oauth"` asks the secret function for
/// `oauth/<server> <url>`: the access token of the sign-in, still valid.
pub const OAUTH_SECRET: &str = "oauth/";

/// One server as an agent starts it, with every secret already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// The server's name from `harness.toml`, such as `context7` in `[mcp.context7]`.
    pub name: String,
    /// The program to start; for a web server this is [`BRIDGE_COMMAND`].
    pub command: String,
    /// The arguments for the program, in order.
    pub args: Vec<String>,
    /// All values are kept as secrets, so none of them is printed by accident.
    pub env: BTreeMap<String, Secret>,
}

/// The servers of every role in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServers {
    roles: BTreeMap<Role, Vec<McpServer>>,
}

/// A problem with the MCP servers in `harness.toml`, found before any agent starts.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// A server name with characters that are not allowed.
    #[error(
        "MCP server name {0:?} is not allowed; use lowercase letters, digits, '-' and '_', \
         for example \"context7\""
    )]
    BadServerName(String),
    /// A role lists a server that has no `[mcp.<name>]` section.
    #[error("the {role:?} role uses MCP server {name:?}, but harness.toml has no [mcp.{name}]")]
    UnknownServer {
        /// The role whose `mcp` list names the server.
        role: Role,
        /// The server name as the role lists it.
        name: String,
    },
    /// A program server whose `command` is empty or missing.
    #[error("MCP server {0:?} has an empty command")]
    EmptyCommand(String),
    /// A server that sets both `command` and `url`.
    #[error("MCP server {0:?} has both a command and a url; it is one or the other")]
    CommandAndUrl(String),
    /// A web address that is not `https://` (or `http://` to this computer).
    #[error(
        "MCP server {server:?}: url {url:?} is not allowed; use https:// \
         (http:// only for localhost)"
    )]
    BadUrl {
        /// The server name from `harness.toml`.
        server: String,
        /// The address that was refused.
        url: String,
    },
    /// A web server that also sets `args` or `env`.
    #[error("MCP server {0:?} is on the web (url): it takes headers, not args or env")]
    WebServerParts(String),
    /// A header name in `headers` that is not allowed.
    #[error("MCP server {server:?}: header {name:?} is not allowed")]
    BadHeader {
        /// The server name from `harness.toml`.
        server: String,
        /// The refused header name.
        name: String,
    },
    /// `auth = "oauth"` on a program server or next to an `Authorization` header.
    #[error(
        "MCP server {0:?}: auth = \"oauth\" is only for a server on the web (url) \
         without its own Authorization header"
    )]
    BadAuth(String),
    /// An OAuth server with no saved, valid sign-in.
    #[error("MCP server {0:?} needs a sign-in: run `harness mcp login {0}`")]
    NotSignedIn(String),
    /// A variable name in `env` with characters that are not allowed.
    #[error(
        "MCP server {server:?}: variable name {name:?} is not allowed; \
         use capital letters, digits and '_', for example API_KEY"
    )]
    BadVariable {
        /// The server name from `harness.toml`.
        server: String,
        /// The refused variable name.
        name: String,
    },
    /// A variable in `env` that only the harness or the agents may set.
    #[error("MCP server {server:?}: variable {name:?} is reserved for the harness and the agents")]
    ReservedVariable {
        /// The server name from `harness.toml`.
        server: String,
        /// The reserved variable name.
        name: String,
    },
    /// Two servers of one role set the same variable.
    #[error(
        "the {role:?} role gets variable {name:?} from two MCP servers; \
         give them different names"
    )]
    VariableTwice {
        /// The role that uses both servers.
        role: Role,
        /// The variable name both servers set.
        name: String,
    },
    /// A `secret:<name>` value whose name is not allowed.
    #[error("MCP server {server:?}: secret name {name:?} is not allowed")]
    BadSecretName {
        /// The server name from `harness.toml`.
        server: String,
        /// The refused secret name, without `secret:`.
        name: String,
    },
    /// A `secret:<name>` value whose secret has not been saved.
    #[error(
        "MCP server {server:?} needs the secret {name:?}; save it with `harness secret set {name}`"
    )]
    MissingSecret {
        /// The server name from `harness.toml`.
        server: String,
        /// The name of the missing secret, without `secret:`.
        name: String,
    },
}

/// Variables an MCP server may not set (`harness_platform::env::is_reserved`,
/// the agents read them themselves), and these prefixes: a server setting
/// could change the agent or leak into it.
const RESERVED_PREFIXES: &[&str] = &[
    "CLAUDE_",
    "ANTHROPIC_",
    "CODEX_",
    "OPENAI_",
    "DEEPSEEK_",
    "AGY_",
    "GEMINI_",
    "GOOGLE_",
    "HARNESS_",
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
/// program server (`stdio`) the same way, so for now this is every agent;
/// servers some agent cannot start would be refused here.
pub fn agent_runs(agent: AgentKind, _server: &crate::config::McpConfig) -> bool {
    // No `_` arm: a new agent must be checked here before it gets servers.
    match agent {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Antigravity | AgentKind::Dsh => true,
    }
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
    if harness_platform::env::is_reserved(name)
        || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p))
    {
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

    #[test]
    fn a_web_server_becomes_the_bridge_with_its_headers_and_secrets() {
        let config = Config::parse(
            "[roles.developer]\nagent = \"codex\"\nmcp = [\"github\"]\n\
             [mcp.github]\nurl = \"https://api.githubcopilot.com/mcp/\"\n\
             headers = { Authorization = \"Bearer secret:github\", X-Toolsets = \"repos\" }\n",
        )
        .unwrap();
        let servers =
            McpServers::load(&config, |n| (n == "github").then(|| Secret::new("ghp_x"))).unwrap();
        let github = &servers.for_role(Role::Developer)[0];
        assert_eq!(github.command, BRIDGE_COMMAND);
        assert_eq!(github.args, [BRIDGE_ARG]);
        assert_eq!(
            github.env[BRIDGE_URL].expose(),
            "https://api.githubcopilot.com/mcp/"
        );
        assert_eq!(
            github.env["HARNESS_MCP_HEADER_1"].expose(),
            "Authorization: Bearer ghp_x"
        );
        assert_eq!(
            github.env["HARNESS_MCP_HEADER_2"].expose(),
            "X-Toolsets: repos"
        );
        assert!(!format!("{github:?}").contains("ghp_x"));
    }

    #[test]
    fn web_servers_are_checked() {
        let check = |toml: &str| {
            let config: crate::config::McpConfig = toml::from_str(toml).unwrap();
            check_server("web", &config)
        };
        check("url = \"https://example.com/mcp\"").unwrap();
        check("url = \"http://127.0.0.1:8080/mcp\"").unwrap();
        assert!(matches!(
            check("url = \"http://example.com/mcp\""),
            Err(McpError::BadUrl { .. })
        ));
        assert!(matches!(
            check("url = \"https://\""),
            Err(McpError::BadUrl { .. })
        ));
        assert!(matches!(
            check("url = \"https://a.b\"\ncommand = \"npx\""),
            Err(McpError::CommandAndUrl(_))
        ));
        assert!(matches!(
            check("url = \"https://a.b\"\nenv = { KEY = \"1\" }"),
            Err(McpError::WebServerParts(_))
        ));
        assert!(matches!(
            check("url = \"https://a.b\"\nheaders = { \"Bad Name\" = \"1\" }"),
            Err(McpError::BadHeader { .. })
        ));
        assert!(matches!(
            check("url = \"https://a.b\"\nheaders = { X = \"1\\nEvil: 2\" }"),
            Err(McpError::BadHeader { .. })
        ));
        assert!(matches!(
            check("url = \"https://a.b\"\nheaders = { Mcp-Session-Id = \"1\" }"),
            Err(McpError::BadHeader { .. })
        ));
        assert!(matches!(
            check("command = \"npx\"\nheaders = { X = \"1\" }"),
            Err(McpError::BadHeader { .. })
        ));
        // A secret with a line break is refused too, without showing it.
        let config: crate::config::McpConfig =
            toml::from_str("url = \"https://a.b\"\nheaders = { X = \"secret:k\" }").unwrap();
        let error = server("web", &config, |_| Some(Secret::new("a\nb"))).unwrap_err();
        assert!(matches!(error, McpError::BadHeader { .. }));
    }

    #[test]
    fn a_signed_in_server_gets_its_access_token() {
        let config: crate::config::McpConfig =
            toml::from_str("url = \"https://mcp.example.com/mcp\"\nauth = \"oauth\"").unwrap();
        let asked = std::cell::RefCell::new(Vec::new());
        let server = server("notion", &config, |key| {
            asked.borrow_mut().push(key.to_string());
            Some(Secret::new("at-1"))
        })
        .unwrap();
        assert_eq!(
            asked.borrow()[0],
            "oauth/notion https://mcp.example.com/mcp"
        );
        assert_eq!(
            server.env["HARNESS_MCP_HEADER_1"].expose(),
            "Authorization: Bearer at-1"
        );
        assert!(matches!(
            super::server("notion", &config, |_| None),
            Err(McpError::NotSignedIn(_))
        ));
        let bad = |toml: &str| {
            let config: crate::config::McpConfig = toml::from_str(toml).unwrap();
            matches!(check_server("x", &config), Err(McpError::BadAuth(_)))
        };
        assert!(bad("command = \"npx\"\nauth = \"oauth\""));
        // Any other kind of sign-in is not even read.
        toml::from_str::<crate::config::McpConfig>("url = \"https://a.b\"\nauth = \"basic\"")
            .unwrap_err();
        assert!(bad(
            "url = \"https://a.b\"\nauth = \"oauth\"\nheaders = { authorization = \"x\" }"
        ));
    }
}
