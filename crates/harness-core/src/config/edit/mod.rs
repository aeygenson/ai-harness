//! Changes to harness.toml made by commands such as `harness plugin add`.
//!
//! The file is Lisa's: it has her comments and her order of settings. So it is
//! edited with `toml_edit`, which changes only the lines that must change and
//! keeps everything else as it was. After every change the result is checked
//! with the normal `Config::parse`.

mod mcp;

pub use mcp::{remove_mcp, set_mcp};

use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::config::{AgentKind, Config, RetroConfig, RoleConfig};
use crate::task::handoff::Role;

/// Why a change to `harness.toml` could not be made; the file is then left unchanged.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The file is not valid TOML, so it cannot be edited.
    #[error("harness.toml is not valid TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    /// A plugin with this name is already in the file.
    #[error("harness.toml already has [plugins.{0}]")]
    PluginExists(String),
    /// No plugin with this name is in the file.
    #[error("harness.toml has no [plugins.{0}]")]
    NoPlugin(String),
    /// No MCP server with this name is in the file.
    #[error("harness.toml has no [mcp.{0}]")]
    NoMcp(String),
    /// An MCP server with this name is already in the file.
    #[error("harness.toml already has [mcp.{0}]")]
    McpExists(String),
    /// The file has no settings for this role.
    #[error("harness.toml has no [roles.{0}]")]
    NoRole(String),
    /// The key (such as `roles.developer`) holds a plain value or list of the wrong kind,
    /// not the table or list the change needs.
    #[error("`{0}` in harness.toml is not a table")]
    NotATable(String),
    /// The change would leave a file that `Config::parse` rejects.
    #[error("the changed harness.toml would not be valid: {0}")]
    Invalid(#[from] toml::de::Error),
}

/// A new `[plugins.<name>]`, as `harness plugin add` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlugin<'a> {
    /// The plugin name, used as the key in `[plugins.<name>]`.
    pub name: &'a str,
    /// Whose plugin it is: Claude Code or Codex.
    pub agent: AgentKind,
    /// Where it was copied from, as `<catalog>/<plugin>`.
    pub source: &'a str,
    /// The git commit it was copied at; `None` writes no `commit` key.
    pub commit: Option<&'a str>,
    /// Writes `allow_hooks = true`, letting the plugin's hooks run commands.
    pub allow_hooks: bool,
    /// Writes `allow_mcp = true`, letting the plugin start its own servers.
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
    entry.insert("agent", value(plugin.agent.as_str()));
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
        let key = role.as_str();
        let roles = table(&mut doc, "roles")?;
        let role_table = roles
            .get_mut(key)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| EditError::NoRole(key.to_string()))?;
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

/// Sets `allow_hooks` and `allow_mcp` of an existing `[plugins.<name>]`; a
/// `false` is written by leaving the key out, as `harness plugin add` does.
pub fn set_plugin_allow(
    text: &str,
    name: &str,
    allow_hooks: bool,
    allow_mcp: bool,
) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let entry = table(&mut doc, "plugins")?
        .get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::NoPlugin(name.to_string()))?;
    for (key, on) in [("allow_hooks", allow_hooks), ("allow_mcp", allow_mcp)] {
        if on {
            entry.insert(key, value(true));
        } else {
            entry.remove(key);
        }
    }
    finish(doc)
}

/// Adds `skill` to a role's `skills` list, or to `always_skills` if `always`.
/// A skill already in that list is not added twice.
pub fn add_role_skill(
    text: &str,
    role: Role,
    skill: &str,
    always: bool,
) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let key = role.as_str();
    let list_name = if always { "always_skills" } else { "skills" };
    let list = table(&mut doc, "roles")?
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::NoRole(key.to_string()))?
        .entry(list_name)
        .or_insert(value(Array::new()))
        .as_array_mut()
        .ok_or_else(|| EditError::NotATable(format!("roles.{key}.{list_name}")))?;
    if !list.iter().any(|item| item.as_str() == Some(skill)) {
        list.push(skill);
    }
    finish(doc)
}

/// Sets everything of `[roles.<role>]`: agent, model and the four lists.
/// The role's table is created if missing; values that did not change keep
/// their lines and comments. An empty list or no model removes the key.
pub fn set_role(text: &str, role: Role, settings: &RoleConfig) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let key = role.as_str();
    let roles = table(&mut doc, "roles")?;
    let entry = roles
        .entry(key)
        .or_insert_with(|| Item::Table(Table::new()));
    let role_table = entry
        .as_table_mut()
        .ok_or_else(|| EditError::NotATable(format!("roles.{key}")))?;
    set_text(role_table, "agent", Some(settings.agent.as_str()));
    set_text(role_table, "model", settings.model.as_deref());
    set_text(role_table, "effort", settings.effort.as_deref());
    set_list(role_table, "skills", &settings.skills);
    set_list(role_table, "always_skills", &settings.always_skills);
    set_list(role_table, "mcp", &settings.mcp);
    set_list(role_table, "plugins", &settings.plugins);
    finish(doc)
}

