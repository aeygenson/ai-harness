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
//! Plugins differ between agents, so each one says which agent loads it:
//! `"claude"` for Claude Code plugins (`.claude-plugin/plugin.json`) or
//! `"codex"` for Codex plugins (`.codex-plugin/plugin.json`).
//!
//! Two parts of a plugin run programs by themselves: hooks (commands on
//! events) and MCP or LSP servers. They could get around the harness's own
//! rules, so a plugin that has them is refused unless its settings allow them.
//! A Codex plugin with apps (`.app.json`, ChatGPT connectors) is always
//! refused: apps reach services outside the project.
//!
//! The folder also holds `catalog` (plugin catalogs), `install` (copying a
//! plugin into a project) and `ops` (plugin changes shared by the CLI and TUI).

pub mod catalog;
pub mod install;
pub mod ops;

mod inspect;

pub use inspect::{describe, inspect, Contents, Details};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::{Config, PluginConfig};
use crate::mcp::is_simple_name;
use crate::task::handoff::Role;

/// Where plugins live by default, inside the project.
pub const PLUGINS_DIR: &str = ".harness/plugins";
/// Claude Code plugins.
pub const CLAUDE: &str = "claude";
/// Codex plugins.
pub const CODEX: &str = "codex";
/// The agents with plugin support.
const AGENTS: &[&str] = &[CLAUDE, CODEX];

/// The plugin manifest each agent looks for inside the plugin folder.
pub fn manifest(agent: &str) -> &'static str {
    if agent == CODEX {
        ".codex-plugin/plugin.json"
    } else {
        ".claude-plugin/plugin.json"
    }
}

/// The agent family a role runs on: an agent with a model inside it, such as
/// `"claude+glm"`, is still the first one.
pub fn family(role_agent: &str) -> &str {
    role_agent.split('+').next().unwrap_or(role_agent)
}

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
    #[error(
        "plugin {name:?} is for agent {agent:?}; only \"claude\" and \"codex\" plugins \
         are supported"
    )]
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
    #[error("plugin {name:?}: {path} has no {manifest}; is it a plugin for {agent:?}?")]
    NoManifest {
        name: String,
        path: String,
        manifest: &'static str,
        agent: String,
    },
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
    #[error(
        "plugin {name:?} has apps (ChatGPT connectors), which reach services outside \
         the project; they are not supported"
    )]
    AppsNotAllowed { name: String },
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
                if !AGENTS.contains(&plugin.agent.as_str()) {
                    return Err(PluginError::UnsupportedAgent {
                        name: name.clone(),
                        agent: plugin.agent.clone(),
                    });
                }
                if family(&settings.agent) != plugin.agent {
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

/// Copies a plugin folder, without its `.git`. Symbolic links and other
/// special files are refused: a link could point outside the project.
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            if entry.file_name() != ".git" {
                copy_dir(&entry.path(), &target)?;
            }
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(std::io::Error::other(format!(
                "{} is a link or a special file; plugins may hold only files and folders",
                entry.path().display()
            )));
        }
    }
    Ok(())
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
    let contents = inspect(&path, name, &plugin.agent)?;
    if contents.hooks && !plugin.allow_hooks {
        return Err(PluginError::HooksNotAllowed {
            name: name.to_string(),
        });
    }
    if contents.servers && !plugin.allow_mcp {
        return Err(PluginError::ServersNotAllowed {
            name: name.to_string(),
        });
    }
    if plugin.agent == CODEX && contents.apps {
        return Err(PluginError::AppsNotAllowed {
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

    /// A project with one Claude Code plugin folder `.harness/plugins/review/`.
    fn project(manifest_text: &str, extra: &[(&str, &str)]) -> tempfile::TempDir {
        project_for(CLAUDE, manifest_text, extra)
    }

    /// The same for any agent's plugin.
    fn project_for(agent: &str, manifest_text: &str, extra: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join(PLUGINS_DIR).join("review");
        let manifest_path = plugin.join(manifest(agent));
        fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        fs::write(manifest_path, manifest_text).unwrap();
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
    fn a_plugin_is_described_by_what_it_brings() {
        let dir = project(
            r#"{"name": "review", "description": "Reviews code", "version": "1.2.0"}"#,
            &[
                ("skills/audit/SKILL.md", "---\n---\n"),
                ("skills/notes.txt", "not a skill"),
                ("commands/review.md", "# review"),
                ("commands/fix.md", "# fix"),
                ("agents/checker.md", "# checker"),
                ("hooks/hooks.json", "{}"),
            ],
        );
        let path = dir.path().join(PLUGINS_DIR).join("review");
        let details = describe(&path, "review", CLAUDE).unwrap();
        assert_eq!(details.description.as_deref(), Some("Reviews code"));
        assert_eq!(details.version.as_deref(), Some("1.2.0"));
        assert_eq!(
            (details.skills, details.commands, details.agents),
            (1, 2, 1)
        );
        assert!(details.contents.hooks && !details.contents.servers);
        assert!(matches!(
            describe(&path, "review", CODEX),
            Err(PluginError::NoManifest { .. })
        ));
    }

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
    fn a_plugin_is_only_for_roles_on_its_agent() {
        let dir = project(MANIFEST_OK, &[]);
        for (role_agent, plugin_agent) in [("codex", "claude"), ("claude", "codex")] {
            let error = load(
                dir.path(),
                &format!(
                    "[roles.tester]\nagent = {role_agent:?}\nplugins = [\"review\"]\n\
                     [plugins.review]\nagent = {plugin_agent:?}\n"
                ),
            )
            .unwrap_err();
            assert!(matches!(error, PluginError::WrongAgent { .. }), "{error}");
        }
        let error = load(
            dir.path(),
            "[roles.tester]\nagent = \"antigravity\"\nplugins = [\"review\"]\n\
             [plugins.review]\nagent = \"antigravity\"\n",
        )
        .unwrap_err();
        assert!(
            matches!(error, PluginError::UnsupportedAgent { .. }),
            "{error}"
        );
    }

    #[test]
    fn codex_plugins_work_for_codex_roles() {
        let dir = project_for(
            CODEX,
            MANIFEST_OK,
            &[("skills/audit/SKILL.md", "---\n---\n")],
        );
        let plugins = load(
            dir.path(),
            "[roles.developer]\nagent = \"codex\"\nplugins = [\"review\"]\n\
             [plugins.review]\nagent = \"codex\"\n",
        )
        .unwrap();
        assert_eq!(plugins.for_role(Role::Developer).len(), 1);

        // A Claude Code manifest is not enough for Codex.
        let claude_only = project(MANIFEST_OK, &[]);
        let error = load(
            claude_only.path(),
            "[roles.developer]\nagent = \"codex\"\nplugins = [\"review\"]\n\
             [plugins.review]\nagent = \"codex\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(".codex-plugin/plugin.json"), "{error}");
    }

    #[test]
    fn codex_plugins_with_apps_are_refused() {
        let toml = "[roles.developer]\nagent = \"codex\"\nplugins = [\"review\"]\n\
                    [plugins.review]\nagent = \"codex\"\nallow_hooks = true\nallow_mcp = true\n";
        for (manifest_text, extra) in [
            (MANIFEST_OK, &[(".app.json", "{}")][..]),
            (r#"{"name": "review", "apps": "./apps"}"#, &[][..]),
        ] {
            let dir = project_for(CODEX, manifest_text, extra);
            assert!(matches!(
                load(dir.path(), toml),
                Err(PluginError::AppsNotAllowed { .. })
            ));
        }
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
