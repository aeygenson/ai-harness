//! The manifest of a step: with what exactly a role worked.
//!
//! `manifest.json` lies in the step's folder next to `agent.log` and is
//! committed with the step. It records the harness's version, the agent with
//! its version, model and effort, and short fingerprints of the skills, MCP
//! servers and the prompt, plus the plugins' sources and commits. When one
//! task went well and the next went badly, comparing their manifests shows
//! what changed in between (a new agent version, an edited skill, ...).
//!
//! No secret goes in: MCP servers are fingerprinted by their command,
//! arguments and address only, never by their variables or headers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{AgentKind, Config};
use crate::skills::{fingerprint, RoleSkills};
use crate::task::handoff::Role;

/// The manifest's file name in the step folder.
pub const MANIFEST_FILE: &str = "manifest.json";

/// With what one role worked; saved as `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The harness's version, such as `0.3.0`.
    pub harness_version: String,
    /// The role that worked.
    pub role: Role,
    /// The role's agent; `None` when the run had no settings (tests).
    pub agent: Option<AgentKind>,
    /// What the agent's `--version` said, such as `2.1.300`; `None` if unknown.
    pub agent_version: Option<String>,
    /// The model; `None` means the agent's default.
    pub model: Option<String>,
    /// The effort level; `None` means the agent's default.
    pub effort: Option<String>,
    /// Every skill in the role's prompt or skill list, with its text's fingerprint.
    pub skills: Vec<Stamp>,
    /// The role's MCP servers, each with a fingerprint of its command and address.
    pub mcp: Vec<Stamp>,
    /// The role's plugins with where they came from.
    pub plugins: Vec<PluginStamp>,
    /// A fingerprint of the whole prompt.
    pub prompt: String,
}

/// A name with a short fingerprint of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The skill's or the server's name.
    pub name: String,
    /// See [`fingerprint`]: it changes when the content changes.
    pub fingerprint: String,
}

/// A plugin and the source it was installed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStamp {
    /// The plugin's name in `harness.toml`.
    pub name: String,
    /// The catalog or repository it came from, or its local folder.
    pub source: Option<String>,
    /// The commit it was installed at; `None` if not recorded.
    pub commit: Option<String>,
}

/// What a run knows about the project's agents besides the skills: the
/// settings from `harness.toml` and the agents' versions.
#[derive(Debug, Clone, Copy)]
pub struct AgentFacts<'a> {
    /// The project's settings; `None` when the run has none (tests).
    pub config: Option<&'a Config>,
    /// What each agent's `--version` said.
    pub versions: &'a BTreeMap<AgentKind, String>,
}

/// The manifest of `role` working with `skills` and `prompt`.
pub fn build(role: Role, agents: AgentFacts<'_>, skills: &RoleSkills, prompt: &str) -> Manifest {
    let settings = agents.config.and_then(|config| config.roles.get(&role));
    let agent = settings.map(|settings| settings.agent);
    let all_skills = skills
        .base
        .iter()
        .chain(&skills.always)
        .chain(&skills.on_demand);
    Manifest {
        harness_version: env!("CARGO_PKG_VERSION").to_string(),
        role,
        agent,
        agent_version: agent.and_then(|agent| agents.versions.get(&agent).cloned()),
        model: settings.and_then(|settings| settings.model.clone()),
        effort: settings.and_then(|settings| settings.effort.clone()),
        skills: all_skills
            .map(|skill| Stamp {
                name: skill.name.clone(),
                fingerprint: fingerprint(&skill.body),
            })
            .collect(),
        mcp: agents
            .config
            .map(|config| mcp_stamps(config, role))
            .unwrap_or_default(),
        plugins: agents
            .config
            .map(|config| plugin_stamps(config, role))
            .unwrap_or_default(),
        prompt: fingerprint(prompt),
    }
}

/// The role's MCP servers. Only the command, the arguments (which usually
/// pin the server's version, such as `pkg@1.2.0`) and the address count.
fn mcp_stamps(config: &Config, role: Role) -> Vec<Stamp> {
    let names = config
        .roles
        .get(&role)
        .map(|r| r.mcp.as_slice())
        .unwrap_or_default();
    names
        .iter()
        .filter_map(|name| {
            let server = config.mcp.get(name)?;
            let what = format!("{:?} {:?} {:?}", server.command, server.args, server.url);
            Some(Stamp {
                name: name.clone(),
                fingerprint: fingerprint(&what),
            })
        })
        .collect()
}

