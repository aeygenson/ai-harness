//! The rows of the Roles tab's details, and keys, clicks and the wheel on them.

use ratatui::crossterm::event::KeyCode;

use harness_core::config::{AgentKind, RoleConfig};

use super::tab::{toggle, Action, Focus, RolesTab, Row, WHO};
use crate::ui::i18n::I18n;

impl RolesTab {
    /// The rows of the details, in order.
    pub fn rows(&self) -> Vec<Row> {
        // Only agents ready to work, and the one chosen now even if it is not.
        let chosen = self.choice().0;
        let mut rows: Vec<Row> = AgentKind::ALL
            .into_iter()
            .filter(|&a| self.is_ready(a) || chosen == Some(a))
            .map(Row::Agent)
            .collect();
        rows.push(Row::Model(None));
        if let Some(list) = self.model_list() {
            rows.extend(list.models.iter().map(|m| Row::Model(Some(m.id.clone()))));
        }
        rows.push(Row::OtherModel);
        if !self.effort_levels().is_empty() || self.choice().2.is_some() {
            rows.push(Row::Effort);
        }
        let Some(role) = self.who() else {
            return rows;
        };
        let settings = self.roles.get(&role);
        let listed = |list: fn(&RoleConfig) -> &Vec<String>| -> Vec<String> {
            settings.map(|s| list(s).clone()).unwrap_or_default()
        };
        let mut skills: Vec<String> = self.skills.iter().map(|s| s.name.clone()).collect();
        for name in listed(|s| &s.skills)
            .into_iter()
            .chain(listed(|s| &s.always_skills))
        {
            if !skills.contains(&name) {
                skills.push(name);
            }
        }
        rows.extend(skills.into_iter().map(Row::Skill));
        let mut servers: Vec<String> = self.saved.mcp.keys().cloned().collect();
        for name in listed(|s| &s.mcp) {
            if !servers.contains(&name) {
                servers.push(name);
            }
        }
        rows.extend(servers.into_iter().map(Row::Mcp));
        let agent = settings.map(|s| s.agent);
        let mut plugins: Vec<String> = self
            .saved
            .plugins
            .iter()
            .filter(|(_, p)| Some(p.agent) == agent)
            .map(|(name, _)| name.clone())
            .collect();
        for name in listed(|s| &s.plugins) {
            if !plugins.contains(&name) {
                plugins.push(name);
            }
        }
        rows.extend(plugins.into_iter().map(Row::Plugin));
        rows
    }

    #[expect(
        clippy::match_same_arms,
        reason = "the arms form a key table: one arm per key and focus"
    )]
    pub fn on_key(&mut self, key: KeyCode, tr: &I18n) -> Action {
        let rows = self.rows().len();
        match (key, self.focus) {
            (KeyCode::Tab | KeyCode::Right, Focus::List) => self.focus = Focus::Details,
            (KeyCode::Tab | KeyCode::Left | KeyCode::Esc, Focus::Details) => {
                self.focus = Focus::List;
            }
            (KeyCode::Up | KeyCode::Char('k'), Focus::List) => {
                self.select(self.selected.saturating_sub(1));
            }
            (KeyCode::Down | KeyCode::Char('j'), Focus::List) => self.select(self.selected + 1),
            (KeyCode::Up | KeyCode::Char('k'), Focus::Details) => {
                self.row = self.row.saturating_sub(1);
            }
            (KeyCode::Down | KeyCode::Char('j'), Focus::Details) => {
                self.row = (self.row + 1).min(rows.saturating_sub(1));
            }
            (KeyCode::Enter | KeyCode::Char(' '), Focus::List) => self.focus = Focus::Details,
            (KeyCode::Enter | KeyCode::Char(' '), Focus::Details) => {
                return self.activate(self.row, tr)
            }
            _ => {}
        }
        Action::None
    }

    pub fn select(&mut self, index: usize) {
        let index = index.min(WHO.len() - 1);
        if index != self.selected {
            self.selected = index;
            self.row = 0;
        }
        self.focus = Focus::List;
    }

    /// The wheel over the details moves the chosen row.
    pub fn on_wheel(&mut self, down: bool) {
        let last = self.rows().len().saturating_sub(1);
        self.focus = Focus::Details;
        self.row = if down {
            (self.row + 1).min(last)
        } else {
            self.row.saturating_sub(1)
        };
    }

    /// A click or Enter on row `index` of the details.
    pub fn activate(&mut self, index: usize, tr: &I18n) -> Action {
        let rows = self.rows();
        let Some(row) = rows.get(index).cloned() else {
            return Action::None;
        };
        self.row = index;
        self.focus = Focus::Details;
        let who = self.who();
        match row {
            Row::Agent(agent) => self.set_agent(agent, tr),
            Row::Model(model) => {
                let effort = match (&model, self.model_list()) {
                    (None, _) => None,
                    (Some(id), Some(list)) => {
                        list.find(id).and_then(|m| m.effort_for(self.choice().2))
                    }
                    (Some(_), None) => self.choice().2.map(str::to_string),
                };
                self.set_choice(model, effort);
                Action::None
            }
            Row::OtherModel => {
                let typed = self
                    .agent()
                    .1
                    .filter(|m| self.model_list().is_none_or(|l| l.find(m).is_none()));
                Action::EditModel(typed.unwrap_or_default().to_string())
            }
            Row::Effort => {
                // The agent's default -> each level -> the agent's default.
                let mut levels: Vec<Option<String>> = vec![None];
                levels.extend(self.effort_levels().into_iter().map(Some));
                let (_, model, effort) = self.choice();
                let at = levels
                    .iter()
                    .position(|l| l.as_deref() == effort)
                    .map_or(0, |i| (i + 1) % levels.len());
                let model = model.map(str::to_string);
                self.set_choice(model, levels[at].clone());
                Action::None
            }
            Row::Skill(name) => {
                if let Some(role) = who {
                    self.cycle_skill(role, &name);
                }
                Action::None
            }
            Row::Mcp(name) => {
                if let Some(settings) = who.and_then(|role| self.roles.get_mut(&role)) {
                    toggle(&mut settings.mcp, name);
                }
                Action::None
            }
            Row::Plugin(name) => {
                if let Some(settings) = who.and_then(|role| self.roles.get_mut(&role)) {
                    toggle(&mut settings.plugins, name);
                }
                Action::None
            }
        }
    }
}
