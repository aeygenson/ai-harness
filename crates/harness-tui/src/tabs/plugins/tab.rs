//! The Plugins tab: which agent plugins each role gets.
//!
//! ```text
//!  Role: [ architect ] [ developer ] [ tester ] [ security ]   developer · claude
//! ┌ Plugins of the developer ────┐┌ code-review ──────────────────────────────┐
//! │> [x] code-review    claude   ││ Agent: claude                             │
//! │  [ ] rust-review    claude   ││ From: claude-plugins-official/code-review │
//! │  [ ] codex-lint     codex    ││ Inside: 3 skills, 2 commands, 1 subagent  │
//! │                              ││ Hooks: yes, not allowed                   │
//! │                              ││ Roles: developer                          │
//! └──────────────────────────────┘└───────────────────────────────────────────┘
//!  [ Give to the developer ] [ Save ] [ Undo changes ]
//!  [ Allow hooks ] [ Allow servers ] [ Remove ] [ Open in editor ]
//! ```
//!
//! Plugins differ between agents: a Claude Code plugin is only for roles on
//! Claude, a Codex plugin for roles on Codex, and
//! Antigravity has none. The role's own plugins come first; the others are
//! grey and cannot be given to it. Giving a plugin changes the same
//! `plugins = [...]` as the «Roles» tab and is kept with that tab's other
//! changes until «Save». Allowing hooks or servers and removing a plugin
//! change harness.toml at once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_core::config::{AgentKind, PluginConfig};
use harness_core::plugins::catalog::Entry;
use harness_core::plugins::{self, Details};
use harness_core::task::handoff::Role;

use crate::tabs::plugins::catalog::{CatalogView, CatalogsView, Unusable};
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;

/// A button of the Plugins tab's plugin list, clicked or chosen with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginButton {
    /// A role of the selector, an index into `ROLES`.
    Role(usize),
    /// Give the selected plugin to the role, or take it away.
    Toggle,
    /// Allow or forbid the plugin's hooks, its own servers.
    Hooks,
    Servers,
    Remove,
    /// Open the plugin's folder in the editor.
    Open,
    /// Download the plugin's newest version from its catalog.
    Update,
    /// Open «From catalog».
    OpenCatalog,
}

/// A button of «From catalog» or «Catalogs».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginCatalogButton {
    /// The agent filter, an index into `catalog::FILTERS`.
    Filter(usize),
    Search,
    Add,
    /// Add and give to the role chosen on the tab.
    AddGive,
    /// Open «Catalogs».
    OpenCatalogs,
    /// Back to the list (from «Catalogs» to «From catalog» first).
    Back,
    CatalogAdd,
    CatalogUpdate,
    CatalogRemove,
}

/// What the tab asks the App to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// Something cannot be done; the key of the message that says why.
    Refused(&'static str),
    /// Ask before removing this plugin.
    Remove(String),
    /// Set what the plugin may run by itself: its hooks, its own servers.
    /// Allowing is asked first; forbidding is not.
    Allow {
        name: String,
        hooks: bool,
        servers: bool,
    },
    /// Open the plugin's folder in the editor.
    Open(String),
    /// Open «From catalog».
    OpenCatalog,
    /// Ask what to look for in the catalogs; the last search offered.
    Search(String),
    /// Add this catalog plugin to the project, and give it to the role.
    Add {
        entry: Entry,
        give: bool,
    },
    /// The chosen catalog plugin cannot be added; why.
    Unusable(Unusable),
    /// Download the newest version of this plugin and show what changes.
    Update(String),
    /// Open «Catalogs».
    OpenCatalogs,
    /// Ask for a catalog to add; the address offered.
    AddCatalog(String),
    UpdateCatalog(String),
    /// Ask before removing this catalog.
    RemoveCatalog(String),
}

#[derive(Debug)]
pub struct PluginsTab {
    root: PathBuf,
    /// The role chosen at the top, an index into `ROLES`.
    pub(crate) role: usize,
    /// The selected plugin of the list.
    pub(crate) row: usize,
    /// What each plugin folder holds, or why it cannot be read.
    pub(super) details: BTreeMap<String, Result<Details, String>>,
    /// «From catalog», while it is open.
    pub(crate) catalog: Option<CatalogView>,
    /// «Catalogs», while it is open (over «From catalog»).
    pub(crate) catalogs: Option<CatalogsView>,
    /// What is being downloaded now, shown at the top.
    pub(crate) busy: Option<String>,
}

