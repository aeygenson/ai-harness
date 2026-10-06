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
//!  [ Allow hooks ] [ Allow servers ] [ Remove ] [ Open in Zed ]
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
use harness_core::plugins::{self, Details};
use harness_core::task::handoff::Role;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::plugins::catalog::Entry;

use crate::tabs::plugins::catalog::{
    draw_catalog, draw_catalogs, unusable, CatalogView, CatalogsView, Unusable, OFFICIAL,
};
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

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
    details: BTreeMap<String, Result<Details, String>>,
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
    fn agent(&self, roles: &RolesTab) -> Option<AgentKind> {
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

    fn at(&self, roles: &RolesTab) -> usize {
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

    fn has(&self, roles: &RolesTab, name: &str) -> bool {
        roles
            .settings(self.role())
            .is_some_and(|s| s.plugins.iter().any(|n| n == name))
    }

    /// Can the role get the plugin now? `Err` is the key of the message why not.
    fn can_give(&self, roles: &RolesTab, name: &str) -> Result<(), &'static str> {
        if roles.settings(self.role()).is_none() {
            return Err("plugins.no_role");
        }
        // A plugin the role has can always be taken away.
        if self.has(roles, name) {
            return Ok(());
        }
        let agent = self.agent(roles);
        match roles.plugins().get(name) {
            None => Err("plugins.not_described_short"),
            Some(_) if !agent.is_some_and(AgentKind::has_plugins) => Err("plugins.agent_has_none"),
            Some(plugin) if Some(plugin.agent) != agent => Err("plugins.other_agent"),
            Some(_) => Ok(()),
        }
    }

    /// «Give» or «Take away». `Err` is the key of the message why it cannot.
    pub fn toggle(&self, roles: &mut RolesTab) -> Result<(), &'static str> {
        let Some(name) = self.current(roles) else {
            return Ok(());
        };
        self.can_give(roles, &name)?;
        roles.toggle_plugin(self.role(), &name);
        Ok(())
    }

    pub fn on_key(&mut self, key: KeyCode, roles: &mut RolesTab) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::Char('e') => return self.press(PluginButton::Open, roles),
            KeyCode::Char('f') => return Action::OpenCatalog,
            KeyCode::Char('U') => return self.press(PluginButton::Update, roles),
            KeyCode::Delete => return self.press(PluginButton::Remove, roles),
            _ => {}
        }
        Action::None
    }

    /// «From catalog» or «Catalogs» is open instead of the list.
    pub fn in_catalog(&self) -> bool {
        self.catalog.is_some() || self.catalogs.is_some()
    }

    /// A key while «From catalog» or «Catalogs» is open.
    pub fn catalog_key(&mut self, key: KeyCode) -> Action {
        if let Some(view) = &mut self.catalogs {
            match key {
                KeyCode::Up | KeyCode::Char('k') => view.move_by(-1),
                KeyCode::Down | KeyCode::Char('j') => view.move_by(1),
                KeyCode::Char('n') => return self.catalog_press(PluginCatalogButton::CatalogAdd),
                KeyCode::Char('U') => {
                    return self.catalog_press(PluginCatalogButton::CatalogUpdate)
                }
                KeyCode::Delete => return self.catalog_press(PluginCatalogButton::CatalogRemove),
                KeyCode::Esc | KeyCode::Backspace => self.catalogs = None,
                _ => {}
            }
            return Action::None;
        }
        let Some(view) = &mut self.catalog else {
            return Action::None;
        };
        match key {
            KeyCode::Up | KeyCode::Char('k') => view.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => view.move_by(1),
            KeyCode::Left | KeyCode::Char('h') => {
                view.choose_filter(view.filter.saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char('l') => view.choose_filter(view.filter + 1),
            KeyCode::Char('/' | 's' | 'f') => {
                return self.catalog_press(PluginCatalogButton::Search)
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                return self.catalog_press(PluginCatalogButton::Add)
            }
            KeyCode::Char('g') => return self.catalog_press(PluginCatalogButton::AddGive),
            KeyCode::Char('c') => return self.catalog_press(PluginCatalogButton::OpenCatalogs),
            KeyCode::Esc | KeyCode::Backspace => self.catalog = None,
            _ => {}
        }
        Action::None
    }

    /// A button of «From catalog» or «Catalogs».
    pub fn catalog_press(&mut self, id: PluginCatalogButton) -> Action {
        // A download is running: nothing that starts another one.
        let busy = self.busy.is_some();
        match id {
            PluginCatalogButton::Back if self.catalogs.is_some() => self.catalogs = None,
            PluginCatalogButton::Back => self.catalog = None,
            PluginCatalogButton::OpenCatalogs => return Action::OpenCatalogs,
            PluginCatalogButton::Filter(index) => {
                if let Some(view) = &mut self.catalog {
                    view.choose_filter(index);
                }
            }
            PluginCatalogButton::Search => {
                if let Some(view) = &self.catalog {
                    return Action::Search(view.query.clone());
                }
            }
            PluginCatalogButton::Add | PluginCatalogButton::AddGive if !busy => {
                if let Some(entry) = self.catalog.as_ref().and_then(CatalogView::current) {
                    if let Some(why) = unusable(entry) {
                        return Action::Unusable(why);
                    }
                    return Action::Add {
                        entry: entry.clone(),
                        give: id == PluginCatalogButton::AddGive,
                    };
                }
            }
            PluginCatalogButton::CatalogAdd if !busy => {
                let empty = self.catalogs.as_ref().is_none_or(|v| v.list.is_empty());
                return Action::AddCatalog(if empty { OFFICIAL } else { "" }.to_string());
            }
            PluginCatalogButton::CatalogUpdate if !busy => {
                if let Some(c) = self.catalogs.as_ref().and_then(CatalogsView::current) {
                    return Action::UpdateCatalog(c.name.clone());
                }
            }
            PluginCatalogButton::CatalogRemove if !busy => {
                if let Some(c) = self.catalogs.as_ref().and_then(CatalogsView::current) {
                    return Action::RemoveCatalog(c.name.clone());
                }
            }
            PluginCatalogButton::Add
            | PluginCatalogButton::AddGive
            | PluginCatalogButton::CatalogAdd
            | PluginCatalogButton::CatalogUpdate
            | PluginCatalogButton::CatalogRemove => {}
        }
        Action::None
    }

    /// A button of the plugin list.
    pub fn press(&mut self, id: PluginButton, roles: &mut RolesTab) -> Action {
        match id {
            PluginButton::Role(index) => {
                self.choose_role(index);
                return Action::None;
            }
            PluginButton::OpenCatalog => return Action::OpenCatalog,
            PluginButton::Toggle => {
                return match self.toggle(roles) {
                    Ok(()) => Action::None,
                    Err(key) => Action::Refused(key),
                }
            }
            PluginButton::Hooks
            | PluginButton::Servers
            | PluginButton::Remove
            | PluginButton::Open
            | PluginButton::Update => {}
        }
        // The rest is about the selected plugin.
        let Some(name) = self.current(roles) else {
            return Action::None;
        };
        let Some(plugin) = roles.plugins().get(&name) else {
            return Action::None;
        };
        let (has_hooks, has_servers) = self.brings(&name);
        match id {
            PluginButton::Remove => Action::Remove(name),
            PluginButton::Open => Action::Open(name),
            PluginButton::Update if plugin.source.is_some() && self.busy.is_none() => {
                Action::Update(name)
            }
            PluginButton::Hooks if has_hooks || plugin.allow_hooks => Action::Allow {
                name,
                hooks: !plugin.allow_hooks,
                servers: plugin.allow_mcp,
            },
            PluginButton::Servers if has_servers || plugin.allow_mcp => Action::Allow {
                name,
                hooks: plugin.allow_hooks,
                servers: !plugin.allow_mcp,
            },
            // Not possible now, or handled above.
            PluginButton::Update
            | PluginButton::Hooks
            | PluginButton::Servers
            | PluginButton::Role(_)
            | PluginButton::Toggle
            | PluginButton::OpenCatalog => Action::None,
        }
    }

    /// Selects the plugin `name`, if it is in the list.
    pub fn select_named(&mut self, name: &str, roles: &RolesTab) {
        if let Some(at) = self.names(roles).iter().position(|n| n == name) {
            self.row = at;
        }
    }

    /// Does the plugin have hooks, and its own servers?
    fn brings(&self, name: &str) -> (bool, bool) {
        match self.details.get(name) {
            Some(Ok(details)) => (details.contents.hooks, details.contents.servers),
            _ => (false, false),
        }
    }

    /// Something the plugin brings that is not allowed: a role with it
    /// cannot run.
    fn blocked(&self, name: &str, plugin: &PluginConfig) -> bool {
        let (hooks, servers) = self.brings(name);
        let apps = matches!(self.details.get(name), Some(Ok(d)) if d.contents.apps);
        (hooks && !plugin.allow_hooks) || (servers && !plugin.allow_mcp) || apps
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let busy = self.busy.as_deref();
        let role_label = self.role().as_str();
        if let Some(view) = &self.catalogs {
            draw_catalogs(view, frame, area, hits, tr, busy);
            return;
        }
        if let Some(view) = &self.catalog {
            draw_catalog(view, frame, area, hits, tr, roles, role_label, busy);
            return;
        }
        let [top, main, bottom, plugins_row] = Layout::vertical([
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
            |index| ButtonId::Plugin(PluginButton::Role(index)),
        );
        if let Some(settings) = roles.settings(role) {
            let text = format!("{} · {} ", role.as_str(), settings.agent);
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
        let list = self.names(roles);
        let items: Vec<ListItem> = list
            .iter()
            .map(|name| {
                let on = self.has(roles, name);
                let mark = if on { "[x]" } else { "[ ]" };
                let grey = self.can_give(roles, name).is_err();
                let plugin = roles.plugins().get(name);
                // Kept by the role although it does not fit: a run stops.
                let wrong = on && plugin.is_none_or(|p| Some(p.agent) != self.agent(roles));
                let note = match plugin {
                    None => Span::styled(tr.t("plugins.not_described_short").to_string(), red),
                    Some(p) if wrong => Span::styled(p.agent.to_string(), red),
                    Some(p) if self.blocked(name, p) => {
                        Span::styled(tr.t("plugins.blocked_short").to_string(), red)
                    }
                    Some(p) => Span::styled(p.agent.to_string(), dim),
                };
                let style = match () {
                    () if wrong => red,
                    () if grey => dim,
                    () => Style::new(),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{mark} {name:<16} "), style),
                    note,
                ]))
            })
            .collect();
        let title = tr.f("plugins.title", &[("role", &role)]);
        draw_list(
            frame,
            hits,
            left,
            ListId::Plugins,
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
                tr.t("plugins.empty")
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
            Some(name) if self.has(roles, name) => tr.f("plugins.take", &[("role", &role)]),
            _ => tr.f("plugins.give", &[("role", &role)]),
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
                (&toggle, ButtonId::Plugin(PluginButton::Toggle), can),
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
        // The plugins themselves: they change harness.toml at once.
        let described = current
            .as_deref()
            .and_then(|name| roles.plugins().get(name).map(|p| (name, p)));
        let (hooks, servers) = described.map_or((false, false), |(name, _)| self.brings(name));
        let hooks_label = match described {
            Some((_, p)) if p.allow_hooks => tr.t("plugins.forbid_hooks"),
            _ => tr.t("plugins.allow_hooks"),
        };
        let servers_label = match described {
            Some((_, p)) if p.allow_mcp => tr.t("plugins.forbid_servers"),
            _ => tr.t("plugins.allow_servers"),
        };
        buttons(
            frame,
            plugins_row,
            hits,
            &[
                (
                    tr.t("plugins.from_catalog"),
                    ButtonId::Plugin(PluginButton::OpenCatalog),
                    true,
                ),
                (
                    tr.t("plugins.update"),
                    ButtonId::Plugin(PluginButton::Update),
                    busy.is_none() && described.is_some_and(|(_, p)| p.source.is_some()),
                ),
                (
                    hooks_label,
                    ButtonId::Plugin(PluginButton::Hooks),
                    described.is_some_and(|(_, p)| hooks || p.allow_hooks),
                ),
                (
                    servers_label,
                    ButtonId::Plugin(PluginButton::Servers),
                    described.is_some_and(|(_, p)| servers || p.allow_mcp),
                ),
                (
                    tr.t("plugins.remove"),
                    ButtonId::Plugin(PluginButton::Remove),
                    described.is_some(),
                ),
                (
                    tr.t("plugins.open"),
                    ButtonId::Plugin(PluginButton::Open),
                    described.is_some(),
                ),
            ],
        );
    }

    /// What the details show about the plugin `name`.
    fn details(&self, roles: &RolesTab, name: &str, tr: &I18n) -> Vec<Line<'static>> {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let label = |key: &str| Span::styled(format!("{} ", tr.t(key)), bold);
        let mut lines = Vec::new();
        let Some(plugin) = roles.plugins().get(name) else {
            lines.push(Line::styled(
                tr.f("plugins.not_described", &[("name", &name)]),
                red,
            ));
            return lines;
        };

        // The agent; red when the role's agent cannot load it.
        let agent = self.agent(roles);
        let fits = Some(plugin.agent) == agent;
        lines.push(Line::from(vec![
            label("plugins.agent"),
            Span::styled(
                plugin.agent.to_string(),
                if fits { Style::new() } else { red },
            ),
        ]));
        if !fits {
            let agent_name = agent.map_or("", AgentKind::as_str);
            let key = if agent.is_some_and(AgentKind::has_plugins) {
                "plugins.other_agent_long"
            } else {
                "plugins.agent_has_none_long"
            };
            let why = tr.f(key, &[("role", &self.role()), ("agent", &agent_name)]);
            lines.push(Line::styled(why, dim));
        }
        let from = match (&plugin.source, &plugin.commit) {
            (Some(source), Some(commit)) => {
                format!("{source} · {}", &commit[..commit.len().min(7)])
            }
            (Some(source), None) => source.clone(),
            (None, _) => tr.t("plugins.own").to_string(),
        };
        lines.push(Line::from(vec![label("plugins.from"), Span::raw(from)]));
        lines.push(Line::from(vec![
            label("plugins.folder"),
            Span::styled(plugins::relative_path(name, plugin), dim),
        ]));
        lines.push(Line::default());

        match self.details.get(name) {
            Some(Ok(details)) => {
                if let Some(version) = &details.version {
                    lines.push(Line::from(vec![
                        label("plugins.version"),
                        Span::raw(version.clone()),
                    ]));
                }
                lines.push(Line::from(vec![
                    label("plugins.inside"),
                    Span::raw(tr.f(
                        "plugins.counts",
                        &[
                            ("skills", &details.skills),
                            ("commands", &details.commands),
                            ("agents", &details.agents),
                        ],
                    )),
                ]));
                let state = |has: bool, allowed: bool| match (has, allowed) {
                    (false, _) => Span::styled(tr.t("plugins.none").to_string(), dim),
                    (true, true) => Span::styled(tr.t("plugins.allowed").to_string(), green),
                    (true, false) => Span::styled(tr.t("plugins.not_allowed").to_string(), red),
                };
                lines.push(Line::from(vec![
                    label("plugins.hooks"),
                    state(details.contents.hooks, plugin.allow_hooks),
                ]));
                lines.push(Line::from(vec![
                    label("plugins.servers"),
                    state(details.contents.servers, plugin.allow_mcp),
                ]));
                if details.contents.apps {
                    lines.push(Line::styled(tr.t("plugins.apps").to_string(), red));
                }
                if details.contents.hooks || details.contents.servers {
                    lines.push(Line::styled(tr.t("plugins.allow_hint").to_string(), dim));
                }
                if let Some(description) = &details.description {
                    lines.push(Line::default());
                    lines.push(Line::from(description.clone()));
                }
            }
            Some(Err(error)) => lines.push(Line::styled(error.clone(), red)),
            None => lines.push(Line::styled(tr.t("plugins.not_read").to_string(), dim)),
        }

        lines.push(Line::default());
        let users: Vec<&str> = ROLES
            .iter()
            .filter(|r| {
                roles
                    .settings(**r)
                    .is_some_and(|s| s.plugins.iter().any(|n| n == name))
            })
            .map(|r| r.as_str())
            .collect();
        let users = if users.is_empty() {
            tr.t("plugins.no_roles").to_string()
        } else {
            users.join(", ")
        };
        lines.push(Line::from(vec![label("plugins.roles"), Span::raw(users)]));
        lines
    }
}
