//! The official MCP registry (registry.modelcontextprotocol.io): servers to
//! choose from on the MCP tab instead of typing their command.
//!
//! The asking is in `harness_agents::mcp_registry`; here the answer is read
//! and an entry becomes a server for `harness.toml`. Only servers the agents
//! can start as a program (`stdio`) from npm (`npx`), PyPI (`uvx`) or a
//! container image (`docker`) can be used; servers that exist only on the web
//! are listed but cannot be chosen yet. The version is pinned, and a variable
//! the registry marks secret becomes `secret:<name>`, so no key is ever taken
//! from the registry or written to the project.
//!
//! Everything here comes from strangers: control characters are removed, and
//! the command is shown in the server form for Lisa to read before saving.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config::McpConfig;
use crate::mcp::{is_simple_name, SECRET_PREFIX};
use crate::text::safe_line;

pub const REGISTRY_URL: &str = "https://registry.modelcontextprotocol.io/v0/servers";

/// One server of the registry, as the catalog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The registry's name, such as `io.github.upstash/context7`.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub repository: Option<String>,
    /// How to start it, if the agents can.
    pub offer: Option<Offer>,
    /// Why it cannot be used, if it cannot.
    pub unusable: Option<String>,
}

/// A server ready for the form: a name for `harness.toml`, the settings and
/// what the registry says about each variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub server: McpConfig,
    /// `(variable, description)`, for the catalog's details.
    pub variables: Vec<(String, String)>,
    /// `npm`, `pypi` or `oci`.
    pub kind: String,
}

/// The servers of one answer of `GET /v0/servers`, each name once, those
/// that can be added first.
pub fn parse(answer: &str) -> Result<Vec<Entry>, String> {
    let value: Value =
        serde_json::from_str(answer).map_err(|_| "the registry did not answer with JSON")?;
    let servers = value["servers"]
        .as_array()
        .ok_or("the registry's answer has no servers")?;
    let mut entries: Vec<Entry> = Vec::new();
    for item in servers {
        let server = &item["server"];
        let Some(entry) = entry(server) else { continue };
        match entries.iter_mut().find(|e| e.name == entry.name) {
            // The usable one wins, then the one listed first.
            Some(seen) if seen.offer.is_none() && entry.offer.is_some() => *seen = entry,
            Some(_) => {}
            None => entries.push(entry),
        }
    }
    // The ones that can be added first, otherwise in the registry's order.
    entries.sort_by_key(|e| e.offer.is_none());
    Ok(entries)
}

fn entry(server: &Value) -> Option<Entry> {
    let name = safe_line(server["name"].as_str()?, 300);
    let text = |key: &str| {
        server[key]
            .as_str()
            .map(|t| safe_line(t, 300))
            .filter(|t| !t.is_empty())
    };
    let version = text("version").unwrap_or_default();
    let packages = server["packages"].as_array().cloned().unwrap_or_default();
    let remotes = server["remotes"].as_array().cloned().unwrap_or_default();
    // A program on this computer first, else the address on the web.
    let offer = packages
        .iter()
        .find_map(|p| offer(&name, p))
        .or_else(|| remotes.iter().find_map(|r| remote_offer(&name, r)));
    let unusable = match (&offer, packages.is_empty(), remotes.is_empty()) {
        (Some(_), _, _) => None,
        (None, true, false) => {
            Some("its web address uses the old SSE way, or has parts to fill in".to_string())
        }
        (None, _, _) => Some("no npm, PyPI or container package, and no web address".to_string()),
    };
    Some(Entry {
        title: text("title"),
        description: text("description"),
        version,
        repository: server["repository"]["url"]
            .as_str()
            .map(|t| safe_line(t, 300)),
        offer,
        unusable,
        name,
    })
}

