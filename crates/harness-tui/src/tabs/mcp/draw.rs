//! Drawing the MCP tab: the role line, the server list and the buttons.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::catalog_draw::draw_catalog;
use super::tab::{is_oauth, secret_names, McpButton, McpTab};
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

impl McpTab {
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
        self.draw_role_line(frame, top, hits, tr, roles);

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(main);
        self.draw_server_list(frame, left, hits, tr, roles);

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

        self.draw_role_buttons(frame, bottom, hits, tr, roles);
        self.draw_server_buttons(frame, servers_row, hits, tr, roles);
    }

    /// The role selector, with the role's agent and model on the right when there is room.
    fn draw_role_line(
        &self,
        frame: &mut Frame,
        top: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
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
    }

    /// The servers the role can have, marked when it has them.
    fn draw_server_list(
        &self,
        frame: &mut Frame,
        left: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let role = self.role();
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
    }

    /// «Give» or «Take», «Save» and «Undo»: they change the role.
    fn draw_role_buttons(
        &self,
        frame: &mut Frame,
        bottom: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let role = self.role();
        let current = self.current(roles);
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
    }

    /// The buttons for the servers themselves; they change harness.toml at once.
    fn draw_server_buttons(
        &self,
        frame: &mut Frame,
        servers_row: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let current = self.current(roles);
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
}
