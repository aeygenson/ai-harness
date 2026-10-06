//! The tools an MCP server offers, as it said itself when «Check» asked it
//! (the asking is in `harness_agents::mcp::check`).
//!
//! The answer is kept in `~/.harness/mcp/<name>-<fingerprint>.json`. The
//! fingerprint is made of the server's settings (command, arguments and
//! variables, with secrets only by name), so a changed server counts as not
//! checked yet.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::McpConfig;
use crate::skills::fingerprint;

const MCP_DIR: &str = "mcp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolList {
    pub server: String,
    /// When it was asked, in seconds since 1970.
    pub fetched: u64,
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    /// The first line of the server's description.
    #[serde(default)]
    pub description: Option<String>,
}

/// `<home>/mcp/<name>-<fingerprint>.json` (`home` is `~/.harness`).
pub fn cache_path(home: &Path, name: &str, server: &McpConfig) -> PathBuf {
    let mut settings = serde_json::json!([name, server.command, server.args, server.env]);
    // A web server's address and headers; a program server keeps its old key.
    if server.url.is_some() || !server.headers.is_empty() {
        settings = serde_json::json!([settings, server.url, server.headers]);
    }
    if server.auth.is_some() {
        settings = serde_json::json!([settings, server.auth]);
    }
    let key = fingerprint(&settings.to_string());
    home.join(MCP_DIR).join(format!("{name}-{key}.json"))
}

/// The list saved for exactly these settings, if any.
pub fn load(home: &Path, name: &str, server: &McpConfig) -> Option<ToolList> {
    let text = fs::read_to_string(cache_path(home, name, server)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(home: &Path, server: &McpConfig, list: &ToolList) -> io::Result<()> {
    let path = cache_path(home, &list.server, server);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(list).map_err(io::Error::other)?;
    fs::write(path, text + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_belongs_to_the_settings_it_was_asked_with() {
        let home = tempfile::tempdir().unwrap();
        let mut server = McpConfig {
            command: "npx".into(),
            args: vec!["docs-mcp".into()],
            env: [("KEY".to_string(), "secret:docs".to_string())].into(),
            ..McpConfig::default()
        };
        let list = ToolList {
            server: "docs".into(),
            fetched: 1,
            tools: vec![Tool {
                name: "search".into(),
                description: Some("Searches the docs.".into()),
            }],
        };
        save(home.path(), &server, &list).unwrap();
        assert_eq!(load(home.path(), "docs", &server), Some(list));
        server.args.push("--new".into());
        assert_eq!(load(home.path(), "docs", &server), None);
    }
}