/// A server on the web (`streamable-http`), reached through the bridge.
fn remote_offer(registry_name: &str, remote: &Value) -> Option<Offer> {
    if remote["type"].as_str()? != "streamable-http" {
        return None;
    }
    let url = safe_line(remote["url"].as_str()?, 300);
    if url.contains('{') || !crate::mcp::is_allowed_url(&url) {
        return None;
    }
    let name = local_name(registry_name);
    let mut headers = BTreeMap::new();
    let mut variables = Vec::new();
    let mut secrets = 0;
    for header in remote["headers"].as_array().into_iter().flatten() {
        let Some(header_name) = header["name"].as_str().map(|t| safe_line(t, 300)) else {
            continue;
        };
        let description = header["description"]
            .as_str()
            .map(|t| safe_line(t, 300))
            .unwrap_or_default();
        let given = header["value"]
            .as_str()
            .or_else(|| header["default"].as_str())
            .map(|t| safe_line(t, 300));
        let value = if header["isSecret"] == true {
            secrets += 1;
            let secret = if secrets == 1 {
                name.clone()
            } else {
                format!("{name}-{}", header_name.to_lowercase().replace('_', "-"))
            };
            let secret = format!("{SECRET_PREFIX}{secret}");
            // `Bearer {token}` keeps its `Bearer `.
            Some(match given.as_deref().and_then(|g| g.split_once('{')) {
                Some((before, _)) => format!("{before}{secret}"),
                None => secret,
            })
        } else if let Some(given) = given.filter(|g| !g.contains('{')) {
            Some(given)
        } else if header["isRequired"] == true {
            Some(String::new())
        } else {
            None
        };
        if let Some(value) = value {
            headers.insert(header_name.clone(), value);
        }
        variables.push((header_name, description));
    }
    Some(Offer {
        name,
        server: McpConfig {
            url: Some(url),
            headers,
            ..McpConfig::default()
        },
        variables,
        kind: "web".to_string(),
    })
}

/// How the agents would start one package, if they can.
fn offer(registry_name: &str, package: &Value) -> Option<Offer> {
    if package["transport"]["type"].as_str().unwrap_or("stdio") != "stdio" {
        return None;
    }
    let kind = package["registryType"].as_str()?.to_string();
    let identifier = safe_line(package["identifier"].as_str()?, 300);
    if identifier.is_empty() || identifier.starts_with('-') {
        return None;
    }
    let version = package["version"]
        .as_str()
        .map(|t| safe_line(t, 300))
        .filter(|v| !v.is_empty() && v != "latest");
    let name = local_name(registry_name);
    let runtime = arguments(&package["runtimeArguments"]);
    let mut variables = Vec::new();
    let mut env = BTreeMap::new();
    let mut secrets = 0;
    for variable in package["environmentVariables"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(var) = variable["name"].as_str().map(|t| safe_line(t, 300)) else {
            continue;
        };
        let description = variable["description"]
            .as_str()
            .map(|t| safe_line(t, 300))
            .unwrap_or_default();
        let value = if variable["isSecret"] == true {
            secrets += 1;
            let secret = if secrets == 1 {
                name.clone()
            } else {
                format!("{name}-{}", var.to_lowercase().replace('_', "-"))
            };
            Some(format!("{SECRET_PREFIX}{secret}"))
        } else if let Some(default) = variable["default"].as_str() {
            Some(safe_line(default, 300))
        } else if variable["isRequired"] == true {
            Some(String::new())
        } else {
            None
        };
        if let Some(value) = value {
            env.insert(var.clone(), value);
        }
        variables.push((var, description));
    }
    let (command, mut args) = match kind.as_str() {
        "npm" => {
            let mut args = runtime;
            if !args.iter().any(|a| a == "-y" || a == "--yes") {
                args.insert(0, "-y".into());
            }
            args.push(match &version {
                Some(v) => format!("{identifier}@{v}"),
                None => identifier.clone(),
            });
            ("npx", args)
        }
        "pypi" => {
            let mut args = runtime;
            args.push(match &version {
                Some(v) => format!("{identifier}=={v}"),
                None => identifier.clone(),
            });
            ("uvx", args)
        }
        "oci" => {
            let mut args: Vec<String> = vec!["run".into(), "-i".into(), "--rm".into()];
            for var in env.keys() {
                args.push("-e".into());
                args.push(var.clone());
            }
            args.extend(runtime);
            args.push(match &version {
                Some(v) if !identifier.contains(':') => format!("{identifier}:{v}"),
                _ => identifier.clone(),
            });
            ("docker", args)
        }
        _ => return None,
    };
    args.extend(arguments(&package["packageArguments"]));
    Some(Offer {
        name,
        server: McpConfig {
            command: command.into(),
            args,
            env,
            ..McpConfig::default()
        },
        variables,
        kind,
    })
}