/// Sets `[retro]`: the agent and model of `harness retro --suggest`.
pub fn set_retro(text: &str, retro: &RetroConfig) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let entry = doc
        .entry("retro")
        .or_insert_with(|| Item::Table(Table::new()));
    let retro_table = entry
        .as_table_mut()
        .ok_or_else(|| EditError::NotATable("retro".into()))?;
    set_text(retro_table, "agent", Some(retro.agent.as_str()));
    set_text(retro_table, "model", retro.model.as_deref());
    set_text(retro_table, "effort", retro.effort.as_deref());
    finish(doc)
}

/// Sets `key` to `text`, or removes it for `None`. An unchanged value is left
/// alone, with its comment.
fn set_text(table: &mut Table, key: &str, text: Option<&str>) {
    match text {
        Some(text) if table.get(key).and_then(Item::as_str) == Some(text) => {}
        Some(text) => {
            // Keep a comment at the end of the line, if there is one.
            let decor = table
                .get(key)
                .and_then(Item::as_value)
                .map(|v| v.decor().clone());
            let mut new = toml_edit::Value::from(text);
            if let Some(decor) = decor {
                *new.decor_mut() = decor;
            }
            table.insert(key, Item::Value(new));
        }
        None => {
            table.remove(key);
        }
    }
}

fn set_list(table: &mut Table, key: &str, items: &[String]) {
    let current: Option<Vec<&str>> = table
        .get(key)
        .and_then(Item::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect());
    let wanted: Vec<&str> = items.iter().map(String::as_str).collect();
    match current {
        Some(current) if current == wanted => {}
        None if wanted.is_empty() => {}
        _ if wanted.is_empty() => {
            table.remove(key);
        }
        _ => {
            table.insert(key, value(Array::from_iter(wanted)));
        }
    }
}

/// Removes `[plugins.<name>]` and the name from every role's `plugins` list.
pub fn remove_plugin(text: &str, name: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    if table(&mut doc, "plugins")?.remove(name).is_none() {
        return Err(EditError::NoPlugin(name.to_string()));
    }
    for_role_lists(&mut doc, "plugins", |list| {
        list.retain(|item| item.as_str() != Some(name));
    });
    finish(doc)
}