/// The role's plugins with their source and commit.
fn plugin_stamps(config: &Config, role: Role) -> Vec<PluginStamp> {
    let names = config
        .roles
        .get(&role)
        .map(|r| r.plugins.as_slice())
        .unwrap_or_default();
    names
        .iter()
        .filter_map(|name| {
            let plugin = config.plugins.get(name)?;
            Some(PluginStamp {
                name: name.clone(),
                source: plugin.source.clone().or_else(|| plugin.path.clone()),
                commit: plugin.commit.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::skills::{Skill, Source};

    const CONFIG: &str = r#"
[roles.developer]
agent = "claude"
model = "opus"
effort = "high"
mcp = ["context7"]
plugins = ["code-review"]

[mcp.context7]
command = "npx"
args = ["-y", "@upstash/context7-mcp@1.2.0"]
env = { CONTEXT7_API_KEY = "plain-key-123" }

[plugins.code-review]
agent = "claude"
source = "claude-plugins-official/code-review"
commit = "fbe07fb"
"#;

    fn skills() -> RoleSkills {
        RoleSkills {
            base: vec![Skill {
                name: "developer".into(),
                description: String::new(),
                path: PathBuf::from("developer.md"),
                body: "Implement the design.".into(),
                source: Source::BuiltIn,
            }],
            ..RoleSkills::default()
        }
    }

    #[test]
    fn the_manifest_names_the_agent_its_version_and_what_the_role_got() {
        let config = Config::parse(CONFIG).unwrap();
        let versions = BTreeMap::from([(AgentKind::Claude, "2.1.300".to_string())]);
        let agents = AgentFacts {
            config: Some(&config),
            versions: &versions,
        };

        let manifest = build(Role::Developer, agents, &skills(), "the prompt");

        assert_eq!(manifest.harness_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest.agent, Some(AgentKind::Claude));
        assert_eq!(manifest.agent_version.as_deref(), Some("2.1.300"));
        assert_eq!(manifest.model.as_deref(), Some("opus"));
        assert_eq!(manifest.effort.as_deref(), Some("high"));
        assert_eq!(manifest.skills[0].name, "developer");
        assert_eq!(
            manifest.skills[0].fingerprint,
            fingerprint("Implement the design.")
        );
        assert_eq!(manifest.mcp[0].name, "context7");
        assert_eq!(
            manifest.plugins,
            [PluginStamp {
                name: "code-review".into(),
                source: Some("claude-plugins-official/code-review".into()),
                commit: Some("fbe07fb".into()),
            }]
        );
        assert_eq!(manifest.prompt, fingerprint("the prompt"));
    }

    #[test]
    fn a_new_server_version_changes_its_fingerprint_but_its_key_never_shows() {
        let config = Config::parse(CONFIG).unwrap();
        let newer = Config::parse(&CONFIG.replace("@1.2.0", "@1.3.0")).unwrap();
        let other_key = Config::parse(&CONFIG.replace("plain-key-123", "other")).unwrap();
        let stamp = |config: &Config| mcp_stamps(config, Role::Developer)[0].fingerprint.clone();

        assert_ne!(stamp(&config), stamp(&newer));
        // The variables (where keys live) are not part of the fingerprint.
        assert_eq!(stamp(&config), stamp(&other_key));
        let agents = AgentFacts {
            config: Some(&config),
            versions: &BTreeMap::new(),
        };
        let json = serde_json::to_string(&build(Role::Developer, agents, &skills(), "")).unwrap();
        assert!(!json.contains("plain-key-123"), "{json}");
    }

    #[test]
    fn without_settings_only_the_skills_and_prompt_are_known() {
        let agents = AgentFacts {
            config: None,
            versions: &BTreeMap::new(),
        };

        let manifest = build(Role::Tester, agents, &RoleSkills::default(), "p");

        assert_eq!(manifest.agent, None);
        assert_eq!(manifest.agent_version, None);
        assert_eq!(manifest.mcp, Vec::new());
        assert_eq!(manifest.plugins, Vec::new());
    }
}
