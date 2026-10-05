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

use std::collections::BTreeMap;

use crate::config::Config;
use crate::secret::Secret;
use crate::task::handoff::Role;

/// A value `"secret:<name>"` in `env` is read from a saved secret.
pub const SECRET_PREFIX: &str = "secret:";

/// A web server becomes this program with the argument [`BRIDGE_ARG`]: the
/// adapters replace it with the path of the running `harness`.
pub const BRIDGE_COMMAND: &str = "harness";
pub const BRIDGE_ARG: &str = "mcp-remote";
/// The bridge reads the address from this variable and each header, as
/// `Name: value`, from `HARNESS_MCP_HEADER_1`, `_2`, ...
pub const BRIDGE_URL: &str = "HARNESS_MCP_URL";
pub const BRIDGE_HEADER: &str = "HARNESS_MCP_HEADER_";
/// The only `auth` there is: a sign-in in the browser.
pub const AUTH_OAUTH: &str = "oauth";
/// A server with `auth = "oauth"` asks the secret function for
/// `oauth/<server> <url>`: the access token of the sign-in, still valid.
pub const OAUTH_SECRET: &str = "oauth/";

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
    #[error("MCP server {0:?} has both a command and a url; it is one or the other")]
    CommandAndUrl(String),
    #[error(
        "MCP server {server:?}: url {url:?} is not allowed; use https:// \
         (http:// only for localhost)"
    )]
    BadUrl { server: String, url: String },
    #[error("MCP server {0:?} is on the web (url): it takes headers, not args or env")]
    WebServerParts(String),
    #[error("MCP server {server:?}: header {name:?} is not allowed")]
    BadHeader { server: String, name: String },
    #[error(
        "MCP server {0:?}: auth may only be \"oauth\", for a server on the web (url) \
         without its own Authorization header"
    )]
    BadAuth(String),
    #[error("MCP server {0:?} needs a sign-in: run `harness mcp login {0}`")]
    NotSignedIn(String),
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

fn resolve(
    name: &str,
    config: &crate::config::McpConfig,
    secret: &impl Fn(&str) -> Option<Secret>,
) -> Result<McpServer, McpError> {
    if let Some(url) = &config.url {
        return resolve_web(name, config, url, secret);
    }
    if config.auth.is_some() {
        return Err(McpError::BadAuth(name.to_string()));
    }
    if !config.headers.is_empty() {
        return Err(McpError::BadHeader {
            server: name.to_string(),
            name: config.headers.keys().next().cloned().unwrap_or_default(),
        });
    }
    if config.command.trim().is_empty() {
        return Err(McpError::EmptyCommand(name.to_string()));
    }
    let mut env = BTreeMap::new();
    for (variable, value) in &config.env {
        check_variable(name, variable)?;
        let value = match value.strip_prefix(SECRET_PREFIX) {
            Some(secret_name) => read_secret(name, secret_name, secret)?,
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

fn read_secret(
    server: &str,
    secret_name: &str,
    secret: &impl Fn(&str) -> Option<Secret>,
) -> Result<Secret, McpError> {
    let secret_name = secret_name.trim();
    if !is_simple_name(secret_name) {
        return Err(McpError::BadSecretName {
            server: server.to_string(),
            name: secret_name.to_string(),
        });
    }
    secret(secret_name).ok_or_else(|| McpError::MissingSecret {
        server: server.to_string(),
        name: secret_name.to_string(),
    })
}

/// Is `url` an address the bridge may use: `https://`, or `http://` to this
/// computer (for tests and local servers)?
pub fn is_allowed_url(url: &str) -> bool {
    let plain = |rest: &str| {
        !rest.is_empty()
            && !rest.starts_with('/')
            && rest
                .chars()
                .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '<' | '>' | '`'))
    };
    if let Some(rest) = url.strip_prefix("https://") {
        return plain(rest);
    }
    url.strip_prefix("http://").is_some_and(|rest| {
        let host = rest.split(['/', '?']).next().unwrap_or_default();
        let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
        plain(rest) && matches!(host, "localhost" | "127.0.0.1" | "[::1]")
    })
}

/// A web server: the bridge program, with the address and the headers (their
/// secrets read) in its variables.
fn resolve_web(
    name: &str,
    config: &crate::config::McpConfig,
    url: &str,
    secret: &impl Fn(&str) -> Option<Secret>,
) -> Result<McpServer, McpError> {
    if !config.command.trim().is_empty() {
        return Err(McpError::CommandAndUrl(name.to_string()));
    }
    if !config.args.is_empty() || !config.env.is_empty() {
        return Err(McpError::WebServerParts(name.to_string()));
    }
    let url = url.trim();
    if !is_allowed_url(url) {
        return Err(McpError::BadUrl {
            server: name.to_string(),
            url: url.to_string(),
        });
    }
    let oauth = match config.auth.as_deref() {
        None => false,
        Some(AUTH_OAUTH)
            if !config
                .headers
                .keys()
                .any(|h| h.eq_ignore_ascii_case("authorization")) =>
        {
            true
        }
        Some(_) => return Err(McpError::BadAuth(name.to_string())),
    };
    let mut env = BTreeMap::from([(BRIDGE_URL.to_string(), Secret::new(url))]);
    for (index, (header, value)) in config.headers.iter().enumerate() {
        let bad = || McpError::BadHeader {
            server: name.to_string(),
            name: header.clone(),
        };
        let token = !header.is_empty()
            && header
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        // The bridge and a few headers belong to the protocol.
        let reserved = [
            "content-type",
            "accept",
            "content-length",
            "host",
            "mcp-session-id",
            "mcp-protocol-version",
        ]
        .contains(&header.to_ascii_lowercase().as_str());
        if !token || reserved {
            return Err(bad());
        }
        // `Bearer secret:github`: the secret's name is the rest of the value.
        let value = match value.split_once(SECRET_PREFIX) {
            Some((before, secret_name)) => {
                let key = read_secret(name, secret_name, secret)?;
                Secret::new(format!("{before}{}", key.expose()))
            }
            None => Secret::new(value.clone()),
        };
        // A line break would start another header.
        if value.expose().chars().any(char::is_control) {
            return Err(bad());
        }
        env.insert(
            format!("{BRIDGE_HEADER}{}", index + 1),
            Secret::new(format!("{header}: {}", value.expose().trim())),
        );
    }
    if oauth {
        let token = secret(&format!("{OAUTH_SECRET}{name} {url}"))
            .ok_or_else(|| McpError::NotSignedIn(name.to_string()))?;
        if token.expose().is_empty() || token.expose().chars().any(char::is_control) {
            return Err(McpError::NotSignedIn(name.to_string()));
        }
        env.insert(
            format!("{BRIDGE_HEADER}{}", config.headers.len() + 1),
            Secret::new(format!("Authorization: Bearer {}", token.expose())),
        );
    }
    Ok(McpServer {
        name: name.to_string(),
        command: BRIDGE_COMMAND.to_string(),
        args: vec![BRIDGE_ARG.to_string()],
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
        assert!(check("url = \"https://example.com/mcp\"").is_ok());
        assert!(check("url = \"http://127.0.0.1:8080/mcp\"").is_ok());
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
        assert!(bad("url = \"https://a.b\"\nauth = \"basic\""));
        assert!(bad(
            "url = \"https://a.b\"\nauth = \"oauth\"\nheaders = { authorization = \"x\" }"
        ));
    }
}
