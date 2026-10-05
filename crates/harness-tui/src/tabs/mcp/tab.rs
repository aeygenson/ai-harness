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

use harness_agents::credentials;
use harness_core::config::{McpConfig, AGENTS};
use harness_core::handoff::Role;
use harness_core::mcp::{self, SECRET_PREFIX};
use harness_core::mcp_registry::{Entry, Offer};
use harness_core::mcp_tools;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

/// What the tab asks the App to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
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
            Some(server) if !mcp::agent_runs(&settings.agent, server) => Err("mcp.agent_cannot"),
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

    pub fn on_key(&mut self, key: KeyCode, roles: &RolesTab) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::Char('n') => return self.press(ButtonId::McpNew, roles),
            KeyCode::Char('f') => return self.open_catalog(),
            KeyCode::Char('i') => return self.press(ButtonId::McpSignIn, roles),
            KeyCode::Char('e') => return self.press(ButtonId::McpEdit, roles),
            KeyCode::Delete => return self.press(ButtonId::McpRemove, roles),
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
            KeyCode::Char('/' | 's' | 'f') => return self.catalog_press(ButtonId::McpSearch),
            KeyCode::Enter | KeyCode::Char('a') => return self.catalog_press(ButtonId::McpUse),
            KeyCode::Esc | KeyCode::Backspace => self.catalog = None,
            _ => {}
        }
        Action::None
    }

    /// A button of the catalog.
    pub fn catalog_press(&mut self, id: ButtonId) -> Action {
        let Some(catalog) = &mut self.catalog else {
            return Action::None;
        };
        match id {
            ButtonId::McpSearch if !catalog.searching => Action::Search(catalog.query.clone()),
            ButtonId::McpUse => catalog.choose(),
            ButtonId::McpBack => {
                self.catalog = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// A button about the servers themselves.
    pub fn press(&self, id: ButtonId, roles: &RolesTab) -> Action {
        let current = self.current(roles);
        match (id, current) {
            (ButtonId::McpNew, _) => Action::New,
            (ButtonId::McpEdit, Some(name)) => Action::Edit(name),
            (ButtonId::McpRemove, Some(name)) if roles.servers().contains_key(&name) => {
                Action::Remove(name)
            }
            (ButtonId::McpSignIn, Some(name))
                if roles.servers().get(&name).is_some_and(is_oauth) && self.signing.is_none() =>
            {
                Action::SignIn(name)
            }
            (ButtonId::McpSecret, Some(name)) => {
                // The first secret not saved yet, else the first one.
                let wanted: Vec<String> = roles
                    .servers()
                    .get(&name)
                    .map(|s| secret_names(s).collect())
                    .unwrap_or_default();
                let offered = wanted
                    .iter()
                    .find(|n| !self.secrets.contains(n))
                    .or(wanted.first())
                    .cloned()
                    .unwrap_or(name);
                Action::Secret(offered)
            }
            _ => Action::None,
        }
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
        harness_agents::mcp_oauth::load(&home.join("credentials"), name, url.trim()).is_some()
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
            ButtonId::McpRole,
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
                (&toggle, ButtonId::McpToggle, can),
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
                (tr.t("mcp.new"), ButtonId::McpNew, true),
                (tr.t("mcp.catalog"), ButtonId::McpCatalog, true),
                (edit, ButtonId::McpEdit, current.is_some()),
                (tr.t("mcp.remove"), ButtonId::McpRemove, described.is_some()),
                (
                    tr.t("mcp.set_secret"),
                    ButtonId::McpSecret,
                    described.is_some_and(|s| secret_names(s).next().is_some()),
                ),
                (
                    tr.t("mcp.sign_in"),
                    ButtonId::McpSignIn,
                    described.is_some_and(is_oauth) && self.signing.is_none(),
                ),
                (
                    tr.t("mcp.check"),
                    ButtonId::McpCheck,
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
            let command = std::iter::once(&server.command)
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
        let agent = roles.settings(self.role()).map(|s| s.agent.clone());
        let mut spans = vec![Span::styled(format!("{} ", tr.t("mcp.agents")), bold)];
        for (index, each) in AGENTS.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            let (mark, look) = if mcp::agent_runs(each, server) {
                ("✓", green)
            } else {
                ("✗", red)
            };
            let style = if agent.as_deref() == Some(*each) {
                Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new()
            };
            spans.push(Span::styled((*each).to_string(), style));
            spans.push(Span::styled(format!(" {mark}"), look));
        }
        lines.push(Line::from(spans));
        lines.push(Line::styled(tr.t("mcp.agents_hint").to_string(), dim));

        // The tools, as the server said when it was checked last.
        lines.push(Line::default());
        let saved = self
            .home
            .as_deref()
            .and_then(|home| mcp_tools::load(home, name, server));
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
            (tr.t("mcp.search"), ButtonId::McpSearch, !catalog.searching),
            (tr.t("mcp.use"), ButtonId::McpUse, usable),
            (tr.t("mcp.back"), ButtonId::McpBack, true),
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
            join_words(std::iter::once(&offer.server.command).chain(&offer.server.args)),
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
    server.url.is_some() && server.auth.as_deref() == Some(harness_core::mcp::AUTH_OAUTH)
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

/// `npx -y "a b"` → `npx`, `-y`, `a b`: words split at spaces, a quoted
/// part stays one word.
pub fn split_words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err("a quote \" is not closed".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// The words again as one line; a word with a space is quoted.
pub fn join_words<'a>(words: impl IntoIterator<Item = &'a String>) -> String {
    words
        .into_iter()
        .map(|w| {
            if w.is_empty() || w.contains(char::is_whitespace) {
                format!("\"{w}\"")
            } else {
                w.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the server form shows for `server`: the command line (or the
/// address), the variables (or headers) and «yes» for a browser sign-in.
pub fn form_values(server: &McpConfig) -> [String; 3] {
    let pairs = |map: &std::collections::BTreeMap<String, String>| {
        let words: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
        join_words(&words)
    };
    match &server.url {
        Some(url) => [
            url.clone(),
            pairs(&server.headers),
            if server.auth.is_some() { "yes" } else { "" }.to_string(),
        ],
        // A new server has no command yet: an empty field, not `""`.
        None if server.command.is_empty() && server.args.is_empty() => {
            [String::new(), pairs(&server.env), String::new()]
        }
        None => [
            join_words(std::iter::once(&server.command).chain(&server.args)),
            pairs(&server.env),
            String::new(),
        ],
    }
}

/// A server from the form: a command line, or an address (`https://...`)
/// on the web; `NAME=value` pairs separated by spaces, which are variables
/// for a command and headers for an address; and whether to sign in through
/// the browser.
pub fn server_from(command: &str, variables: &str, sign_in: &str) -> Result<McpConfig, String> {
    let mut pairs = std::collections::BTreeMap::new();
    for word in split_words(variables)? {
        let Some((name, value)) = word.split_once('=') else {
            return Err(format!("{word:?} is not NAME=value"));
        };
        pairs.insert(name.trim().to_string(), value.trim().to_string());
    }
    let oauth = match sign_in.trim().to_lowercase().as_str() {
        "" | "no" | "нет" => false,
        "yes" | "да" | "oauth" => true,
        other => {
            return Err(format!(
                "sign-in: write yes or leave it empty, not {other:?}"
            ))
        }
    };
    let command = command.trim();
    if command.starts_with("https://") || command.starts_with("http://") {
        if command.contains(char::is_whitespace) {
            return Err("an address has no spaces".into());
        }
        return Ok(McpConfig {
            url: Some(command.to_string()),
            headers: pairs,
            auth: oauth.then(|| harness_core::mcp::AUTH_OAUTH.to_string()),
            ..McpConfig::default()
        });
    }
    if oauth {
        return Err("a sign-in through the browser is only for an address (https://...)".into());
    }
    let mut words = split_words(command)?.into_iter();
    Ok(McpConfig {
        command: words.next().unwrap_or_default(),
        args: words.collect(),
        env: pairs,
        ..McpConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_lines_become_a_server_and_back() {
        let server = server_from(
            "npx -y \"@scope/a b\"",
            "API_KEY=secret:docs  MODE=\"read only\"",
            "",
        )
        .unwrap();
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, ["-y", "@scope/a b"]);
        assert_eq!(server.env["API_KEY"], "secret:docs");
        assert_eq!(server.env["MODE"], "read only");
        assert_eq!(join_words(&server.args), "-y \"@scope/a b\"");
        assert!(server_from("npx \"open", "", "").is_err());
        assert!(server_from("npx", "NOVALUE", "").is_err());
        assert!(server_from("npx", "", "yes").is_err());
        assert!(server_from("https://a.b/mcp", "", "maybe").is_err());

        let web = server_from(
            " https://a.b/mcp ",
            "Authorization=\"Bearer secret:gh\" X-Mode=read",
            "",
        )
        .unwrap();
        assert_eq!(web.url.as_deref(), Some("https://a.b/mcp"));
        assert_eq!(web.headers["Authorization"], "Bearer secret:gh");
        assert!(web.command.is_empty() && web.auth.is_none());
        let [address, headers, sign_in] = form_values(&web);
        assert_eq!(server_from(&address, &headers, &sign_in).unwrap(), web);

        assert_eq!(form_values(&McpConfig::default()), ["", "", ""]);

        let oauth = server_from("https://a.b/mcp", "", "да").unwrap();
        assert_eq!(oauth.auth.as_deref(), Some("oauth"));
        assert_eq!(form_values(&oauth)[2], "yes");
    }
}
