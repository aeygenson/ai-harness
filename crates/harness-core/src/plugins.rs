//! Agent plugins: ready-made packs of skills, commands and subagents.
//!
//! A plugin is a folder kept in the project, by default `.harness/plugins/<name>/`,
//! so the project carries everything its agents may use. `harness.toml` names
//! each plugin once and lists it in the roles that need it:
//!
//! ```toml
//! [plugins.rust-review]
//! agent = "claude"
//!
//! [roles.security]
//! agent = "claude"
//! plugins = ["rust-review"]
//! ```
//!
//! Plugins differ between agents, so each one says which agent loads it; for
//! now only Claude Code plugins are supported (`.claude-plugin/plugin.json`).
//!
//! Two parts of a plugin run programs by themselves: hooks (commands on
//! events) and MCP or LSP servers. They could get around the harness's own
//! rules, so a plugin that has them is refused unless its settings allow them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::{Config, PluginConfig};
use crate::handoff::Role;
use crate::mcp::is_simple_name;

/// Where plugins live by default, inside the project.
pub const PLUGINS_DIR: &str = ".harness/plugins";
/// The only agent with plugin support for now.
pub const CLAUDE: &str = "claude";
/// Claude Code's plugin manifest, inside the plugin folder.
const MANIFEST: &str = ".claude-plugin/plugin.json";

/// One plugin as the agent loads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    /// The plugin folder, absolute.
    pub path: PathBuf,
    /// Hooks are allowed to run.
    pub allow_hooks: bool,
}

/// The plugins of every role in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plugins {
    roles: BTreeMap<Role, Vec<Plugin>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error(
        "plugin name {0:?} is not allowed; use lowercase letters, digits, '-' and '_', \
         for example \"rust-review\""
    )]
    BadName(String),
    #[error("the {role:?} role uses plugin {name:?}, but harness.toml has no [plugins.{name}]")]
    Unknown { role: Role, name: String },
    #[error("plugin {name:?} is for agent {agent:?}; only \"claude\" plugins are supported")]
    UnsupportedAgent { name: String, agent: String },
    #[error(
        "the {role:?} role runs on {role_agent:?}, but plugin {name:?} is for {plugin_agent:?}"
    )]
    WrongAgent {
        role: Role,
        name: String,
        role_agent: String,
        plugin_agent: String,
    },
    #[error("plugin {name:?}: the path {path:?} must be a folder inside the project")]
    BadPath { name: String, path: String },
    #[error("plugin {name:?}: {path} has no {MANIFEST}; is it a Claude Code plugin?")]
    NoManifest { name: String, path: String },
    #[error("plugin {name:?}: {path} is not valid JSON")]
    BadManifest { name: String, path: String },
    #[error(
        "plugin {name:?} has hooks, which run commands by themselves; \
         add `allow_hooks = true` to [plugins.{name}] if you trust them"
    )]
    HooksNotAllowed { name: String },
    #[error(
        "plugin {name:?} starts its own MCP or LSP servers; \
         add `allow_mcp = true` to [plugins.{name}] if you trust them"
    )]
    ServersNotAllowed { name: String },
}

impl Plugins {
    /// No plugins for anyone.
    pub fn none() -> Self {
        Self::default()
    }

    /// Checks every plugin the roles list, in the project at `project_dir`.
    /// Plugins no role uses are not checked.
    pub fn load(project_dir: &Path, config: &Config) -> Result<Self, PluginError> {
        let mut roles = BTreeMap::new();
        for (&role, settings) in &config.roles {
            let mut plugins = Vec::new();
            for name in &settings.plugins {
                if !is_simple_name(name) {
                    return Err(PluginError::BadName(name.clone()));
                }
                let plugin = config
                    .plugins
                    .get(name)
                    .ok_or_else(|| PluginError::Unknown {
                        role,
                        name: name.clone(),
                    })?;
                if plugin.agent != CLAUDE {
                    return Err(PluginError::UnsupportedAgent {
                        name: name.clone(),
                        agent: plugin.agent.clone(),
                    });
                }
                if settings.agent != plugin.agent {
                    return Err(PluginError::WrongAgent {
                        role,
                        name: name.clone(),
                        role_agent: settings.agent.clone(),
                        plugin_agent: plugin.agent.clone(),
                    });
                }
                plugins.push(check(project_dir, name, plugin)?);
            }
            roles.insert(role, plugins);
        }
        Ok(Self { roles })
    }

    /// The plugins of one role; a role without plugins gets an empty list.
    pub fn for_role(&self, role: Role) -> Vec<Plugin> {
        self.roles.get(&role).cloned().unwrap_or_default()
    }
}

/// The plugin's folder relative to the project: its `path`, or the default.
pub fn relative_path(name: &str, plugin: &PluginConfig) -> String {
    plugin
        .path
        .clone()
        .unwrap_or_else(|| format!("{PLUGINS_DIR}/{name}"))
}

