//! Adding, changing, renaming and removing MCP servers in `harness.toml`.

use std::collections::BTreeMap;

use toml_edit::{value, DocumentMut, Item, Table};

use super::{finish, for_role_lists, set_list, set_text, table, EditError};
use crate::config::McpConfig;

/// Writes `[mcp.<name>]`. With `old`, that server is changed: when the name
/// is different it is renamed, in the roles' `mcp` lists too. Without `old`
/// the server is new and must not exist yet.
pub fn set_mcp(
    text: &str,
    old: Option<&str>,
    name: &str,
    server: &McpConfig,
) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let servers = table(&mut doc, "mcp")?;
    let renamed = old.is_some_and(|old| old != name);
    if (old.is_none() || renamed) && servers.contains_key(name) {
        return Err(EditError::McpExists(name.to_string()));
    }
    // A changed server keeps its place and the comments above it.
    let mut entry = match old {
        Some(old) => servers
            .remove(old)
            .and_then(|item| item.into_table().ok())
            .ok_or_else(|| EditError::NoMcp(old.to_string()))?,
        None => Table::new(),
    };
    let command = Some(server.command.as_str()).filter(|c| !c.is_empty());
    set_text(&mut entry, "command", command);
    if server.args.is_empty() {
        entry.remove("args");
    } else {
        set_list(&mut entry, "args", &server.args);
    }
    set_map(&mut entry, "env", &server.env);
    set_text(&mut entry, "url", server.url.as_deref());
    set_map(&mut entry, "headers", &server.headers);
    set_text(&mut entry, "auth", server.auth.as_deref());
    servers.insert(name, Item::Table(entry));
    if let (Some(old), true) = (old, renamed) {
        for_role_lists(&mut doc, "mcp", |list| {
            for item in list.iter_mut() {
                if item.as_str() == Some(old) {
                    *item = name.into();
                }
            }
        });
    }
    finish(doc)
}

/// `key = { NAME = "value", ... }`, left as it is when it already says that,
/// removed when `wanted` is empty.
pub(super) fn set_map(entry: &mut Table, key: &str, wanted: &BTreeMap<String, String>) {
    let current: Option<Vec<(String, String)>> = entry.get(key).and_then(|map| {
        map.as_table_like().map(|t| {
            t.iter()
                .filter_map(|(k, v)| Some((k.to_string(), v.as_str()?.to_string())))
                .collect()
        })
    });
    let wanted: Vec<(String, String)> =
        wanted.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if current.as_ref() == Some(&wanted) {
        return;
    }
    if wanted.is_empty() {
        entry.remove(key);
    } else {
        let mut map = toml_edit::InlineTable::new();
        for (name, text) in &wanted {
            map.insert(name, text.as_str().into());
        }
        entry.insert(key, value(map));
    }
}

/// Removes `[mcp.<name>]` and the name from every role's `mcp` list.
pub fn remove_mcp(text: &str, name: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    if table(&mut doc, "mcp")?.remove(name).is_none() {
        return Err(EditError::NoMcp(name.to_string()));
    }
    for_role_lists(&mut doc, "mcp", |list| {
        list.retain(|item| item.as_str() != Some(name));
    });
    finish(doc)
}
