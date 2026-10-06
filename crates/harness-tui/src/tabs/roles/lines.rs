//! One line of the Roles tab's details each: an agent, a model, a level, a skill,
//! an MCP server or a plugin.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use harness_core::config::{AgentKind, RoleConfig};

use super::tab::{RolesTab, SkillFile};
use crate::ui::i18n::I18n;
use crate::ui::theme;

impl RolesTab {
    /// An agent to choose, with a note when it is not installed or signed in.
    pub(super) fn agent_line(
        &self,
        name: AgentKind,
        chosen: Option<AgentKind>,
        tr: &I18n,
    ) -> Line<'static> {
        let mark = if chosen == Some(name) { "(•)" } else { "( )" };
        let mut spans = vec![Span::raw(format!("  {mark} {name:<16}"))];
        if !self.is_ready(name) {
            spans.push(Span::styled(
                tr.t("roles.not_ready").to_string(),
                theme::bad(),
            ));
        }
        Line::from(spans)
    }

    /// «Other model»: chosen when the role's model is not in the agent's list.
    pub(super) fn other_model_line(&self, model: Option<&str>, tr: &I18n) -> Line<'static> {
        let list = self.model_list();
        let typed = model.filter(|m| list.is_none_or(|l| l.find(m).is_none()));
        match typed {
            Some(m) => Line::from(vec![
                Span::raw(format!(
                    "  (•) {} ",
                    tr.f("roles.model_typed", &[("model", &m)])
                )),
                Span::styled(tr.t("roles.model_hint").to_string(), theme::dim()),
            ]),
            None => Line::from(format!("  ( ) {}", tr.t("roles.model_other"))),
        }
    }

    /// A model of the agent's list, with what the list says about it.
    pub(super) fn model_line(&self, id: &str, model: Option<&str>, tr: &I18n) -> Line<'static> {
        let dim = theme::dim();
        let list = self.model_list();
        let mark = if model == Some(id) { "(•)" } else { "( )" };
        let about = list
            .and_then(|l| l.find(id))
            .map(|m| {
                let default = if m.default {
                    format!("{} ", tr.t("roles.agents_default"))
                } else {
                    String::new()
                };
                format!("{default}{}", m.name.clone().unwrap_or_default())
            })
            .unwrap_or_default();
        Line::from(vec![
            Span::raw(format!("  {mark} {id:<24} ")),
            Span::styled(about, dim),
        ])
    }

    /// The effort levels in one line, the chosen one in brackets.
    pub(super) fn effort_line(&self, effort: Option<&str>, tr: &I18n) -> Line<'static> {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let dim = theme::dim();
        let mut spans = vec![Span::styled(format!("{:<10}", tr.t("roles.effort")), bold)];
        let mut levels: Vec<Option<String>> = vec![None];
        levels.extend(self.effort_levels().into_iter().map(Some));
        if effort.is_some() && !levels.iter().any(|l| l.as_deref() == effort) {
            levels.push(effort.map(str::to_string));
        }
        for (i, level) in levels.iter().enumerate() {
            let name = level
                .clone()
                .unwrap_or_else(|| tr.t("roles.default_effort").to_string());
            if i > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            if level.as_deref() == effort {
                spans.push(Span::styled(format!("[{name}]"), bold));
            } else {
                spans.push(Span::styled(name, dim));
            }
        }
        spans.push(Span::styled(
            format!("   {}", tr.t("roles.effort_hint")),
            dim,
        ));
        Line::from(spans)
    }

    /// A skill: whether the role always uses it, may use it, or not, and its description.
    pub(super) fn skill_line(
        &self,
        name: &str,
        settings: Option<&RoleConfig>,
        tr: &I18n,
    ) -> Line<'static> {
        let dim = theme::dim();
        let mark = if settings.is_some_and(|s| s.always_skills.iter().any(|n| n == name)) {
            "[■]"
        } else if settings.is_some_and(|s| s.skills.iter().any(|n| n == name)) {
            "[x]"
        } else {
            "[ ]"
        };
        let about = match self.skills.iter().find(|s| s.name == name) {
            None => Span::styled(tr.t("roles.no_file").to_string(), theme::bad()),
            Some(SkillFile {
                description: None, ..
            }) => Span::styled(tr.t("roles.no_description").to_string(), theme::bad()),
            Some(SkillFile {
                description: Some(d),
                ..
            }) => Span::styled(d.clone(), dim),
        };
        Line::from(vec![Span::raw(format!("  {mark} {name:<24} ")), about])
    }

    /// An MCP server: whether the role has it, and its command.
    pub(super) fn mcp_line(
        &self,
        name: &str,
        settings: Option<&RoleConfig>,
        tr: &I18n,
    ) -> Line<'static> {
        let dim = theme::dim();
        let on = settings.is_some_and(|s| s.mcp.iter().any(|n| n == name));
        let about = match self.saved.mcp.get(name) {
            Some(server) => Span::styled(server.command.clone().unwrap_or_default(), dim),
            None => Span::styled(tr.t("roles.not_described").to_string(), theme::bad()),
        };
        Line::from(vec![
            Span::raw(format!("  {} {name:<24} ", check(on))),
            about,
        ])
    }

    /// A plugin: whether the role has it, and where it came from.
    pub(super) fn plugin_line(
        &self,
        name: &str,
        settings: Option<&RoleConfig>,
        tr: &I18n,
    ) -> Line<'static> {
        let dim = theme::dim();
        let on = settings.is_some_and(|s| s.plugins.iter().any(|n| n == name));
        let about = match self.saved.plugins.get(name) {
            Some(plugin) => Span::styled(plugin.source.clone().unwrap_or_default(), dim),
            None => Span::styled(tr.t("roles.not_described").to_string(), theme::bad()),
        };
        Line::from(vec![
            Span::raw(format!("  {} {name:<24} ", check(on))),
            about,
        ])
    }
}

pub(super) fn check(on: bool) -> &'static str {
    if on {
        "[x]"
    } else {
        "[ ]"
    }
}
