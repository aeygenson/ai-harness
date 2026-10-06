//! Turning a server from `harness.toml` into one ready to start: checking its
//! parts and reading its secrets. A web server becomes the bridge program.

use std::collections::BTreeMap;

use super::{
    check_variable, is_simple_name, McpError, McpServer, BRIDGE_ARG, BRIDGE_COMMAND, BRIDGE_HEADER,
    BRIDGE_URL, OAUTH_SECRET, SECRET_PREFIX,
};
use crate::config::{McpAuth, McpConfig};
use crate::secret::Secret;

pub(super) fn resolve(
    name: &str,
    config: &McpConfig,
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
    let command = match &config.command {
        Some(command) if !command.trim().is_empty() => command.clone(),
        _ => return Err(McpError::EmptyCommand(name.to_string())),
    };
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
        command,
        args: config.args.clone(),
        env,
    })
}

pub(super) fn read_secret(
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
pub(super) fn resolve_web(
    name: &str,
    config: &McpConfig,
    url: &str,
    secret: &impl Fn(&str) -> Option<Secret>,
) -> Result<McpServer, McpError> {
    if config.command.is_some() {
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
    let oauth = match config.auth {
        None => false,
        Some(McpAuth::OAuth) => {
            // The sign-in sends its own Authorization header.
            let has_own = config
                .headers
                .keys()
                .any(|h| h.eq_ignore_ascii_case("authorization"));
            if has_own {
                return Err(McpError::BadAuth(name.to_string()));
            }
            true
        }
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
