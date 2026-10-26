//! The Tasks tab: tasks, the steps of the selected task, and one step in full.
//! In the step, «Files» are links: what the step created or changed (from
//! its git commit), its notes and its log; a click opens one in Zed.
//! A click on the title of the steps, the step or the agent log (⤢) shows
//! that window over the whole tab; another click or Esc puts it back (⤡).
//! At the bottom a box to act: a text of several lines, whom it goes to,
//! and «Send».
//!
//! ```text
//! ┌ Message ─────────────────────────────────────────────────────┐
//! │ Use serde for the parser.                                    │
//! │ Keep the public API as in the design▏                        │
//! │      [ To: developer ▾ ] [ Model: opus ▾ ] [ Level: high ▾ ] [ Send ]
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! «Model» and «Level» are for the role that runs first, for this launch
//! only; they start from what `harness.toml` says and `harness.toml` is not
//! changed.
//!
//! «To» lists a new task (its workflow's first role starts it), the user's answer to one of
//! the roles and approving and finishing the task; what the selected task
//! cannot take now is grey. The roles then run in the background (see
//! `runner`) and the live log shows what the agent prints. What is saved in
//! `.harness/` is read again every few seconds, so a `harness run` in another
//! terminal shows up here too.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use harness_core::config::{AgentKind, Config, RoleConfig};
use harness_core::git::HARNESS_DIR;
use harness_core::models::{self, ModelList};
use harness_core::retro::usage::Usage;
use harness_core::task::handoff::Role;
use harness_core::task::scenario::Scenario;
use harness_core::task::store::{self, Step};
use harness_core::task::{Stage, TaskState};

use super::choice::{Choice, Menu};
use super::live_log::LiveLog;
use super::message_text::MessageScroll;
use super::steps::{agent_line, load_task, waits_on};
use super::times::TaskTimes;
use crate::tabs::tasks::runner::{RunChoice, Running};

/// One task as the tab shows it.
#[derive(Debug, Clone)]
pub(super) struct TaskView {
    pub(super) id: String,
    pub(super) state: TaskState,
    pub(super) description: String,
    pub(super) steps: Vec<Step>,
    pub(super) failures: usize,
}

/// A file of the selected step, as a link in its «Files».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// `+` created, `~` changed, `−` deleted; `·` the step's own notes and log.
    pub mark: char,
    /// The path as shown: from the project folder, or the name of the step's file.
    pub shown: String,
    pub path: PathBuf,
}

/// A window of the right side shown over the whole tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Steps,
    Step,
    Log,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Tasks,
    Steps,
    /// The live log: the arrow and page keys scroll it.
    Log,
    /// Typing the message.
    Input,
}

#[derive(Debug)]
pub struct TasksTab {
    pub(super) root: PathBuf,
    /// `~/.harness`, where the agents' model lists are kept.
    home: Option<PathBuf>,
    /// The project's scenario: «To» lists its roles.
    pub(super) scenario: Scenario,
    /// The project is on GitHub (`[github]`): a done task becomes a pull request.
    pub(super) on_github: bool,
    /// What `harness.toml` says about each role.
    pub(super) settings: BTreeMap<Role, RoleConfig>,
    /// The models each agent said it has.
    pub(super) models: BTreeMap<AgentKind, ModelList>,
    pub(super) tasks: Vec<TaskView>,
    /// `architect  claude (opus)`, one line per role.
    pub(super) roles: Vec<(Option<Role>, String)>,
    /// Every task; `tasks` holds the ones the role filter lets through.
    pub(super) all: Vec<TaskView>,
    /// A click on a role in «Roles» shows only the tasks waiting on it.
    pub(crate) filter: Option<Role>,
    /// A file that could not be read.
    pub problem: Option<String>,
    pub(crate) task: usize,
    pub(crate) step: usize,
    /// The first task on screen, kept between frames so the list does not
    /// jump when a task already on screen is chosen (see `draw_list_from`).
    pub(super) first_task_row: Cell<usize>,
    /// The first step on screen, kept the same way.
    pub(super) first_step_row: Cell<usize>,
    pub(super) focus: Focus,
    pub(crate) scroll: u16,
    /// The message being written.
    pub(crate) input: String,
    /// How far the message is scrolled back from its end.
    pub(super) input_scroll: MessageScroll,
    pub(crate) choice: Choice,
    /// The task, its number of steps and its stage the choice was made for:
    /// when they change, the choice goes back to what the task needs next.
    pub(super) choice_for: Option<(String, usize, Stage)>,
    /// The open list, if any.
    pub(crate) menu: Option<Menu>,
    /// The model and level chosen for the next launch; `harness.toml` is
    /// not changed. Only for the role it was chosen for.
    pub(crate) run: Option<RunChoice>,
    /// Roles working in the background.
    pub(crate) running: Option<Running>,
    /// What the agent printed in the last run.
    pub(crate) log: LiveLog,
    /// The background work checked out a task's branch: the other tabs
    /// show settings read from the old one and must read them again.
    pub(crate) branch_changed: bool,
    /// A file to open in Zed, taken by the app.
    pub(crate) open: Option<PathBuf>,
    /// The window shown over the whole tab, if any.
    pub(crate) zoom: Option<Zoom>,
    /// The files of each step's commit, by step folder; a step is only kept
    /// here once its commit is there.
    pub(super) commits: RefCell<HashMap<PathBuf, Vec<(char, String)>>>,
    /// The tokens and cost in each step's `agent.log`, by step folder. A log
    /// does not change once written, so each is read only once.
    pub(super) usages: RefCell<HashMap<PathBuf, Option<Usage>>>,
    /// When each task and its steps were committed, by task id.
    pub(super) times: HashMap<String, TaskTimes>,
}

