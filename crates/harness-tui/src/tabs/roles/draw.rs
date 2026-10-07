//! Drawing the Roles tab: the role list and the details of the selected role.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use harness_core::config::{independence, AgentKind};
use harness_core::task::handoff::Role;

use super::tab::{Focus, RolesTab, Row, WHO};
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selected, ButtonId, Hits, ListId, Target};

impl RolesTab {
    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [bar, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let changed = self.changed();
        buttons(
            frame,
            bar,
            hits,
            &[
                (tr.t("roles.save"), ButtonId::Save, changed),
                (tr.t("roles.undo"), ButtonId::Undo, changed),
                (
                    tr.t("roles.refresh"),
                    ButtonId::RefreshModels,
                    !self.refreshing,
                ),
            ],
        );
        if changed {
            let note = tr.t("roles.unsaved");
            let width = u16::try_from(note.chars().count()).unwrap_or(0);
            let x = bar.right().saturating_sub(width);
            frame.render_widget(
                Span::styled(note.to_string(), theme::warn()),
                Rect::new(x.max(bar.x), bar.y, width.min(bar.width), 1),
            );
        }
        let [left, right] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(0)]).areas(main);

        let items: Vec<ListItem> = WHO
            .iter()
            .map(|who| {
                let (name, agent) = match who {
                    Some(role) => (
                        role.as_str(),
                        self.roles.get(role).map(|r| r.agent.as_str()),
                    ),
                    None => ("retro", self.retro.as_ref().map(|r| r.agent.as_str())),
                };
                let dirty = match who {
                    Some(role) => self.roles.get(role) != self.saved.roles.get(role),
                    None => self.retro != self.saved.retro,
                };
                let mark = if dirty { "*" } else { " " };
                ListItem::new(format!("{name:<10}{mark}{}", agent.unwrap_or("—")))
            })
            .collect();
        draw_list(
            frame,
            hits,
            left,
            ListId::Roles,
            tr.t("roles.title"),
            items,
            self.selected,
            self.focus == Focus::List,
        );

        let name = self.who().map_or("retro", Role::as_str);
        let block = panel(&format!(" {name} "), self.focus == Focus::Details);
        let inner = block.inner(right);
        frame.render_widget(block, right);

        let lines = self.detail_lines(tr);
        // Keep the chosen row on the screen.
        let chosen_line = lines
            .iter()
            .position(|(row, _)| *row == Some(self.row))
            .unwrap_or(0);
        let height = usize::from(inner.height);
        let first = (chosen_line + 1).saturating_sub(height);
        for (y, (row, line)) in lines.into_iter().skip(first).take(height).enumerate() {
            let rect = Rect::new(
                inner.x,
                inner.y + u16::try_from(y).unwrap_or(0),
                inner.width,
                1,
            );
            let line = match row {
                Some(index) if index == self.row && self.focus == Focus::Details => {
                    line.style(selected())
                }
                _ => line,
            };
            frame.render_widget(Paragraph::new(line), rect);
            if let Some(index) = row {
                hits.add(rect, Target::Row(index));
            }
        }
    }

    /// A warning on the Developer and Security when both use the same agent
    /// and model (see `harness_core::config::independence`); no lines otherwise.
    fn same_reviewer_warning(
        &self,
        who: Option<Role>,
        tr: &I18n,
    ) -> Vec<(Option<usize>, Line<'static>)> {
        let shown = matches!(who, Some(Role::Developer | Role::Security))
            && independence::security_same_as_developer(&self.roles);
        if !shown {
            return Vec::new();
        }
        let mut lines: Vec<(Option<usize>, Line<'static>)> =
            ["roles.same_reviewer", "roles.same_reviewer_hint"]
                .into_iter()
                .map(|key| (None, Line::styled(tr.t(key).to_string(), theme::warn())))
                .collect();
        lines.push((None, Line::default()));
        lines
    }

    /// The lines of the details; clickable ones carry their row index.
    fn detail_lines(&self, tr: &I18n) -> Vec<(Option<usize>, Line<'static>)> {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let dim = theme::dim();
        let mut lines = Vec::new();
        if self.problem.is_some() {
            return lines;
        }
        let (agent, model, effort) = self.choice();
        let list = self.model_list();
        let who = self.who();
        let settings = who.and_then(|role| self.roles.get(&role));
        let heading = |key: &str| (None, Line::styled(tr.t(key).to_string(), bold));
        lines.extend(self.same_reviewer_warning(who, tr));

        lines.push(heading(if who.is_some() {
            "roles.agent"
        } else {
            "roles.retro_agent"
        }));
        // Each group gets its heading before its first row.
        let (mut skills_seen, mut mcp_seen, mut plugins_seen) = (false, false, false);
        for (index, row) in self.rows().into_iter().enumerate() {
            let line = match row {
                Row::Agent(name) => self.agent_line(name, agent, tr),
                Row::Model(None) => {
                    lines.push((None, Line::default()));
                    lines.push(heading("roles.model"));
                    let mark = if model.is_none() { "(•)" } else { "( )" };
                    Line::from(format!("  {mark} {}", tr.t("roles.default_model")))
                }
                Row::Model(Some(id)) => self.model_line(&id, model, tr),
                Row::OtherModel => {
                    let line = self.other_model_line(model, tr);
                    if list.is_none() {
                        lines.push((Some(index), line));
                        let hint = tr.f(
                            "roles.no_models",
                            &[("agent", &agent.map_or("", AgentKind::as_str))],
                        );
                        lines.push((None, Line::styled(hint, dim)));
                        continue;
                    }
                    line
                }
                Row::Effort => {
                    lines.push((None, Line::default()));
                    self.effort_line(effort, tr)
                }
                Row::Skill(name) => {
                    if !skills_seen {
                        skills_seen = true;
                        lines.push((None, Line::default()));
                        lines.push(heading("roles.skills"));
                        lines.push((
                            None,
                            Line::styled(tr.t("roles.skills_hint").to_string(), dim),
                        ));
                    }
                    self.skill_line(&name, settings, tr)
                }
                Row::Mcp(name) => {
                    if !mcp_seen {
                        mcp_seen = true;
                        lines.push((None, Line::default()));
                        lines.push(heading("roles.mcp"));
                    }
                    self.mcp_line(&name, settings, tr)
                }
                Row::Plugin(name) => {
                    if !plugins_seen {
                        plugins_seen = true;
                        let title = tr.f(
                            "roles.plugins",
                            &[("agent", &agent.map_or("", AgentKind::as_str))],
                        );
                        lines.push((None, Line::default()));
                        lines.push((None, Line::styled(title, bold)));
                    }
                    self.plugin_line(&name, settings, tr)
                }
            };
            lines.push((Some(index), line));
        }
        if who.is_some() {
            for (present, key) in [
                (skills_seen, "roles.no_skills"),
                (mcp_seen, "roles.no_mcp"),
                (plugins_seen, "roles.no_plugins"),
            ] {
                if !present {
                    lines.push((None, Line::default()));
                    lines.push((None, Line::styled(tr.t(key).to_string(), dim)));
                }
            }
        } else {
            lines.push((None, Line::default()));
            lines.push((
                None,
                Line::styled(tr.t("roles.retro_hint").to_string(), dim),
            ));
        }
        lines
    }
}
