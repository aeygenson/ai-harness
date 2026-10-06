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
use harness_core::config::{AgentKind, McpAuth, McpConfig};
use harness_core::mcp::registry::{Entry, Offer};
use harness_core::mcp::{self, SECRET_PREFIX};
use harness_core::task::handoff::Role;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::form::join_words;
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

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

/// The catalog of the MCP registry, while it is open.
#[derive(Debug, Default)]
pub struct Catalog {
    /// The last search.
    pub query: String,
    pub entries: Vec<Entry>,
    pub row: usize,
    /// The registry is being asked.
    pub searching: bool,
    /// Why the last search failed.
    pub error: Option<String>,
}

impl Catalog {
    fn current(&self) -> Option<&Entry> {
        self.entries.get(self.row)
    }

    /// The registry answered.
    pub fn found(&mut self, answer: Result<Vec<Entry>, String>) {
        self.searching = false;
        self.row = 0;
        match answer {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.error = Some(error);
            }
        }
    }

    fn move_by(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    fn choose(&self) -> Action {
        match self.current() {
            Some(Entry {
                offer: Some(offer), ..
            }) => Action::Use(offer.clone()),
            Some(entry) => Action::Unusable(entry.unusable.clone().unwrap_or_default()),
            None => Action::None,
        }
    }
}

#[derive(Debug)]
pub struct McpTab {
    /// `~/.harness`, where the secrets are kept.
    home: Option<PathBuf>,
    /// The role chosen at the top, an index into `ROLES`.
    pub(crate) role: usize,
    /// The selected server of the list.
    pub(crate) row: usize,
    /// The names of the saved secrets; never their values.
    secrets: Vec<String>,
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
            for name in roles.settings(role).map_or(&[][..], |s| &s.mcp[..]) {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// The selected row; the list gets shorter when a server no role
    /// lists any more and harness.toml does not describe is taken away.
    fn at(&self, roles: &RolesTab) -> usize {
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

    /// Can the role get the selected server now? `Err` says why not.
    fn can_give(&self, roles: &RolesTab, name: &str) -> Result<(), &'static str> {
        let settings = roles.settings(self.role()).ok_or("mcp.no_role")?;
        let on = settings.mcp.iter().any(|n| n == name);
        match roles.servers().get(name) {
            // A server the role has can always be taken away.
            _ if on => Ok(()),
            None => Err("mcp.not_described"),
            Some(server) if !mcp::agent_runs(settings.agent, server) => Err("mcp.agent_cannot"),
            Some(_) => Ok(()),
        }
    }

    /// «Give» or «Take away»: changes the role's list of servers. `Err` is
    /// the key of the message why it cannot.
    pub fn toggle(&self, roles: &mut RolesTab) -> Result<(), &'static str> {
        let Some(name) = self.current(roles) else {
            return Ok(());
        };
        self.can_give(roles, &name)?;
        roles.toggle_mcp(self.role(), &name);
        Ok(())
    }