impl PluginsTab {
    pub fn load(root: &Path, roles: &RolesTab) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            role: 0,
            row: 0,
            details: BTreeMap::new(),
            catalog: None,
            catalogs: None,
            busy: None,
        };
        tab.reload(roles);
        tab
    }

    /// Reads the plugin folders again.
    pub fn reload(&mut self, roles: &RolesTab) {
        self.details = roles
            .plugins()
            .iter()
            .map(|(name, plugin)| {
                let path = self.folder(name, plugin);
                let details =
                    plugins::describe(&path, name, plugin.agent).map_err(|e| e.to_string());
                (name.clone(), details)
            })
            .collect();
    }

    /// The plugin's folder, absolute.
    pub fn folder(&self, name: &str, plugin: &PluginConfig) -> PathBuf {
        self.root.join(plugins::relative_path(name, plugin))
    }

    pub fn role(&self) -> Role {
        ROLES[self.role.min(ROLES.len() - 1)]
    }

    /// The agent the role runs on, if the role is set up.
    pub(super) fn agent(&self, roles: &RolesTab) -> Option<AgentKind> {
        roles.settings(self.role()).map(|settings| settings.agent)
    }

    /// The plugins in the list: those for the role's agent, those the role
    /// lists although they do not fit it, then all the others.
    pub fn names(&self, roles: &RolesTab) -> Vec<String> {
        let agent = self.agent(roles);
        let listed = roles
            .settings(self.role())
            .map_or(&[][..], |s| s.plugins.as_slice());
        let mut names: Vec<String> = roles
            .plugins()
            .iter()
            .filter(|(_, p)| Some(p.agent) == agent)
            .map(|(name, _)| name.clone())
            .collect();
        for name in listed {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        for name in roles.plugins().keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    pub(super) fn at(&self, roles: &RolesTab) -> usize {
        self.row.min(self.names(roles).len().saturating_sub(1))
    }

    /// The selected plugin.
    pub fn current(&self, roles: &RolesTab) -> Option<String> {
        self.names(roles).get(self.at(roles)).cloned()
    }

    pub fn choose_role(&mut self, index: usize) {
        if index < ROLES.len() && index != self.role {
            self.role = index;
            // The list is in another order for another agent.
            self.row = 0;
        }
    }

    /// A click on a line of the list.
    pub fn select(&mut self, index: usize, roles: &RolesTab) {
        if index < self.names(roles).len() {
            self.row = index;
        }
    }

    pub fn move_by(&mut self, delta: isize, roles: &RolesTab) {
        let len = self.names(roles).len();
        self.row = self
            .at(roles)
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    pub(super) fn has(&self, roles: &RolesTab, name: &str) -> bool {
        roles
            .settings(self.role())
            .is_some_and(|s| s.plugins.iter().any(|n| n == name))
    }

    /// «From catalog» or «Catalogs» is open instead of the list.
    pub fn in_catalog(&self) -> bool {
        self.catalog.is_some() || self.catalogs.is_some()
    }

    /// Selects the plugin `name`, if it is in the list.
    pub fn select_named(&mut self, name: &str, roles: &RolesTab) {
        if let Some(at) = self.names(roles).iter().position(|n| n == name) {
            self.row = at;
        }
    }

    /// Does the plugin have hooks, and its own servers?
    pub(super) fn brings(&self, name: &str) -> (bool, bool) {
        match self.details.get(name) {
            Some(Ok(details)) => (details.contents.hooks, details.contents.servers),
            _ => (false, false),
        }
    }

    /// Something the plugin brings that is not allowed: a role with it
    /// cannot run.
    pub(super) fn blocked(&self, name: &str, plugin: &PluginConfig) -> bool {
        let (hooks, servers) = self.brings(name);
        let apps = matches!(self.details.get(name), Some(Ok(d)) if d.contents.apps);
        (hooks && !plugin.allow_hooks) || (servers && !plugin.allow_mcp) || apps
    }
}
