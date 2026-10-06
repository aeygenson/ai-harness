//! Choosing a role's agent, model and effort level on the Roles tab.

use harness_core::config::{AgentKind, RetroConfig, RoleConfig};
use harness_core::models::ModelList;

use super::tab::{Action, RolesTab};
use crate::ui::i18n::I18n;

impl RolesTab {
    /// The model list of the selected role's agent.
    pub(super) fn model_list(&self) -> Option<&ModelList> {
        self.agent().0.and_then(|agent| self.models.get(&agent))
    }

    /// The levels the chosen model takes (for the agent's default model,
    /// those of the model the agent calls its default).
    pub(super) fn effort_levels(&self) -> Vec<String> {
        let Some(list) = self.model_list() else {
            return Vec::new();
        };
        let model = match self.agent().1 {
            Some(id) => list.find(id),
            None => list.models.iter().find(|m| m.default),
        };
        model.map(|m| m.efforts.clone()).unwrap_or_default()
    }

    /// Sets the model and effort of the selected role or of `[retro]`.
    pub(super) fn set_choice(&mut self, model: Option<String>, effort: Option<String>) {
        match self.who() {
            Some(role) => {
                if let Some(settings) = self.roles.get_mut(&role) {
                    (settings.model, settings.effort) = (model, effort);
                }
            }
            None => {
                if let Some(retro) = &mut self.retro {
                    (retro.model, retro.effort) = (model, effort);
                }
            }
        }
    }

    /// What a role gets when it moves to `agent`: the model and level of
    /// another role on that agent, otherwise the agent's own default.
    fn first_choice(&self, agent: AgentKind) -> (Option<String>, Option<String>) {
        let who = self.who();
        let other = self
            .roles
            .iter()
            .filter(|(role, _)| Some(**role) != who)
            .map(|(_, r)| (r.agent, &r.model, &r.effort))
            .chain(
                self.retro
                    .iter()
                    .filter(|_| who.is_some())
                    .map(|r| (r.agent, &r.model, &r.effort)),
            )
            .find(|(a, model, _)| *a == agent && model.is_some());
        match other {
            Some((_, model, effort)) => (model.clone(), effort.clone()),
            None => self
                .models
                .get(&agent)
                .map_or((None, None), ModelList::default_choice),
        }
    }

    pub(super) fn set_agent(&mut self, agent: AgentKind, tr: &I18n) -> Action {
        if let Some(role) = self.who() {
            let first = self.first_choice(agent);
            let settings = self.roles.entry(role).or_insert_with(|| RoleConfig {
                agent,
                model: None,
                effort: None,
                skills: Vec::new(),
                always_skills: Vec::new(),
                mcp: Vec::new(),
                plugins: Vec::new(),
            });
            if settings.agent == agent {
                return Action::None;
            }
            // A model belongs to one agent: take the new agent's.
            (settings.model, settings.effort) = first;
            settings.agent = agent;
            // Plugins are made for one kind of agent.
            let plugins = &self.saved.plugins;
            let (keep, drop): (Vec<String>, Vec<String>) = settings
                .plugins
                .drain(..)
                .partition(|name| plugins.get(name).is_some_and(|p| p.agent == agent));
            settings.plugins = keep;
            if !drop.is_empty() {
                return Action::Say(tr.f(
                    "roles.plugins_dropped",
                    &[("plugins", &drop.join(", ")), ("agent", &agent)],
                ));
            }
        } else {
            if self.retro.as_ref().is_some_and(|r| r.agent == agent) {
                return Action::None;
            }
            let (model, effort) = self.first_choice(agent);
            self.retro = Some(RetroConfig {
                agent,
                model,
                effort,
            });
        }
        Action::None
    }

    /// The model from the form; empty means the agent's own default. The
    /// level stays if the model is in the list and takes it.
    pub fn set_model(&mut self, model: &str) {
        let model = Some(model.trim().to_string()).filter(|m| !m.is_empty());
        let effort = self.choice().2.map(str::to_string);
        let effort = match (&model, self.model_list()) {
            (None, _) => None,
            (Some(id), Some(list)) => match list.find(id) {
                Some(found) => found.effort_for(effort.as_deref()),
                None => effort,
            },
            (Some(_), None) => effort,
        };
        self.set_choice(model, effort);
    }
}
