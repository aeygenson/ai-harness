//! The MCP tab: which MCP servers each role gets.
//!
//! ```text
//!  Role: [ architect ] [ developer ] [ tester ] [ security ]
//! ┌ MCP servers of the developer ┐┌ context7 ─────────────────────────────────┐
//! │> [x] context7                ││ Command: npx -y @upstash/context7-mcp     │
//! │  [ ] github     ✗ secret     ││ Variables:                                │
//! │                              ││   CONTEXT7_API_KEY = secret:context7  ✓   │
//! │                              ││ Agents: claude ✓ · codex ✓ · …            │
//! │                              ││ Roles: developer, tester                  │
//! └──────────────────────────────┘└───────────────────────────────────────────┘
//!  [ Give to the developer ] [ Save ] [ Undo changes ]
//! ```
//!
//! The servers are described once in `harness.toml` (`[mcp.<name>]`) and are
//! the same for every agent: each of them starts a program server the same
//! way. A server the role's agent cannot start is grey. Giving a server to a
//! role changes the same `mcp = [...]` as the «Roles» tab, and is kept with
//! that tab's other changes until «Save».
//!
//! «From catalog» searches the official MCP registry. The chosen server
//! opens in the «New server» form with its command, pinned version and
//! variables filled in; nothing is written before that form is saved.

use std::path::{Path, PathBuf};

use harness_agents::install::credentials;
use harness_core::config::{McpAuth, McpConfig};
use harness_core::mcp::registry::Offer;
use harness_core::mcp::SECRET_PREFIX;
use harness_core::task::handoff::Role;

use super::catalog::Catalog;
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;

/// A button of the MCP tab's server list, clicked or chosen with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpButton {
    /// A role of the selector, an index into `tabs::skills::ROLES`.
    Role(usize),
    /// Gives the selected server to the role, or takes it away.
    Toggle,
    New,
    Edit,
    Remove,
    /// Saves a secret the selected server needs.
    Secret,
    /// Starts the selected server and asks it for its tools.
    Check,
    /// Signs in to the selected web server in the browser.
    SignIn,
    /// Opens the catalog of the MCP registry.
    OpenCatalog,
}

/// A button of the MCP registry's catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCatalogButton {
    /// A new search.
    Search,
    /// Add the chosen server.
    Use,
    /// Back to the list.
    Back,
}

/// What the tab asks the App to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Something cannot be done; the key of the message that says why.
    Refused(&'static str),
    /// Start the selected server and ask it for its tools.
    Check,
    /// Ask for a new server.
    New,
    /// Change this server; one harness.toml does not describe yet gets
    /// described.
    Edit(String),
    /// Ask before removing this server.
    Remove(String),
    /// Ask for a secret; the name offered first.
    Secret(String),
    /// Sign in to this web server in the browser.
    SignIn(String),
    /// Ask what to search in the registry; the last search offered.
    Search(String),
    /// Open the «New server» form with this server from the registry.
    Use(Offer),
    /// The chosen registry server cannot be used; why.
    Unusable(String),
}

#[derive(Debug)]
pub struct McpTab {
    /// `~/.harness`, where the secrets are kept.
    pub(super) home: Option<PathBuf>,
    /// The role chosen at the top, an index into `ROLES`.
    pub(crate) role: usize,
    /// The selected server of the list.
    pub(crate) row: usize,
    /// The names of the saved secrets; never their values.
    pub(super) secrets: Vec<String>,
    /// The server «Check» is asking now.
    pub(crate) checking: Option<String>,
    /// «From catalog», while it is open.
    pub(crate) catalog: Option<Catalog>,
    /// The last search, offered again when the catalog opens.
    pub(crate) last_query: String,
    /// The server whose sign-in in the browser is going on.
    pub(crate) signing: Option<String>,
}

impl McpTab {
    pub fn load(home: Option<&Path>) -> Self {
        let mut tab = Self {
            home: home.map(Path::to_path_buf),
            role: 0,
            row: 0,
            secrets: Vec::new(),
            checking: None,
            catalog: None,
            last_query: String::new(),
            signing: None,
        };
        tab.reload();
        tab
    }

    /// Reads which secrets are saved again.
    pub fn reload(&mut self) {
        self.secrets = self
            .home
            .as_ref()
            .and_then(|home| credentials::secret_names(&home.join("credentials")).ok())
            .unwrap_or_default();
    }

    pub fn role(&self) -> Role {
        ROLES[self.role.min(ROLES.len() - 1)]
    }

    /// The servers in the list: those `harness.toml` describes, then those a
    /// role lists without a description.
    pub fn names(roles: &RolesTab) -> Vec<String> {
        let mut names: Vec<String> = roles.servers().keys().cloned().collect();
        for role in ROLES {
            for name in roles.settings(role).map_or(&[][..], |s| s.mcp.as_slice()) {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// The selected row; the list gets shorter when a server no role
    /// lists any more and harness.toml does not describe is taken away.
    pub(super) fn at(&self, roles: &RolesTab) -> usize {
        self.row.min(Self::names(roles).len().saturating_sub(1))
    }

    /// The selected server.
    pub fn current(&self, roles: &RolesTab) -> Option<String> {
        Self::names(roles).get(self.at(roles)).cloned()
    }

    pub fn choose_role(&mut self, index: usize) {
        if index < ROLES.len() {
            self.role = index;
        }
    }

    /// A click on a line of the list.
    pub fn select(&mut self, index: usize, roles: &RolesTab) {
        if index < Self::names(roles).len() {
            self.row = index;
        }
    }

    pub fn move_by(&mut self, delta: isize, roles: &RolesTab) {
        let len = Self::names(roles).len();
        self.row = self
            .at(roles)
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    /// Selects the server `name`, if it is in the list.
    pub fn select_named(&mut self, name: &str, roles: &RolesTab) {
        if let Some(at) = Self::names(roles).iter().position(|n| n == name) {
            self.row = at;
        }
    }

    pub(super) fn has(&self, roles: &RolesTab, name: &str) -> bool {
        roles
            .settings(self.role())
            .is_some_and(|s| s.mcp.iter().any(|n| n == name))
    }

    /// The secrets a server needs that are not saved.
    /// Is the web server `name` signed in (for this address)?
    pub(super) fn signed_in(&self, name: &str, server: &McpConfig) -> bool {
        let (Some(home), Some(url)) = (&self.home, &server.url) else {
            return false;
        };
        harness_agents::mcp::oauth::load(&home.join("credentials"), name, url.trim()).is_some()
    }

    pub(super) fn missing_secrets(&self, server: &McpConfig) -> Vec<String> {
        secret_names(server)
            .filter(|n| !self.secrets.contains(n))
            .collect()
    }
}

/// `io.github.x/some-server` → `some-server`, for a server that gets no
/// name of its own.
pub(super) fn short_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// A web server Lisa signs in to in the browser.
pub(super) fn is_oauth(server: &McpConfig) -> bool {
    server.url.is_some() && server.auth == Some(McpAuth::OAuth)
}

/// The names of the secrets a server's variables (or a web server's
/// headers, as in `Bearer secret:<name>`) use.
pub(super) fn secret_names(server: &McpConfig) -> impl Iterator<Item = String> + '_ {
    let variables = server
        .env
        .values()
        .filter_map(|v| v.strip_prefix(SECRET_PREFIX));
    let headers = server
        .headers
        .values()
        .filter_map(|v| v.split_once(SECRET_PREFIX).map(|(_, name)| name));
    variables.chain(headers).map(|n| n.trim().to_string())
}
