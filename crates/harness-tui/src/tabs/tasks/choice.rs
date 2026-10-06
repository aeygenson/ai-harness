//! The message box's choices: whom the message goes to, and the model and level
//! of the role that runs first.

use harness_core::config::AgentKind;
use harness_core::models::ModelList;
use harness_core::task::handoff::{NextStep, Role};
use harness_core::task::{Stage, WaitReason};

use super::tab::TasksTab;
use crate::tabs::tasks::runner::RunChoice;

impl TasksTab {
    /// Everything «To» lists, and whether it can be chosen now. The roles
    /// are always listed, so it is clear who can get a message; they are
    /// grey until the selected task waits for Lisa's answer.
    pub fn menu_items(&self) -> Vec<(Choice, bool)> {
        let stage = self.current().map(|t| t.state.stage);
        let waiting = matches!(stage, Some(Stage::WaitingForHuman(_)));
        let mut items = vec![(Choice::NewTask, true)];
        if let Some(Stage::Working(role)) = stage {
            items.push((Choice::Continue(role), true));
        }
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            items.push((Choice::Role(role), waiting));
        }
        items.push((Choice::Finish, waiting));
        items
    }

    /// What can be chosen in «To» now.
    pub fn options(&self) -> Vec<Choice> {
        self.menu_items()
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(choice, _)| choice)
            .collect()
    }

    /// What the selected task needs next: the developer after the design, the
    /// role that asked for help, a new task when it is done.
    fn default_choice(&self) -> Choice {
        let Some(task) = self.current() else {
            return Choice::NewTask;
        };
        match task.state.stage {
            Stage::Done => Choice::NewTask,
            Stage::Working(role) => Choice::Continue(role),
            Stage::WaitingForHuman(WaitReason::ApproveDesign) => Choice::Role(Role::Developer),
            Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => Choice::Role(role),
            Stage::WaitingForHuman(WaitReason::RoundLimitReached) => {
                let suggested = task.steps.last().and_then(|s| match s.handoff.next_role {
                    NextStep::To(role) if role != Role::Human => Some(role),
                    _ => None,
                });
                Choice::Role(suggested.unwrap_or(Role::Developer))
            }
        }
    }

    /// A new task, or a new step in it, brings back the default choice.
    pub(super) fn sync_choice(&mut self) {
        let now = self.current().map(|t| (t.id.clone(), t.steps.len()));
        if now != self.choice_for || !self.options().contains(&self.choice) {
            self.set_choice(self.default_choice());
            self.choice_for = now;
        }
    }

    /// Another addressee: a model or level chosen for another role is
    /// forgotten.
    fn set_choice(&mut self, choice: Choice) {
        self.choice = choice;
        if self.run.as_ref().map(|r| r.role) != self.target() {
            self.run = None;
        }
    }

    /// Opens `menu`, or closes it if it is open.
    pub fn toggle_menu(&mut self, menu: Menu) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
    }

    /// Picks the option at `index` of `menu`; `false` if it is grey.
    pub fn choose(&mut self, menu: Menu, index: usize) -> bool {
        self.menu = None;
        match menu {
            Menu::To => match self.menu_items().get(index) {
                Some((choice, true)) => {
                    self.set_choice(*choice);
                    true
                }
                Some((_, false)) => false,
                None => true,
            },
            Menu::Model => {
                if let Some(model) = self.model_items().get(index).cloned() {
                    self.pick_model(model);
                }
                true
            }
            Menu::Level => {
                if let Some(effort) = self.level_items().get(index).cloned() {
                    self.pick_level(effort);
                }
                true
            }
        }
    }

    /// The role that runs first after «Send», whose model and level
    /// «Model» and «Level» set; none when the task is only finished.
    pub fn target(&self) -> Option<Role> {
        match self.choice {
            Choice::NewTask => Some(Role::Architect),
            Choice::Role(role) | Choice::Continue(role) => Some(role),
            Choice::Finish => None,
        }
    }

    /// The agent, model and level the target role gets in the next launch:
    /// what was chosen here, otherwise what `harness.toml` says. `None` when
    /// no role runs or the role has no settings.
    pub fn run_choice(&self) -> Option<(AgentKind, Option<&str>, Option<&str>)> {
        let role = self.target()?;
        let settings = self.settings.get(&role)?;
        let (model, effort) = match self.run.as_ref().filter(|r| r.role == role) {
            Some(run) => (run.model.as_deref(), run.effort.as_deref()),
            None => (settings.model.as_deref(), settings.effort.as_deref()),
        };
        Some((settings.agent, model, effort))
    }

    /// The model list of the target role's agent.
    fn model_list(&self) -> Option<&ModelList> {
        self.models.get(&self.run_choice()?.0)
    }

    /// What «Model» lists: the agent's default (`None`), its models, and
    /// the models in use that the list does not have.
    pub fn model_items(&self) -> Vec<Option<String>> {
        let Some((_, model, _)) = self.run_choice() else {
            return Vec::new();
        };
        let mut items = vec![None];
        if let Some(list) = self.model_list() {
            items.extend(list.models.iter().map(|m| Some(m.id.clone())));
        }
        let configured = self
            .target()
            .and_then(|role| self.settings.get(&role))
            .and_then(|s| s.model.as_deref());
        for extra in [configured, model].into_iter().flatten() {
            if !items.iter().any(|m| m.as_deref() == Some(extra)) {
                items.push(Some(extra.to_string()));
            }
        }
        items
    }

    /// What «Level» lists: the agent's default (`None`) and the levels the
    /// model takes; empty if it takes none or nothing is known about it.
    pub fn level_items(&self) -> Vec<Option<String>> {
        let Some((_, model, _)) = self.run_choice() else {
            return Vec::new();
        };
        let Some(list) = self.model_list() else {
            return Vec::new();
        };
        let found = match model {
            Some(id) => list.find(id),
            None => list.models.iter().find(|m| m.default),
        };
        let levels = found.map(|m| m.efforts.clone()).unwrap_or_default();
        if levels.is_empty() {
            return Vec::new();
        }
        std::iter::once(None)
            .chain(levels.into_iter().map(Some))
            .collect()
    }

    /// Another model for the next launch; the level stays if the model
    /// takes it, otherwise the model's own default level.
    fn pick_model(&mut self, model: Option<String>) {
        let (Some(role), Some((_, _, effort))) = (self.target(), self.run_choice()) else {
            return;
        };
        let effort = effort.map(str::to_string);
        let found = self.model_list().and_then(|list| match &model {
            Some(id) => list.find(id),
            None => list.models.iter().find(|m| m.default),
        });
        let effort = match (effort, found) {
            (None, _) => None,
            (Some(e), Some(m)) => m.effort_for(Some(&e)),
            (Some(e), None) => Some(e),
        };
        self.run = Some(RunChoice {
            role,
            model,
            effort,
        });
    }

    /// Another level for the next launch.
    fn pick_level(&mut self, effort: Option<String>) {
        let (Some(role), Some((_, model, _))) = (self.target(), self.run_choice()) else {
            return;
        };
        self.run = Some(RunChoice {
            role,
            model: model.map(str::to_string),
            effort,
        });
    }

    /// The next (`1`) or previous (`-1`) option of «To».
    pub(super) fn cycle_choice(&mut self, delta: isize) {
        let options = self.options();
        let at = options.iter().position(|c| *c == self.choice).unwrap_or(0);
        // `try_from` instead of `as`: a list this short always fits in `isize`.
        let len = isize::try_from(options.len()).unwrap_or(isize::MAX);
        let at = isize::try_from(at).unwrap_or(0);
        let next = (at + delta).rem_euclid(len.max(1)) as usize;
        if let Some(choice) = options.get(next) {
            self.set_choice(*choice);
        }
    }
}

/// Whom the message goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The text is a new task; the architect starts.
    NewTask,
    /// The text is Lisa's answer to this role, which then runs.
    Role(Role),
    /// A role stopped part-way: run it again.
    Continue(Role),
    /// Approve and finish the task.
    Finish,
}

/// The open list of the message box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    /// Whom the message goes to.
    To,
    /// The model of the role that runs first, for this launch.
    Model,
    /// Its effort level, for this launch.
    Level,
}