/// Arguments as the registry lists them: a positional one is its value, a
/// named one `--name value`. A value the registry leaves open is `<name>`,
/// for Lisa to fill in the form.
fn arguments(list: &Value) -> Vec<String> {
    let mut args = Vec::new();
    for argument in list.as_array().into_iter().flatten() {
        let value = argument["value"]
            .as_str()
            .or_else(|| argument["default"].as_str())
            .map(|t| safe_line(t, 300));
        let hint = || {
            let name = argument["valueHint"]
                .as_str()
                .or_else(|| argument["name"].as_str())
                .unwrap_or("value");
            format!("<{}>", safe_line(name.trim_start_matches('-'), 300))
        };
        match argument["type"].as_str() {
            Some("named") => {
                let Some(name) = argument["name"].as_str().map(|t| safe_line(t, 300)) else {
                    continue;
                };
                if value.is_none() && argument["isRequired"] != true {
                    continue;
                }
                args.push(if name.starts_with('-') {
                    name
                } else {
                    format!("--{name}")
                });
                args.push(value.unwrap_or_else(hint));
            }
            _ => match value {
                Some(value) => args.push(value),
                None if argument["isRequired"] == true => args.push(hint()),
                None => {}
            },
        }
    }
    args
}

/// `io.github.upstash/context7` → `context7`: a name `harness.toml` takes.
pub fn local_name(registry_name: &str) -> String {
    let last = registry_name.rsplit('/').next().unwrap_or(registry_name);
    let mut name: String = last
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches(['-', '_']).replace("__", "_");
    name.truncate(48);
    if !name.is_empty() && !is_simple_name(&name) {
        name = format!("mcp-{name}");
    }
    if !is_simple_name(&name) {
        name = "server".into();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"servers": [
      {"server": {"name": "io.github.upstash/context7", "title": "Context7",
        "description": "Up-to-date code docs", "version": "4.1.1",
        "repository": {"url": "https://github.com/upstash/context7"},
        "packages": [{"registryType": "npm", "identifier": "@upstash/context7-mcp",
          "version": "4.1.1", "transport": {"type": "stdio"},
          "environmentVariables": [
            {"name": "CONTEXT7_API_KEY", "isSecret": true, "description": "API key"},
            {"name": "OPTIONAL", "description": "left out"},
            {"name": "MODE", "default": "fast"}]}],
        "remotes": [{"type": "streamable-http", "url": "https://mcp.context7.com/mcp"}]}},
      {"server": {"name": "io.github.upstash/context7", "version": "4.0.0"}},
      {"server": {"name": "ac.inference.sh/mcp", "description": "Web only\u001b[2J",
        "version": "1.0.0", "remotes": [{"type": "sse", "url": "https://x"}]}},
      {"server": {"name": "io.github.x/Files Server", "version": "latest",
        "packages": [{"registryType": "pypi", "identifier": "files-mcp", "version": "latest",
          "runtimeArguments": [{"type": "positional", "value": "--quiet"}],
          "packageArguments": [
            {"type": "named", "name": "allowed-directories", "isRequired": true},
            {"type": "named", "name": "--port", "default": "8080"},
            {"type": "named", "name": "optional"},
            {"type": "positional", "valueHint": "root", "isRequired": true}]}]}},
      {"server": {"name": "io.github.y/image", "version": "2.0",
        "packages": [{"registryType": "oci", "identifier": "ghcr.io/y/image", "version": "2.0",
          "environmentVariables": [{"name": "TOKEN", "isSecret": true},
                                   {"name": "OTHER_KEY", "isSecret": true}]}]}}
    ]}"#;

    #[test]
    fn registry_servers_become_settings_with_pinned_versions_and_secret_names() {
        let entries = parse(ANSWER).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "io.github.upstash/context7",
                "io.github.x/Files Server",
                "io.github.y/image",
                "ac.inference.sh/mcp"
            ]
        );

        let context7 = entries[0].offer.as_ref().unwrap();
        assert_eq!(context7.name, "context7");
        assert_eq!(context7.server.command, "npx");
        assert_eq!(context7.server.args, ["-y", "@upstash/context7-mcp@4.1.1"]);
        assert_eq!(context7.server.env["CONTEXT7_API_KEY"], "secret:context7");
        assert_eq!(context7.server.env["MODE"], "fast");
        assert!(!context7.server.env.contains_key("OPTIONAL"));
        assert_eq!(context7.variables.len(), 3);
        crate::mcp::check_server(&context7.name, &context7.server).unwrap();

        let web = &entries[3];
        assert!(web.offer.is_none());
        assert!(web.unusable.as_ref().unwrap().contains("web"));
        // The whole escape sequence goes, not only the ESC character.
        assert_eq!(web.description.as_deref(), Some("Web only"));

        let files = entries[1].offer.as_ref().unwrap();
        assert_eq!(files.name, "files-server");
        assert_eq!(files.server.command, "uvx");
        assert_eq!(
            files.server.args,
            [
                "--quiet",
                "files-mcp",
                "--allowed-directories",
                "<allowed-directories>",
                "--port",
                "8080",
                "<root>"
            ]
        );

        let image = entries[2].offer.as_ref().unwrap();
        assert_eq!(
            image.server.args,
            [
                "run",
                "-i",
                "--rm",
                "-e",
                "OTHER_KEY",
                "-e",
                "TOKEN",
                "ghcr.io/y/image:2.0"
            ]
        );
        assert_eq!(image.server.env["TOKEN"], "secret:image");
        assert_eq!(image.server.env["OTHER_KEY"], "secret:image-other-key");
    }

    #[test]
    fn registry_names_become_simple_names() {
        assert_eq!(local_name("io.github.upstash/context7"), "context7");
        assert_eq!(local_name("com.example/My_Server!"), "my_server");
        assert_eq!(local_name("x/__"), "server");
        assert_eq!(local_name("ai.smithery/9lives"), "9lives");
        assert!(parse("not json").is_err());
    }

    #[test]
    fn a_server_on_the_web_becomes_an_address_with_headers() {
        let answer = r#"{"servers": [
          {"server": {"name": "io.github.github/github-mcp-server", "version": "1.0.0",
            "remotes": [{"type": "streamable-http", "url": "https://api.githubcopilot.com/mcp/",
              "headers": [
                {"name": "Authorization", "value": "Bearer {token}", "isSecret": true,
                 "description": "A GitHub token"},
                {"name": "X-MCP-Toolsets", "value": "repos,issues"},
                {"name": "X-Optional", "description": "not needed"}]}]}},
          {"server": {"name": "x/templated", "version": "1",
            "remotes": [{"type": "streamable-http", "url": "https://{tenant}.example.com/mcp"}]}}
        ]}"#;
        let entries = parse(answer).unwrap();
        let github = entries[0].offer.as_ref().unwrap();
        assert_eq!(github.kind, "web");
        assert_eq!(github.name, "github-mcp-server");
        let server = &github.server;
        assert_eq!(
            server.url.as_deref(),
            Some("https://api.githubcopilot.com/mcp/")
        );
        assert_eq!(
            server.headers["Authorization"],
            "Bearer secret:github-mcp-server"
        );
        assert_eq!(server.headers["X-MCP-Toolsets"], "repos,issues");
        assert!(!server.headers.contains_key("X-Optional"));
        assert_eq!(github.variables.len(), 3);
        crate::mcp::check_server(&github.name, server).unwrap();
        assert!(entries[1].offer.is_none());
        assert!(entries[1].unusable.as_ref().unwrap().contains("fill in"));
    }
}
