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

use std::path::{Path, PathBuf};

use harness_agents::credentials;
use harness_core::config::{McpConfig, AGENTS};
use harness_core::handoff::Role;
use harness_core::mcp::{self, SECRET_PREFIX};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::roles_tab::RolesTab;
use crate::skills_tab::{role_name, ROLES};
use crate::tasks::draw_list;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

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
}

impl McpTab {
    pub fn load(home: Option<&Path>) -> Self {
        let mut tab = Self {
            home: home.map(Path::to_path_buf),
            role: 0,
            row: 0,
            secrets: Vec::new(),
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

    pub fn on_key(&mut self, key: KeyCode, roles: &RolesTab) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            _ => {}
        }
    }

    fn has(&self, roles: &RolesTab, name: &str) -> bool {
        roles
            .settings(self.role())
            .is_some_and(|s| s.mcp.iter().any(|n| n == name))
    }

    /// The secrets a server needs that are not saved.
    fn missing_secrets(&self, server: &McpConfig) -> Vec<String> {
        server
            .env
            .values()
            .filter_map(|v| v.strip_prefix(SECRET_PREFIX))
            .map(|n| n.trim().to_string())
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
        let [top, main, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);
        let role = self.role();
        let names: Vec<&str> = ROLES.iter().map(|r| role_name(*r)).collect();
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
                Some(model) => format!("{} · {} ({model}) ", role_name(role), settings.agent),
                None => format!("{} · {} ", role_name(role), settings.agent),
            };
            let width = u16::try_from(text.chars().count()).unwrap_or(0);
            let x = top.right().saturating_sub(width);
            if x > top.x + 60 {
                frame.render_widget(
                    Span::styled(text, Style::new().fg(Color::DarkGray)),
                    Rect::new(x, top.y, width, 1),
                );
            }
        }

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(main);
        let dim = Style::new().fg(Color::DarkGray);
        let red = Style::new().fg(Color::LightRed);
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
        let title = tr.f("mcp.title", &[("role", &role_name(role))]);
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
            Some(name) if self.has(roles, name) => tr.f("mcp.take", &[("role", &role_name(role))]),
            _ => tr.f("mcp.give", &[("role", &role_name(role))]),
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
                Span::styled(note.to_string(), Style::new().fg(Color::Yellow)),
                Rect::new(x.max(bottom.x), bottom.y, width.min(bottom.width), 1),
            );
        }
    }

    /// What the details show about the server `name`.
    fn details(&self, roles: &RolesTab, name: &str, tr: &I18n) -> Vec<Line<'static>> {
        let dim = Style::new().fg(Color::DarkGray);
        let red = Style::new().fg(Color::LightRed);
        let green = Style::new().fg(Color::Green);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        let Some(server) = roles.servers().get(name) else {
            lines.push(Line::styled(
                tr.f("mcp.not_described", &[("name", &name)]),
                red,
            ));
            return lines;
        };
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

        lines.push(Line::default());
        let users: Vec<&str> = ROLES
            .iter()
            .filter(|r| {
                roles
                    .settings(**r)
                    .is_some_and(|s| s.mcp.iter().any(|n| n == name))
            })
            .map(|r| role_name(*r))
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
