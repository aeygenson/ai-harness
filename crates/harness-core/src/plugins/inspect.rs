//! Reading a plugin's manifest to see what it brings: hooks, servers, apps,
//! skills, commands and subagents.

use std::fs;
use std::path::Path;

use super::{manifest, PluginError};
use crate::config::AgentKind;

/// What a plugin folder contains that runs or reaches out by itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contents {
    /// Hooks: commands run on events.
    pub hooks: bool,
    /// Its own MCP or LSP servers.
    pub servers: bool,
    /// Codex apps (ChatGPT connectors).
    pub apps: bool,
}

/// Reads the manifest of `agent`'s plugin in `path` and looks at what the
/// plugin brings. `name` is only for messages.
pub fn inspect(path: &Path, name: &str, agent: AgentKind) -> Result<Contents, PluginError> {
    let manifest = read_manifest(path, name, agent)?;
    Ok(contents_of(path, &manifest))
}

/// What the Plugins tab shows about a plugin folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    /// Whether it has hooks, its own servers or apps.
    pub contents: Contents,
    /// `description` and `version` from its manifest.
    pub description: Option<String>,
    /// The plugin's version text; `None` when the manifest has none.
    pub version: Option<String>,
    /// Skills (`skills/<name>/SKILL.md`), commands (`commands/*.md`) and
    /// subagents (`agents/*.md`).
    pub skills: usize,
    /// How many command files (`commands/*.md`) it has.
    pub commands: usize,
    /// How many subagent files (`agents/*.md`) it has.
    pub agents: usize,
}

/// Reads what `agent`'s plugin in `path` brings, for showing it.
pub fn describe(path: &Path, name: &str, agent: AgentKind) -> Result<Details, PluginError> {
    let manifest = read_manifest(path, name, agent)?;
    let text = |key: &str| {
        manifest
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let count = |dir: &str, wanted: &dyn Fn(&Path) -> bool| {
        fs::read_dir(path.join(dir)).map_or(0, |entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| wanted(&entry.path()))
                .count()
        })
    };
    let markdown = |p: &Path| p.is_file() && p.extension().is_some_and(|e| e == "md");
    Ok(Details {
        contents: contents_of(path, &manifest),
        description: text("description"),
        version: text("version"),
        skills: count("skills", &|p| p.join("SKILL.md").is_file()),
        commands: count("commands", &markdown),
        agents: count("agents", &markdown),
    })
}

pub(super) fn read_manifest(
    path: &Path,
    name: &str,
    agent: AgentKind,
) -> Result<serde_json::Value, PluginError> {
    let manifest_path = path.join(manifest(agent));
    let text = fs::read_to_string(&manifest_path).map_err(|_missing| PluginError::NoManifest {
        name: name.to_string(),
        path: path.display().to_string(),
        manifest: manifest(agent),
        agent,
    })?;
    serde_json::from_str(&text)
        .ok()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| PluginError::BadManifest {
            name: name.to_string(),
            path: manifest_path.display().to_string(),
        })
}

pub(super) fn contents_of(path: &Path, manifest: &serde_json::Value) -> Contents {
    Contents {
        hooks: path.join("hooks/hooks.json").exists() || manifest.get("hooks").is_some(),
        servers: path.join(".mcp.json").exists()
            || path.join(".lsp.json").exists()
            || manifest.get("mcpServers").is_some()
            || manifest.get("lspServers").is_some(),
        apps: path.join(".app.json").exists() || manifest.get("apps").is_some(),
    }
}