    pub fn on_key(&mut self, key: KeyCode, roles: &mut RolesTab) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::Char('n') => return self.press(McpButton::New, roles),
            KeyCode::Char('f') => return self.open_catalog(),
            KeyCode::Char('i') => return self.press(McpButton::SignIn, roles),
            KeyCode::Char('e') => return self.press(McpButton::Edit, roles),
            KeyCode::Delete => return self.press(McpButton::Remove, roles),
            _ => {}
        }
        Action::None
    }

    pub fn in_catalog(&self) -> bool {
        self.catalog.is_some()
    }

    /// Opens the catalog and asks what to search.
    pub fn open_catalog(&mut self) -> Action {
        let query = self.last_query.clone();
        let catalog = self.catalog.get_or_insert_with(|| Catalog {
            query,
            ..Catalog::default()
        });
        Action::Search(catalog.query.clone())
    }

    /// A click on a line of the catalog.
    pub fn select_found(&mut self, index: usize) {
        if let Some(catalog) = &mut self.catalog {
            if index < catalog.entries.len() {
                catalog.row = index;
            }
        }
    }

    pub fn move_found(&mut self, delta: isize) {
        if let Some(catalog) = &mut self.catalog {
            catalog.move_by(delta);
        }
    }

    /// A key while the catalog is open.
    pub fn catalog_key(&mut self, key: KeyCode) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_found(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_found(1),
            KeyCode::Char('/' | 's' | 'f') => return self.catalog_press(McpCatalogButton::Search),
            KeyCode::Enter | KeyCode::Char('a') => {
                return self.catalog_press(McpCatalogButton::Use)
            }
            KeyCode::Esc | KeyCode::Backspace => self.catalog = None,
            _ => {}
        }
        Action::None
    }

    /// A button of the catalog.
    pub fn catalog_press(&mut self, id: McpCatalogButton) -> Action {
        let Some(catalog) = &mut self.catalog else {
            return Action::None;
        };
        match id {
            McpCatalogButton::Search if catalog.searching => Action::None,
            McpCatalogButton::Search => Action::Search(catalog.query.clone()),
            McpCatalogButton::Use => catalog.choose(),
            McpCatalogButton::Back => {
                self.catalog = None;
                Action::None
            }
        }
    }

    /// A button of the server list.
    pub fn press(&mut self, id: McpButton, roles: &mut RolesTab) -> Action {
        // The selected server; `None` when the list is empty.
        let current = self.current(roles);
        match id {
            McpButton::Role(index) => {
                self.choose_role(index);
                Action::None
            }
            McpButton::New => Action::New,
            McpButton::OpenCatalog => self.open_catalog(),
            McpButton::Check => Action::Check,
            McpButton::Toggle => match self.toggle(roles) {
                Ok(()) => Action::None,
                Err(key) => Action::Refused(key),
            },
            McpButton::Edit => current.map_or(Action::None, Action::Edit),
            McpButton::Remove => match current {
                Some(name) if roles.servers().contains_key(&name) => Action::Remove(name),
                _ => Action::None,
            },
            McpButton::SignIn => match current {
                Some(name)
                    if roles.servers().get(&name).is_some_and(is_oauth)
                        && self.signing.is_none() =>
                {
                    Action::SignIn(name)
                }
                _ => Action::None,
            },
            McpButton::Secret => match current {
                Some(name) => Action::Secret(self.secret_to_ask(&name, roles)),
                None => Action::None,
            },
        }
    }

    /// The secret of server `name` to ask for: the first one not saved yet,
    /// else the first one, else the server's own name.
    fn secret_to_ask(&self, name: &str, roles: &RolesTab) -> String {
        let wanted: Vec<String> = roles
            .servers()
            .get(name)
            .map(|s| secret_names(s).collect())
            .unwrap_or_default();
        wanted
            .iter()
            .find(|n| !self.secrets.contains(n))
            .or(wanted.first())
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Selects the server `name`, if it is in the list.
    pub fn select_named(&mut self, name: &str, roles: &RolesTab) {
        if let Some(at) = Self::names(roles).iter().position(|n| n == name) {
            self.row = at;
        }
    }

    fn has(&self, roles: &RolesTab, name: &str) -> bool {
        roles
            .settings(self.role())
            .is_some_and(|s| s.mcp.iter().any(|n| n == name))
    }

    /// The secrets a server needs that are not saved.
    /// Is the web server `name` signed in (for this address)?
    fn signed_in(&self, name: &str, server: &McpConfig) -> bool {
        let (Some(home), Some(url)) = (&self.home, &server.url) else {
            return false;
        };
        harness_agents::mcp::oauth::load(&home.join("credentials"), name, url.trim()).is_some()
    }

    fn missing_secrets(&self, server: &McpConfig) -> Vec<String> {
        secret_names(server)
            .filter(|n| !self.secrets.contains(n))
            .collect()
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        if let Some(catalog) = &self.catalog {
            draw_catalog(catalog, frame, area, hits, tr, roles);
            return;
        }
        let [top, main, bottom, servers_row] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let role = self.role();
        let names: Vec<&str> = ROLES.iter().map(|r| r.as_str()).collect();
        selector(
            frame,
            top,
            hits,
            tr.t("skills.role"),
            &names,
            self.role,
            |index| ButtonId::Mcp(McpButton::Role(index)),
        );
        // The role's agent, on the right of the selector.
        if let Some(settings) = roles.settings(role) {
            let text = match &settings.model {
                Some(model) => format!("{} · {} ({model}) ", role.as_str(), settings.agent),
                None => format!("{} · {} ", role.as_str(), settings.agent),
            };
            let width = u16::try_from(text.chars().count()).unwrap_or(0);
            let x = top.right().saturating_sub(width);
            if x > top.x + 60 {
                frame.render_widget(
                    Span::styled(text, theme::dim()),
                    Rect::new(x, top.y, width, 1),
                );
            }
        }

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(main);
        let dim = theme::dim();
        let red = theme::bad();
        let servers = Self::names(roles);
        let items: Vec<ListItem> = servers
            .iter()
            .map(|name| {
                let mark = if self.has(roles, name) { "[x]" } else { "[ ]" };
                let grey = self.can_give(roles, name).is_err();
                let note = match roles.servers().get(name) {
                    None => Span::styled(tr.t("mcp.not_described_short").to_string(), red),
                    Some(server) if !self.missing_secrets(server).is_empty() => {
                        Span::styled(tr.t("mcp.no_secret_short").to_string(), red)
                    }
                    Some(server) if is_oauth(server) && !self.signed_in(name, server) => {
                        Span::styled(tr.t("mcp.no_sign_in_short").to_string(), red)
                    }
                    Some(_) => Span::raw(""),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{mark} {name:<16} "),
                        if grey { dim } else { Style::new() },
                    ),
                    note,
                ]))
            })
            .collect();
        let title = tr.f("mcp.title", &[("role", &role)]);
        draw_list(
            frame,
            hits,
            left,
            ListId::Mcp,
            &title,
            items,
            self.at(roles),
            true,
        );

        let current = self.current(roles);
        let (title, lines) = match &current {
            Some(name) => (format!(" {name} "), self.details(roles, name, tr)),
            None => (
                String::new(),
                tr.t("mcp.empty")
                    .lines()
                    .map(|l| Line::from(l.to_string()))
                    .collect(),
            ),
        };
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(&title, false))
                .wrap(Wrap { trim: false }),
            right,
        );

        let toggle = match &current {
            Some(name) if self.has(roles, name) => tr.f("mcp.take", &[("role", &role)]),
            _ => tr.f("mcp.give", &[("role", &role)]),
        };
        let can = current
            .as_deref()
            .is_some_and(|name| self.can_give(roles, name).is_ok());
        let changed = roles.changed();
        buttons(
            frame,
            bottom,
            hits,
            &[
                (&toggle, ButtonId::Mcp(McpButton::Toggle), can),
                (tr.t("roles.save"), ButtonId::Save, changed),
                (tr.t("roles.undo"), ButtonId::Undo, changed),
            ],
        );
        if changed {
            let note = tr.t("roles.unsaved");
            let width = u16::try_from(note.chars().count()).unwrap_or(0);
            let x = bottom.right().saturating_sub(width);
            frame.render_widget(
                Span::styled(note.to_string(), theme::warn()),
                Rect::new(x.max(bottom.x), bottom.y, width.min(bottom.width), 1),
            );
        }
        // The servers themselves: they change harness.toml at once.
        let described = current
            .as_deref()
            .and_then(|name| roles.servers().get(name));
        let edit = if current.is_some() && described.is_none() {
            tr.t("mcp.describe")
        } else {
            tr.t("mcp.edit")
        };
        buttons(
            frame,
            servers_row,
            hits,
            &[
                (tr.t("mcp.new"), ButtonId::Mcp(McpButton::New), true),
                (
                    tr.t("mcp.catalog"),
                    ButtonId::Mcp(McpButton::OpenCatalog),
                    true,
                ),
                (edit, ButtonId::Mcp(McpButton::Edit), current.is_some()),
                (
                    tr.t("mcp.remove"),
                    ButtonId::Mcp(McpButton::Remove),
                    described.is_some(),
                ),
                (
                    tr.t("mcp.set_secret"),
                    ButtonId::Mcp(McpButton::Secret),
                    described.is_some_and(|s| secret_names(s).next().is_some()),
                ),
                (
                    tr.t("mcp.sign_in"),
                    ButtonId::Mcp(McpButton::SignIn),
                    described.is_some_and(is_oauth) && self.signing.is_none(),
                ),
                (
                    tr.t("mcp.check"),
                    ButtonId::Mcp(McpButton::Check),
                    described.is_some() && self.checking.is_none(),
                ),
            ],
        );
    }

    /// The address and headers of a web server.
    fn web_details(
        &self,
        name: &str,
        server: &McpConfig,
        url: &str,
        tr: &I18n,
        lines: &mut Vec<Line<'static>>,
    ) {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.address")), bold),
            Span::raw(url.to_string()),
        ]));
        lines.push(Line::styled(tr.t("mcp.address_hint").to_string(), dim));
        lines.push(Line::default());
        if is_oauth(server) {
            let state = if self.signing.as_deref() == Some(name) {
                Span::styled(tr.t("mcp.signing_in").to_string(), dim)
            } else if self.signed_in(name, server) {
                Span::styled(tr.t("mcp.signed_in").to_string(), green)
            } else {
                Span::styled(tr.t("mcp.not_signed_in").to_string(), red)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", tr.t("mcp.sign_in_label")), bold),
                state,
            ]));
            lines.push(Line::styled(tr.t("mcp.sign_in_hint").to_string(), dim));
            lines.push(Line::default());
        }
        if server.headers.is_empty() {
            lines.push(Line::styled(tr.t("mcp.no_headers").to_string(), dim));
        } else {
            lines.push(Line::styled(tr.t("mcp.headers").to_string(), bold));
            for (header, value) in &server.headers {
                let mut spans = vec![Span::raw(format!("  {header}: {value}"))];
                if let Some((_, secret)) = value.split_once(SECRET_PREFIX) {
                    let secret = secret.trim();
                    spans.push(if self.secrets.iter().any(|s| s == secret) {
                        Span::styled(format!("  {}", tr.t("mcp.secret_saved")), green)
                    } else {
                        Span::styled(
                            format!("  {}", tr.f("mcp.secret_missing", &[("name", &secret)])),
                            red,
                        )
                    });
                }
                lines.push(Line::from(spans));
            }
        }
    }

    /// What the details show about the server `name`.
    fn details(&self, roles: &RolesTab, name: &str, tr: &I18n) -> Vec<Line<'static>> {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        let Some(server) = roles.servers().get(name) else {
            lines.push(Line::styled(
                tr.f("mcp.not_described", &[("name", &name)]),
                red,
            ));
            return lines;
        };
        if let Some(url) = &server.url {
            self.web_details(name, server, url, tr, &mut lines);
        } else {
            let command = server
                .command
                .iter()
                .chain(&server.args)
                .map(|part| {
                    if part.contains(char::is_whitespace) || part.is_empty() {
                        format!("{part:?}")
                    } else {
                        part.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", tr.t("mcp.command")), bold),
                Span::raw(command),
            ]));
            lines.push(Line::default());
            if server.env.is_empty() {
                lines.push(Line::styled(tr.t("mcp.no_variables").to_string(), dim));
            } else {
                lines.push(Line::styled(tr.t("mcp.variables").to_string(), bold));
                for (variable, value) in &server.env {
                    let mut spans = vec![Span::raw(format!("  {variable} = {value}"))];
                    if let Some(secret) = value.strip_prefix(SECRET_PREFIX) {
                        let secret = secret.trim();
                        spans.push(if self.secrets.iter().any(|s| s == secret) {
                            Span::styled(format!("  {}", tr.t("mcp.secret_saved")), green)
                        } else {
                            Span::styled(
                                format!("  {}", tr.f("mcp.secret_missing", &[("name", &secret)])),
                                red,
                            )
                        });
                    }
                    lines.push(Line::from(spans));
                }
            }
        }

        // Which agents can start it; the role's own agent in bold.
        lines.push(Line::default());
        let agent = roles.settings(self.role()).map(|s| s.agent);
        let mut spans = vec![Span::styled(format!("{} ", tr.t("mcp.agents")), bold)];
        for (index, each) in AgentKind::ALL.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            let (mark, look) = if mcp::agent_runs(each, server) {
                ("✓", green)
            } else {
                ("✗", red)
            };
            let style = if agent == Some(each) {
                Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new()
            };
            spans.push(Span::styled(each.to_string(), style));
            spans.push(Span::styled(format!(" {mark}"), look));
        }
        lines.push(Line::from(spans));
        lines.push(Line::styled(tr.t("mcp.agents_hint").to_string(), dim));

        // The tools, as the server said when it was checked last.
        lines.push(Line::default());
        let saved = self
            .home
            .as_deref()
            .and_then(|home| mcp::tools::load(home, name, server));
        match (&self.checking, saved) {
            (Some(checking), _) if checking == name => {
                lines.push(Line::styled(tr.t("mcp.tools_checking").to_string(), dim));
            }
            (_, Some(list)) => {
                lines.push(Line::styled(
                    tr.f("mcp.tools", &[("count", &list.tools.len())]),
                    bold,
                ));
                for tool in list.tools {
                    let mut spans = vec![Span::raw(format!("  {}", tool.name))];
                    if let Some(description) = tool.description {
                        spans.push(Span::styled(format!("  {description}"), dim));
                    }
                    lines.push(Line::from(spans));
                }
            }
            (_, None) => {
                lines.push(Line::styled(tr.t("mcp.tools_unknown").to_string(), dim));
            }
        }

        lines.push(Line::default());
        let users: Vec<&str> = ROLES
            .iter()
            .filter(|r| {
                roles
                    .settings(**r)
                    .is_some_and(|s| s.mcp.iter().any(|n| n == name))
            })
            .map(|r| r.as_str())
            .collect();
        let users = if users.is_empty() {
            tr.t("mcp.no_roles").to_string()
        } else {
            users.join(", ")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.roles")), bold),
            Span::raw(users),
        ]));
        lines
    }
}

