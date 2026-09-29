//! Changes to harness.toml made by commands such as `harness plugin add`.
//!
//! The file is Lisa's: it has her comments and her order of settings. So it is
//! edited with `toml_edit`, which changes only the lines that must change and
//! keeps everything else as it was. After every change the result is checked
//! with the normal `Config::parse`.

use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::config::Config;
use crate::handoff::Role;

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("harness.toml is not valid TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    #[error("harness.toml already has [plugins.{0}]")]
    PluginExists(String),
    #[error("harness.toml has no [plugins.{0}]")]
    NoPlugin(String),
    #[error("harness.toml has no [roles.{0}]")]
    NoRole(String),
    #[error("`{0}` in harness.toml is not a table")]
    NotATable(String),
    #[error("the changed harness.toml would not be valid: {0}")]
    Invalid(#[from] toml::de::Error),
}

/// A new `[plugins.<name>]`, as `harness plugin add` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlugin<'a> {
    pub name: &'a str,
    pub agent: &'a str,
    pub source: &'a str,
    pub commit: Option<&'a str>,
    pub allow_hooks: bool,
    pub allow_mcp: bool,
}

/// Adds `[plugins.<name>]` and, if `role` is given, the name to that role's
/// `plugins` list.
pub fn add_plugin(text: &str, plugin: &NewPlugin, role: Option<Role>) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let plugins = table(&mut doc, "plugins")?;
    if plugins.contains_key(plugin.name) {
        return Err(EditError::PluginExists(plugin.name.to_string()));
    }
    let mut entry = Table::new();
    entry.insert("agent", value(plugin.agent));
    entry.insert("source", value(plugin.source));
    if let Some(commit) = plugin.commit {
        entry.insert("commit", value(commit));
    }
    if plugin.allow_hooks {
        entry.insert("allow_hooks", value(true));
    }
    if plugin.allow_mcp {
        entry.insert("allow_mcp", value(true));
    }
    plugins.insert(plugin.name, Item::Table(entry));
    if let Some(role) = role {
        let key = role_key(role);
        let roles = table(&mut doc, "roles")?;
        let role_table = roles
            .get_mut(&key)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| EditError::NoRole(key.clone()))?;
        let list = role_table
            .entry("plugins")
            .or_insert(value(Array::new()))
            .as_array_mut()
            .ok_or_else(|| EditError::NotATable(format!("roles.{key}.plugins")))?;
        if !list.iter().any(|item| item.as_str() == Some(plugin.name)) {
            list.push(plugin.name);
        }
    }
    finish(doc)
}

/// Sets `commit` of an existing `[plugins.<name>]`.
pub fn set_plugin_commit(text: &str, name: &str, commit: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let entry = table(&mut doc, "plugins")?
        .get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::NoPlugin(name.to_string()))?;
    entry.insert("commit", value(commit));
    finish(doc)
}

/// Removes `[plugins.<name>]` and the name from every role's `plugins` list.
pub fn remove_plugin(text: &str, name: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    if table(&mut doc, "plugins")?.remove(name).is_none() {
        return Err(EditError::NoPlugin(name.to_string()));
    }
    if let Some(roles) = doc.get_mut("roles").and_then(Item::as_table_mut) {
        for (_, role) in roles.iter_mut() {
            if let Some(list) = role
                .as_table_mut()
                .and_then(|role| role.get_mut("plugins"))
                .and_then(Item::as_array_mut)
            {
                list.retain(|item| item.as_str() != Some(name));
            }
        }
    }
    finish(doc)
}

/// The top-level table `key`, created (without its own header line) if missing.
fn table<'a>(doc: &'a mut DocumentMut, key: &str) -> Result<&'a mut Table, EditError> {
    let item = doc.entry(key).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    item.as_table_mut()
        .ok_or_else(|| EditError::NotATable(key.to_string()))
}

fn role_key(role: Role) -> String {
    serde_json::to_value(role)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn finish(doc: DocumentMut) -> Result<String, EditError> {
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = "# Lisa's settings\n\
                        max_rounds = 5\n\n\
                        [roles.developer]\n\
                        agent = \"codex\" # fast\n\
                        plugins = [\"old\"]\n\n\
                        [roles.tester]\n\
                        agent = \"claude\"\n\n\
                        [plugins.old]\n\
                        agent = \"codex\"\n";

    fn plugin(name: &str) -> NewPlugin<'_> {
        NewPlugin {
            name,
            agent: "codex",
            source: "official/review",
            commit: Some("abc123"),
            allow_hooks: false,
            allow_mcp: false,
        }
    }

    #[test]
    fn adding_a_plugin_keeps_comments_and_can_give_it_to_a_role() {
        let text = add_plugin(TOML, &plugin("review"), Some(Role::Developer)).unwrap();
        assert!(text.starts_with("# Lisa's settings\n"), "{text}");
        assert!(text.contains("agent = \"codex\" # fast"), "{text}");
        let config = Config::parse(&text).unwrap();
        let review = &config.plugins["review"];
        assert_eq!(review.source.as_deref(), Some("official/review"));
        assert_eq!(review.commit.as_deref(), Some("abc123"));
        assert!(!review.allow_hooks);
        assert_eq!(config.roles[&Role::Developer].plugins, ["old", "review"]);

        // A role without a list gets one.
        let text = add_plugin(TOML, &plugin("review"), Some(Role::Tester)).unwrap();
        assert_eq!(
            Config::parse(&text).unwrap().roles[&Role::Tester].plugins,
            ["review"]
        );
    }

    #[test]
    fn adding_twice_or_to_a_missing_role_is_refused() {
        assert!(matches!(
            add_plugin(TOML, &plugin("old"), None),
            Err(EditError::PluginExists(_))
        ));
        assert!(matches!(
            add_plugin(TOML, &plugin("review"), Some(Role::Security)),
            Err(EditError::NoRole(_))
        ));
    }

    #[test]
    fn a_first_plugin_gets_a_plugins_table() {
        let text = add_plugin(
            "[roles.tester]\nagent = \"claude\"\n",
            &NewPlugin {
                allow_hooks: true,
                ..plugin("review")
            },
            None,
        )
        .unwrap();
        assert!(text.contains("[plugins.review]"), "{text}");
        assert!(Config::parse(&text).unwrap().plugins["review"].allow_hooks);
    }

    #[test]
    fn commit_is_updated_and_removal_cleans_the_roles() {
        let text = set_plugin_commit(TOML, "old", "def456").unwrap();
        assert_eq!(
            Config::parse(&text).unwrap().plugins["old"]
                .commit
                .as_deref(),
            Some("def456")
        );
        assert!(matches!(
            set_plugin_commit(TOML, "missing", "x"),
            Err(EditError::NoPlugin(_))
        ));

        let text = remove_plugin(TOML, "old").unwrap();
        let config = Config::parse(&text).unwrap();
        assert!(config.plugins.is_empty());
        assert!(config.roles[&Role::Developer].plugins.is_empty());
        assert!(text.contains("agent = \"codex\" # fast"), "{text}");
    }
}