impl TasksTab {
    pub fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
            scenario: Scenario::default(),
            on_github: false,
            settings: BTreeMap::new(),
            models: BTreeMap::new(),
            tasks: Vec::new(),
            roles: Vec::new(),
            all: Vec::new(),
            filter: None,
            problem: None,
            task: 0,
            step: 0,
            first_task_row: Cell::new(0),
            first_step_row: Cell::new(0),
            focus: Focus::Tasks,
            scroll: 0,
            input: String::new(),
            input_scroll: MessageScroll::default(),
            choice: Choice::NewTask,
            choice_for: None,
            menu: None,
            run: None,
            running: None,
            log: LiveLog::default(),
            branch_changed: false,
            open: None,
            zoom: None,
            commits: RefCell::default(),
            usages: RefCell::default(),
            times: HashMap::new(),
        };
        tab.reload();
        tab.step = tab.last_step();
        tab
    }

    /// Reads everything again, keeping the selected task and step.
    pub fn reload(&mut self) {
        let selected = self.tasks.get(self.task).map(|t| t.id.clone());
        let harness_dir = self.root.join(HARNESS_DIR);
        let runs = harness_dir.join("runs");
        let mut problems = Vec::new();

        self.all.clear();
        match store::task_ids(&runs) {
            Ok(ids) => {
                for id in ids {
                    match load_task(&runs, &id) {
                        Ok(task) => self.all.push(task),
                        Err(error) => problems.push(format!("{id}: {error}")),
                    }
                }
            }
            Err(error) => problems.push(error.to_string()),
        }

        self.roles.clear();
        self.models = match &self.home {
            Some(home) => AgentKind::ALL
                .into_iter()
                .filter_map(|agent| models::load(home, agent).map(|l| (agent, l)))
                .collect(),
            None => BTreeMap::new(),
        };
        match Config::load(&harness_dir) {
            Ok(config) => {
                self.scenario = config.scenario;
                self.on_github = config.github.is_some();
                self.settings = config.roles.clone();
                for (role, settings) in &config.roles {
                    self.roles.push((
                        Some(*role),
                        agent_line(role.as_str(), settings.agent, settings.model.as_deref()),
                    ));
                }
                if let Some(retro) = &config.retro {
                    self.roles.push((
                        None,
                        agent_line("retro", retro.agent, retro.model.as_deref()),
                    ));
                }
            }
            Err(error) => problems.push(error.to_string()),
        }
        self.problem = problems.into_iter().next();
        self.refresh_times();
        self.apply_filter(selected);
    }

    /// Lets through the tasks of the filter's role, keeping `selected` if it
    /// is still there.
    fn apply_filter(&mut self, selected: Option<String>) {
        let filter = self.filter;
        self.tasks = self
            .all
            .iter()
            .filter(|t| filter.is_none_or(|role| waits_on(t, role)))
            .cloned()
            .collect();
        let at = selected.and_then(|id| self.tasks.iter().position(|t| t.id == id));
        self.task = at.unwrap_or(0);
        self.step = if at.is_some() {
            self.step.min(self.last_step())
        } else {
            // Another task is shown now: at its latest step.
            self.scroll = 0;
            self.last_step()
        };
        self.sync_choice();
    }

    /// A click on a line of «Roles»: show only the tasks waiting on that
    /// role; a second click on it shows all again.
    pub fn toggle_filter(&mut self, index: usize) {
        let Some((Some(role), _)) = self.roles.get(index) else {
            return;
        };
        self.filter = if self.filter == Some(*role) {
            None
        } else {
            Some(*role)
        };
        let selected = self.current().map(|t| t.id.clone());
        self.apply_filter(selected);
    }

    /// Shows the task `id` at its latest step.
    pub(super) fn show_task(&mut self, id: &str) {
        self.reload();
        if !self.tasks.iter().any(|t| t.id == id) && self.filter.is_some() {
            // The filter must not hide what was just sent.
            self.filter = None;
            self.apply_filter(Some(id.to_string()));
        }
        if let Some(index) = self.tasks.iter().position(|t| t.id == id) {
            self.task = index;
            self.step = self.last_step();
            self.scroll = 0;
            self.sync_choice();
        }
    }

    /// The stage of the task at `index`.
    #[cfg(test)]
    pub(crate) fn tasks_state(&self, index: usize) -> Option<harness_core::task::Stage> {
        self.tasks.get(index).map(|t| t.state.stage)
    }

    pub(super) fn current(&self) -> Option<&TaskView> {
        self.tasks.get(self.task)
    }

    pub(super) fn last_step(&self) -> usize {
        self.current()
            .map_or(0, |task| task.steps.len().saturating_sub(1))
    }
}