/// The catalog: the servers found on the left, the chosen one on the right.
fn draw_catalog(
    catalog: &Catalog,
    frame: &mut Frame,
    area: Rect,
    hits: &mut Hits,
    tr: &I18n,
    roles: &RolesTab,
) {
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let status = if catalog.searching {
        Span::styled(tr.f("mcp.searching", &[("query", &catalog.query)]), dim)
    } else if let Some(error) = &catalog.error {
        Span::styled(tr.f("mcp.search_failed", &[("error", &error)]), red)
    } else if catalog.query.is_empty() {
        Span::styled(
            tr.f("mcp.found_all", &[("count", &catalog.entries.len())]),
            dim,
        )
    } else {
        Span::styled(
            tr.f(
                "mcp.found",
                &[("count", &catalog.entries.len()), ("query", &catalog.query)],
            ),
            dim,
        )
    };
    frame.render_widget(
        Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.catalog_title")), bold),
            status,
        ]),
        top,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(main);
    let items: Vec<ListItem> = catalog
        .entries
        .iter()
        .map(|entry| {
            let shown = entry
                .offer
                .as_ref()
                .map_or_else(|| short_name(&entry.name), |o| o.name.clone());
            let added = entry
                .offer
                .as_ref()
                .is_some_and(|o| roles.servers().contains_key(&o.name));
            let style = if entry.offer.is_some() {
                Style::new()
            } else {
                dim
            };
            let mut spans = vec![Span::styled(format!("{shown:<20} "), style)];
            if added {
                spans.push(Span::styled(tr.t("mcp.already_added").to_string(), dim));
            } else if let Some(offer) = &entry.offer {
                spans.push(Span::styled(offer.kind.clone(), dim));
            } else {
                spans.push(Span::styled(tr.t("mcp.web_only").to_string(), dim));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    draw_list(
        frame,
        hits,
        left,
        ListId::McpCatalog,
        &format!(" {} ", tr.t("mcp.catalog_list")),
        items,
        catalog.row,
        true,
    );

    let (title, lines) = match catalog.current() {
        Some(entry) => (format!(" {} ", entry.name), entry_details(entry, tr)),
        None => (
            String::new(),
            tr.t("mcp.catalog_empty")
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect(),
        ),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&title, false))
            .wrap(Wrap { trim: false }),
        right,
    );

    let usable = catalog.current().is_some_and(|e| e.offer.is_some());
    buttons(
        frame,
        bottom,
        hits,
        &[
            (
                tr.t("mcp.search"),
                ButtonId::McpCatalog(McpCatalogButton::Search),
                !catalog.searching,
            ),
            (
                tr.t("mcp.use"),
                ButtonId::McpCatalog(McpCatalogButton::Use),
                usable,
            ),
            (
                tr.t("mcp.back"),
                ButtonId::McpCatalog(McpCatalogButton::Back),
                true,
            ),
        ],
    );
}