/// Finds the plugin folder and looks at what the plugin contains.
fn check(project_dir: &Path, name: &str, plugin: &PluginConfig) -> Result<Plugin, PluginError> {
    let relative = relative_path(name, plugin);
    let inside = Path::new(&relative)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if relative.is_empty() || !inside {
        return Err(PluginError::BadPath {
            name: name.to_string(),
            path: relative,
        });
    }
    let path = project_dir.join(&relative);
    let manifest_path = path.join(MANIFEST);
    let shown = manifest_path.display().to_string();
    let text = fs::read_to_string(&manifest_path).map_err(|_| PluginError::NoManifest {
        name: name.to_string(),
        path: path.display().to_string(),
    })?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .ok()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| PluginError::BadManifest {
            name: name.to_string(),
            path: shown,
        })?;

    let has_hooks = path.join("hooks/hooks.json").exists() || manifest.get("hooks").is_some();
    if has_hooks && !plugin.allow_hooks {
        return Err(PluginError::HooksNotAllowed {
            name: name.to_string(),
        });
    }
    let has_servers = path.join(".mcp.json").exists()
        || path.join(".lsp.json").exists()
        || manifest.get("mcpServers").is_some()
        || manifest.get("lspServers").is_some();
    if has_servers && !plugin.allow_mcp {
        return Err(PluginError::ServersNotAllowed {
            name: name.to_string(),
        });
    }
    Ok(Plugin {
        name: name.to_string(),
        path,
        allow_hooks: plugin.allow_hooks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project with one plugin folder `.harness/plugins/review/`.
    fn project(manifest: &str, extra: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join(PLUGINS_DIR).join("review");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(MANIFEST), manifest).unwrap();
        for (file, text) in extra {
            let path = plugin.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    fn load(dir: &Path, toml: &str) -> Result<Plugins, PluginError> {
        Plugins::load(dir, &Config::parse(toml).unwrap())
    }

    const MANIFEST_OK: &str = r#"{"name": "review", "description": "Reviews code"}"#;
    const ROLE: &str = "[roles.security]\nagent = \"claude\"\nplugins = [\"review\"]\n";

    #[test]
    fn a_role_gets_its_plugin_folder() {
        let dir = project(MANIFEST_OK, &[("skills/audit/SKILL.md", "---\n---\n")]);
        let plugins = load(
            dir.path(),
            &format!(
                "{ROLE}[roles.tester]\nagent = \"claude\"\n[plugins.review]\nagent = \"claude\"\n"
            ),
        )
        .unwrap();
        let security = plugins.for_role(Role::Security);
        assert_eq!(security.len(), 1);
        assert_eq!(security[0].path, dir.path().join(".harness/plugins/review"));
        assert!(!security[0].allow_hooks);
        assert!(plugins.for_role(Role::Tester).is_empty());
    }

    #[test]
    fn plugins_are_only_for_claude_roles() {
        let dir = project(MANIFEST_OK, &[]);
        let error = load(
            dir.path(),
            "[roles.tester]\nagent = \"codex\"\nplugins = [\"review\"]\n\
             [plugins.review]\nagent = \"claude\"\n",
        )
        .unwrap_err();
        assert!(matches!(error, PluginError::WrongAgent { .. }), "{error}");
        let error = load(
            dir.path(),
            "[roles.tester]\nagent = \"codex\"\nplugins = [\"review\"]\n\
             [plugins.review]\nagent = \"codex\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(error, PluginError::UnsupportedAgent { .. }),
            "{error}"
        );
    }

    #[test]
    fn missing_or_broken_plugins_are_refused() {
        let dir = project("not json", &[]);
        let config = format!("{ROLE}[plugins.review]\nagent = \"claude\"\n");
        assert!(matches!(
            load(dir.path(), &config),
            Err(PluginError::BadManifest { .. })
        ));
        assert!(matches!(
            load(
                dir.path(),
                "[roles.security]\nagent = \"claude\"\nplugins = [\"x\"]\n"
            ),
            Err(PluginError::Unknown { .. })
        ));
        let config = format!("{ROLE}[plugins.review]\nagent = \"claude\"\npath = \"elsewhere\"\n");
        assert!(matches!(
            load(dir.path(), &config),
            Err(PluginError::NoManifest { .. })
        ));
    }

    #[test]
    fn a_plugin_path_cannot_leave_the_project() {
        let dir = project(MANIFEST_OK, &[]);
        for path in ["../other", "/etc", "a/../../b", ""] {
            let config = format!("{ROLE}[plugins.review]\nagent = \"claude\"\npath = {path:?}\n");
            assert!(
                matches!(load(dir.path(), &config), Err(PluginError::BadPath { .. })),
                "{path}"
            );
        }
    }

    #[test]
    fn hooks_and_servers_need_permission() {
        type Files = &'static [(&'static str, &'static str)];
        let cases: [(&str, Files, &str); 5] = [
            (MANIFEST_OK, &[("hooks/hooks.json", "{}")], "allow_hooks"),
            (r#"{"name": "review", "hooks": {}}"#, &[], "allow_hooks"),
            (MANIFEST_OK, &[(".mcp.json", "{}")], "allow_mcp"),
            (r#"{"name": "review", "mcpServers": {}}"#, &[], "allow_mcp"),
            (r#"{"name": "review", "lspServers": {}}"#, &[], "allow_mcp"),
        ];
        for (manifest, extra, allow) in cases {
            let dir = project(manifest, extra);
            let config = format!("{ROLE}[plugins.review]\nagent = \"claude\"\n");
            let error = load(dir.path(), &config).unwrap_err().to_string();
            assert!(error.contains(allow), "{error}");
            let allowed = format!("{config}{allow} = true\n");
            let plugins = load(dir.path(), &allowed).unwrap();
            assert_eq!(
                plugins.for_role(Role::Security)[0].allow_hooks,
                allow == "allow_hooks"
            );
        }
    }
}
