//! Drawing the Plugins tab: the role line, the plugin list and the buttons.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::catalog_draw::draw_catalog;
use super::catalogs_draw::draw_catalogs;
use super::tab::{PluginButton, PluginsTab};
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

impl PluginsTab {
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
        self.draw_role_line(frame, top, hits, tr, roles);

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(main);
        self.draw_plugin_list(frame, left, hits, tr, roles);

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

        self.draw_role_buttons(frame, bottom, hits, tr, roles);
        self.draw_plugin_buttons(frame, plugins_row, hits, tr, roles);
    }

    /// The role selector, with the role's agent on the right when there is room.
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
    }

    /// The plugins the role can have, marked when it has them.
    fn draw_plugin_list(
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
    }

    /// The buttons for the selected plugin itself; they change harness.toml at once.
    fn draw_plugin_buttons(
        &self,
        frame: &mut Frame,
        plugins_row: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let busy = self.busy.as_deref();
        let current = self.current(roles);
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
}
