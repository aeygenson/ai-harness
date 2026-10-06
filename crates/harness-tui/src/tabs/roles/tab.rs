//! The Roles tab: which agent, model and effort level each role runs on, and
//! which skills, MCP servers and plugins it gets; plus the agent of `[retro]`.
//!
//! The models and levels come from what the agents themselves said last
//! (`harness_core::models`, «Refresh models»); a model can also be typed in.
//!
//! Changes are kept here until «Save». Saving goes through
//! `harness_core::config::save::save`: the same checks as before a run, then
//! `harness.toml` is changed with `toml_edit` (comments stay) and committed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use harness_core::config;
use harness_core::config::{
    AgentKind, Config, McpConfig, PluginConfig, RetroConfig, RoleConfig, CONFIG_FILE,
};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::models::{self, ModelList};
use harness_core::skills;
use harness_core::task::handoff::Role;

/// The roles in the list, `None` is `[retro]`.
pub const WHO: [Option<Role>; 5] = [
    Some(Role::Architect),
    Some(Role::Developer),
    Some(Role::Tester),
    Some(Role::Security),
    None,
];

/// A line of the details that can be clicked or chosen with Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Agent(AgentKind),
    /// A model from the agent's list; `None` is the agent's own default.
    Model(Option<String>),
    /// A model typed in by hand.
    OtherModel,
    /// The effort level: a click moves to the next one.
    Effort,
    Skill(String),
    Mcp(String),
    Plugin(String),
}

/// What the App has to do after a row was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Ask for the model in a form; the current one is given.
    EditModel(String),
    /// Show this message.
    Say(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    List,
    Details,
}

/// A skill a role can choose: its name and the description from its header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SkillFile {
    pub(super) name: String,
    pub(super) description: Option<String>,
}

#[derive(Debug)]
pub struct RolesTab {
    root: PathBuf,
    /// `~/.harness`, where the agents' model lists are kept.
    home: Option<PathBuf>,
    /// The model list of each agent, as it answered last.
    pub(super) models: BTreeMap<AgentKind, ModelList>,
    /// `harness.toml` as it is on disk, and what it says.
    text: String,
    pub(super) saved: Config,
    /// The settings with the changes not saved yet.
    pub(super) roles: BTreeMap<Role, RoleConfig>,
    pub(super) retro: Option<RetroConfig>,
    pub(super) skills: Vec<SkillFile>,
    /// The agents installed with a saved login, from the Agents tab's check;
    /// `None` until it has answered, then all agents are offered.
    ready: Option<BTreeSet<AgentKind>>,
    pub selected: usize,
    /// The chosen row of the details.
    pub row: usize,
    pub(super) focus: Focus,
    pub problem: Option<String>,
    /// The agents are being asked for their models.
    pub refreshing: bool,
}

