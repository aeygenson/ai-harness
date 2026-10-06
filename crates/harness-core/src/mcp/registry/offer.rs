//! What one registry entry offers to start: a package on npm, PyPI or a
//! container image, or an address on the web.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{local_name, Offer};
use crate::config::McpConfig;
use crate::mcp::SECRET_PREFIX;
use crate::text::safe_line;

/// A server on the web (`streamable-http`), reached through the bridge.
pub(super) fn remote_offer(registry_name: &str, remote: &Value) -> Option<Offer> {
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
pub(super) fn offer(registry_name: &str, package: &Value) -> Option<Offer> {
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
pub(super) fn arguments(list: &Value) -> Vec<String> {
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
