//! The right side of the Plugins tab: everything about the selected plugin.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use harness_core::config::{AgentKind, PluginConfig};
use harness_core::plugins::{self};

use super::tab::PluginsTab;
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::ROLES;
use crate::ui::i18n::I18n;
use crate::ui::theme;

impl PluginsTab {
    /// What the details show about the plugin `name`.
    pub(super) fn details(&self, roles: &RolesTab, name: &str, tr: &I18n) -> Vec<Line<'static>> {
        let dim = theme::dim();
        let red = theme::bad();
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

        lines.extend(self.contents_lines(name, plugin, tr));

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

    /// What the plugin `name` has inside, read from its folder, and what is allowed.
    fn contents_lines(&self, name: &str, plugin: &PluginConfig, tr: &I18n) -> Vec<Line<'static>> {
        let dim = theme::dim();
        let red = theme::bad();
        let green = theme::ok();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let label = |key: &str| Span::styled(format!("{} ", tr.t(key)), bold);
        let mut lines = Vec::new();
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
        lines
    }
}