impl RolesTab {
    pub fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
            models: BTreeMap::new(),
            text: String::new(),
            saved: Config::parse("").unwrap_or_else(|_| unreachable!("an empty config is valid")),
            roles: BTreeMap::new(),
            retro: None,
            skills: Vec::new(),
            ready: None,
            selected: 0,
            row: 0,
            focus: Focus::List,
            problem: None,
            refreshing: false,
        };
        tab.reload();
        tab
    }

    /// Reads the settings again; changes not saved are dropped.
    pub fn reload(&mut self) {
        self.problem = None;
        let harness_dir = self.root.join(HARNESS_DIR);
        let path = harness_dir.join(CONFIG_FILE);
        match fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|text| {
                let config = Config::parse(&text).map_err(|e| e.to_string())?;
                Ok((text, config))
            }) {
            Ok((text, config)) => {
                self.roles = config.roles.clone();
                self.retro = config.retro.clone();
                self.text = text;
                self.saved = config;
            }
            Err(problem) => self.problem = Some(problem),
        }
        self.skills = read_skills(&harness_dir);
        self.reload_models();
    }

    /// What the Agents tab found: only these agents are offered for a role.
    pub fn set_ready(&mut self, ready: Option<BTreeSet<AgentKind>>) {
        self.ready = ready;
        self.row = self.row.min(self.rows().len().saturating_sub(1));
    }

    /// The agent can work: installed with a login, or not checked yet.
    pub(super) fn is_ready(&self, agent: AgentKind) -> bool {
        self.ready
            .as_ref()
            .is_none_or(|ready| ready.contains(&agent))
    }

    /// Reads the saved model lists again (after «Refresh models»).
    pub fn reload_models(&mut self) {
        self.models = match &self.home {
            Some(home) => AgentKind::ALL
                .into_iter()
                .filter_map(|agent| models::load(home, agent).map(|l| (agent, l)))
                .collect(),
            None => BTreeMap::new(),
        };
        self.row = self.row.min(self.rows().len().saturating_sub(1));
    }

    /// Are the details focused (so Esc goes back to the list)?
    pub fn in_details(&self) -> bool {
        self.focus == Focus::Details
    }

    /// Are there changes not saved yet?
    pub fn changed(&self) -> bool {
        self.roles != self.saved.roles || self.retro != self.saved.retro
    }

    pub(super) fn who(&self) -> Option<Role> {
        WHO[self.selected.min(WHO.len() - 1)]
    }

    /// The agent and model of the selected role or of `[retro]`.
    pub(super) fn agent(&self) -> (Option<AgentKind>, Option<&str>) {
        let (agent, model, _) = self.choice();
        (agent, model)
    }

    /// The agent, model and effort of the selected role or of `[retro]`.
    pub(super) fn choice(&self) -> (Option<AgentKind>, Option<&str>, Option<&str>) {
        match self.who() {
            Some(role) => self.roles.get(&role).map_or((None, None, None), |r| {
                (Some(r.agent), r.model.as_deref(), r.effort.as_deref())
            }),
            None => self.retro.as_ref().map_or((None, None, None), |r| {
                (Some(r.agent), r.model.as_deref(), r.effort.as_deref())
            }),
        }
    }

    /// `harness.toml` as it is on disk.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The MCP servers `harness.toml` describes.
    pub fn servers(&self) -> &BTreeMap<String, McpConfig> {
        &self.saved.mcp
    }

    /// A role's settings with the changes not saved yet.
    pub fn settings(&self, role: Role) -> Option<&RoleConfig> {
        self.roles.get(&role)
    }

    /// The next mark of skill `name` for `role`: not used -> read when
    /// needed -> always in the prompt -> not used; saved with the rest of
    /// the changes.
    pub fn cycle_skill(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            let on_demand = settings.skills.iter().any(|s| s == name);
            let always = settings.always_skills.iter().any(|s| s == name);
            settings.skills.retain(|s| s != name);
            settings.always_skills.retain(|s| s != name);
            match (on_demand, always) {
                (false, false) => settings.skills.push(name.to_string()),
                (true, false) => settings.always_skills.push(name.to_string()),
                _ => {}
            }
        }
    }

    /// Gives `role` the MCP server `name`, or takes it away; saved with
    /// the rest of the changes.
    pub fn toggle_mcp(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            toggle(&mut settings.mcp, name.to_string());
        }
    }

    /// The plugins `harness.toml` describes.
    pub fn plugins(&self) -> &BTreeMap<String, PluginConfig> {
        &self.saved.plugins
    }

    /// Gives `role` the plugin `name`, or takes it away; saved with the
    /// rest of the changes.
    pub fn toggle_plugin(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            toggle(&mut settings.plugins, name.to_string());
        }
    }

    pub fn undo(&mut self) {
        self.roles = self.saved.roles.clone();
        self.retro = self.saved.retro.clone();
    }

    /// Checks, writes and commits the changes. The error is shown as it is.
    pub fn save(&mut self) -> Result<(), String> {
        let mut text = self.text.clone();
        for (role, settings) in &self.roles {
            if self.saved.roles.get(role) != Some(settings) {
                text = config::edit::set_role(&text, *role, settings).map_err(|e| e.to_string())?;
            }
        }
        if self.retro != self.saved.retro {
            if let Some(retro) = &self.retro {
                text = config::edit::set_retro(&text, retro).map_err(|e| e.to_string())?;
            }
        }
        let repo = Repo::open(&self.root).map_err(|e| e.to_string())?;
        config::save::save(&repo, &text).map_err(|e| e.to_string())?;
        let (selected, row) = (self.selected, self.row);
        self.reload();
        (self.selected, self.row) = (selected, row);
        Ok(())
    }
}

pub(super) fn toggle(list: &mut Vec<String>, name: String) {
    if list.contains(&name) {
        list.retain(|n| *n != name);
    } else {
        list.push(name);
    }
}

/// The skills a role can choose: built-in optional ones and the project's
/// own, sorted by name. The base of each role is not chosen here.
fn read_skills(harness_dir: &Path) -> Vec<SkillFile> {
    skills::library(harness_dir)
        .into_iter()
        .filter(|s| !skills::is_base(&s.name))
        .map(|s| SkillFile {
            name: s.name,
            description: s.description,
        })
        .collect()
}