/// What the catalog shows about one registry server.
fn entry_details(entry: &Entry, tr: &I18n) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    if let Some(title) = &entry.title {
        lines.push(Line::styled(title.clone(), bold));
    }
    if let Some(description) = &entry.description {
        lines.push(Line::from(description.clone()));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t("mcp.version")), bold),
        Span::raw(entry.version.clone()),
    ]));
    if let Some(repository) = &entry.repository {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.repository")), bold),
            Span::raw(repository.clone()),
        ]));
    }
    lines.push(Line::default());
    let Some(offer) = &entry.offer else {
        let why = entry.unusable.clone().unwrap_or_default();
        lines.push(Line::styled(tr.f("mcp.cannot_use", &[("why", &why)]), red));
        return lines;
    };
    let (label, shown, values, none, title) = match &offer.server.url {
        Some(url) => (
            "mcp.address",
            url.clone(),
            &offer.server.headers,
            "mcp.no_headers",
            "mcp.headers",
        ),
        None => (
            "mcp.command",
            join_words(offer.server.command.iter().chain(&offer.server.args)),
            &offer.server.env,
            "mcp.no_variables",
            "mcp.variables",
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t(label)), bold),
        Span::raw(shown),
    ]));
    lines.push(Line::default());
    if offer.variables.is_empty() {
        lines.push(Line::styled(tr.t(none).to_string(), dim));
    } else {
        lines.push(Line::styled(tr.t(title).to_string(), bold));
        for (variable, description) in &offer.variables {
            let value = match values.get(variable) {
                Some(value) if value.is_empty() => tr.t("mcp.fill_in").to_string(),
                Some(value) => value.clone(),
                None => tr.t("mcp.optional").to_string(),
            };
            lines.push(Line::from(format!("  {variable} = {value}")));
            if !description.is_empty() {
                lines.push(Line::styled(format!("    {description}"), dim));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::styled(tr.t("mcp.use_hint").to_string(), dim));
    lines
}

/// `io.github.x/some-server` → `some-server`, for a server that gets no
/// name of its own.
fn short_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// A web server Lisa signs in to in the browser.
fn is_oauth(server: &McpConfig) -> bool {
    server.url.is_some() && server.auth == Some(McpAuth::OAuth)
}

/// The names of the secrets a server's variables (or a web server's
/// headers, as in `Bearer secret:<name>`) use.
fn secret_names(server: &McpConfig) -> impl Iterator<Item = String> + '_ {
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