/// Calls `change` with the list `key` of every role that has one.
fn for_role_lists(doc: &mut DocumentMut, key: &str, mut change: impl FnMut(&mut Array)) {
    if let Some(roles) = doc.get_mut("roles").and_then(Item::as_table_mut) {
        for (_, role) in roles.iter_mut() {
            if let Some(list) = role
                .as_table_mut()
                .and_then(|role| role.get_mut(key))
                .and_then(Item::as_array_mut)
            {
                change(list);
            }
        }
    }
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

fn finish(doc: DocumentMut) -> Result<String, EditError> {
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpConfig;

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
            agent: AgentKind::Codex,
            source: "official/review",
            commit: Some("abc123"),
            allow_hooks: false,
            allow_mcp: false,
        }
    }

    #[test]
    fn plugins_are_allowed_hooks_and_servers_and_back() {
        let text = set_plugin_allow(TOML, "old", true, false).unwrap();
        assert!(text.contains("[plugins.old]\nagent = \"codex\"\nallow_hooks = true\n"));
        let config = Config::parse(&text).unwrap();
        assert!(config.plugins["old"].allow_hooks && !config.plugins["old"].allow_mcp);
        assert_eq!(set_plugin_allow(&text, "old", false, false).unwrap(), TOML);
        assert!(matches!(
            set_plugin_allow(TOML, "nope", true, true),
            Err(EditError::NoPlugin(_))
        ));
    }

    #[test]
    fn a_role_is_set_whole_and_the_rest_of_the_file_stays() {
        let mut developer = Config::parse(TOML).unwrap().roles[&Role::Developer].clone();
        // Nothing changed: the text stays exactly the same.
        assert_eq!(set_role(TOML, Role::Developer, &developer).unwrap(), TOML);

        developer.model = Some("gpt-5.5".into());
        developer.effort = Some("xhigh".into());
        developer.plugins.clear();
        developer.skills = vec!["rust-errors".into()];
        let text = set_role(TOML, Role::Developer, &developer).unwrap();
        assert!(text.starts_with("# Lisa's settings\n"), "{text}");
        assert!(text.contains("agent = \"codex\" # fast"), "{text}");
        assert!(!text.contains("plugins = "), "{text}");
        let config = Config::parse(&text).unwrap();
        let saved = &config.roles[&Role::Developer];
        assert_eq!(saved, &developer);

        developer.agent = AgentKind::Claude;
        developer.model = None;
        developer.effort = None;
        let text = set_role(&text, Role::Developer, &developer).unwrap();
        assert!(text.contains("agent = \"claude\" # fast"), "{text}");
        assert!(!text.contains("model"), "{text}");
        assert!(!text.contains("effort"), "{text}");

        // A role that is not in the file yet gets its table.
        let text = set_role(TOML, Role::Security, &developer).unwrap();
        assert_eq!(
            Config::parse(&text).unwrap().roles[&Role::Security],
            developer
        );
    }

    #[test]
    fn retro_is_created_or_changed() {
        let retro = RetroConfig {
            agent: AgentKind::Dsh,
            model: Some("deepseek-v4-pro".into()),
            effort: None,
        };
        let text = set_retro(TOML, &retro).unwrap();
        assert_eq!(Config::parse(&text).unwrap().retro.as_ref(), Some(&retro));
        let text = set_retro(
            &text,
            &RetroConfig {
                agent: AgentKind::Claude,
                model: None,
                effort: Some("high".into()),
            },
        )
        .unwrap();
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.retro.unwrap().model, None);
        assert!(text.starts_with("# Lisa's settings\n"));
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
    fn a_skill_is_added_to_a_role_once() {
        let text = add_role_skill(TOML, Role::Developer, "rust-errors", false).unwrap();
        let text = add_role_skill(&text, Role::Developer, "rust-errors", false).unwrap();
        let text = add_role_skill(&text, Role::Developer, "style", true).unwrap();
        assert!(text.contains("agent = \"codex\" # fast"), "{text}");
        let config = Config::parse(&text).unwrap();
        let developer = &config.roles[&Role::Developer];
        assert_eq!(developer.skills, ["rust-errors"]);
        assert_eq!(developer.always_skills, ["style"]);
        assert!(matches!(
            add_role_skill(TOML, Role::Security, "x", false),
            Err(EditError::NoRole(_))
        ));
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

    #[test]
    fn mcp_servers_are_added_changed_renamed_and_removed() {
        let text = "[roles.developer]\nagent = \"codex\"\nmcp = [\"docs\"]\n\n\
                    # Documentation search.\n[mcp.docs]\ncommand = \"npx\" # pinned\n\
                    args = [\"-y\", \"docs-mcp\"]\n";
        let server = |command: &str, args: &[&str], env: &[(&str, &str)]| McpConfig {
            command: Some(command.into()),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..McpConfig::default()
        };

        let added = set_mcp(
            text,
            None,
            "fetch",
            &server("uvx", &["mcp-server-fetch"], &[]),
        )
        .unwrap();
        let config = Config::parse(&added).unwrap();
        assert_eq!(config.mcp["fetch"].command.as_deref(), Some("uvx"));
        assert!(matches!(
            set_mcp(text, None, "docs", &server("x", &[], &[])),
            Err(EditError::McpExists(_))
        ));

        // A change keeps the comments; a variable is an inline table.
        let changed = set_mcp(
            text,
            Some("docs"),
            "docs",
            &server("npx", &["-y", "docs-mcp@2"], &[("API_KEY", "secret:docs")]),
        )
        .unwrap();
        assert!(changed.contains("# Documentation search."), "{changed}");
        assert!(changed.contains("command = \"npx\" # pinned"), "{changed}");
        assert!(
            changed.contains("env = { API_KEY = \"secret:docs\" }"),
            "{changed}"
        );

        // A new name is a rename, in the roles too.
        let renamed = set_mcp(text, Some("docs"), "manuals", &server("npx", &[], &[])).unwrap();
        let config = Config::parse(&renamed).unwrap();
        assert!(!config.mcp.contains_key("docs"));
        assert_eq!(config.roles[&Role::Developer].mcp, ["manuals"]);

        let removed = remove_mcp(text, "docs").unwrap();
        let config = Config::parse(&removed).unwrap();
        assert!(config.mcp.is_empty());
        assert!(config.roles[&Role::Developer].mcp.is_empty());
        assert!(matches!(remove_mcp(text, "nope"), Err(EditError::NoMcp(_))));
    }

    #[test]
    fn a_web_server_is_written_with_its_address_and_headers() {
        let text = "[mcp.docs]\ncommand = \"npx\"\nargs = [\"docs-mcp\"]\n";
        let web = McpConfig {
            url: Some("https://example.com/mcp".into()),
            headers: [(
                "Authorization".to_string(),
                "Bearer secret:docs".to_string(),
            )]
            .into(),
            ..McpConfig::default()
        };
        let text = set_mcp(text, Some("docs"), "docs", &web).unwrap();
        let config = crate::config::Config::parse(&text).unwrap();
        assert_eq!(config.mcp["docs"], web);
        assert!(!text.contains("command"), "{text}");
        assert!(
            text.contains("Authorization = \"Bearer secret:docs\""),
            "{text}"
        );
    }
}
