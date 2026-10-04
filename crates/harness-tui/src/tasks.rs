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
//! «To» lists a new task (the architect starts it), Lisa's answer to one of
//! the roles and approving and finishing the task; what the selected task
//! cannot take now is grey. The roles then run in the background (see
//! `runner`) and the live log shows what the agent prints. What is saved in
//! `.harness/` is read again every few seconds, so a `harness run` in another
//! terminal shows up here too.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::Result;
use harness_core::config::{Config, RoleConfig, AGENTS};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::{FileAction, NextStep, Role, Severity, Verdict};
use harness_core::models::{self, ModelList};
use harness_core::orchestrator::{self, StopReason};
use harness_core::retro::stage_text;
use harness_core::store::{self, Step, TaskStore};
use harness_core::task::{Stage, TaskState, WaitReason};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::runner::{push_line, Builder, Outcome, Request, RunChoice, Running};
use crate::theme;
use crate::ui::{buttons, panel, selected, ButtonId, Hits, ListId, Target};

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

/// One task as the tab shows it.
#[derive(Debug, Clone)]
struct TaskView {
    id: String,
    state: TaskState,
    description: String,
    steps: Vec<Step>,
    failures: usize,
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
enum Focus {
    Tasks,
    Steps,
    /// Typing the message.
    Input,
}

#[derive(Debug)]
pub struct TasksTab {
    root: PathBuf,
    /// `~/.harness`, where the agents' model lists are kept.
    home: Option<PathBuf>,
    /// What `harness.toml` says about each role.
    settings: BTreeMap<Role, RoleConfig>,
    /// The models each agent said it has.
    models: BTreeMap<String, ModelList>,
    tasks: Vec<TaskView>,
    /// `architect  claude (opus)`, one line per role.
    roles: Vec<(Option<Role>, String)>,
    /// Every task; `tasks` holds the ones the role filter lets through.
    all: Vec<TaskView>,
    /// A click on a role in «Roles» shows only the tasks waiting on it.
    pub(crate) filter: Option<Role>,
    /// A file that could not be read.
    pub problem: Option<String>,
    pub(crate) task: usize,
    pub(crate) step: usize,
    focus: Focus,
    pub(crate) scroll: u16,
    /// The message being written.
    pub(crate) input: String,
    pub(crate) choice: Choice,
    /// The task and its number of steps the choice was made for: when they
    /// change, the choice goes back to what the task needs next.
    choice_for: Option<(String, usize)>,
    /// The open list, if any.
    pub(crate) menu: Option<Menu>,
    /// The model and level chosen for the next launch; `harness.toml` is
    /// not changed. Only for the role it was chosen for.
    pub(crate) run: Option<RunChoice>,
    /// Roles working in the background.
    pub(crate) running: Option<Running>,
    /// What the agent printed in the last run.
    pub(crate) log: VecDeque<String>,
    /// A file to open in Zed, taken by the app.
    pub(crate) open: Option<PathBuf>,
    /// The window shown over the whole tab, if any.
    pub(crate) zoom: Option<Zoom>,
    /// The files of each step's commit, by step folder; a step is only kept
    /// here once its commit is there.
    commits: RefCell<HashMap<PathBuf, Vec<(char, String)>>>,
}

impl TasksTab {
    pub fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
            settings: BTreeMap::new(),
            models: BTreeMap::new(),
            tasks: Vec::new(),
            roles: Vec::new(),
            all: Vec::new(),
            filter: None,
            problem: None,
            task: 0,
            step: 0,
            focus: Focus::Tasks,
            scroll: 0,
            input: String::new(),
            choice: Choice::NewTask,
            choice_for: None,
            menu: None,
            run: None,
            running: None,
            log: VecDeque::new(),
            open: None,
            zoom: None,
            commits: RefCell::default(),
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
            Some(home) => AGENTS
                .iter()
                .filter_map(|agent| models::load(home, agent).map(|l| (agent.to_string(), l)))
                .collect(),
            None => BTreeMap::new(),
        };
        match Config::load(&harness_dir) {
            Ok(config) => {
                self.settings = config.roles.clone();
                for (role, settings) in &config.roles {
                    self.roles.push((
                        Some(*role),
                        agent_line(role_name(*role), &settings.agent, &settings.model),
                    ));
                }
                if let Some(retro) = &config.retro {
                    self.roles
                        .push((None, agent_line("retro", &retro.agent, &retro.model)));
                }
            }
            Err(error) => problems.push(error.to_string()),
        }
        self.problem = problems.into_iter().next();
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
        if at.is_none() {
            // Another task is shown now: at its latest step.
            self.step = usize::MAX;
            self.scroll = 0;
        }
        self.task = at.unwrap_or(0);
        self.step = self.step.min(self.last_step());
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
    fn show_task(&mut self, id: &str) {
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
    fn sync_choice(&mut self) {
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
    pub fn run_choice(&self) -> Option<(&str, Option<&str>, Option<&str>)> {
        let role = self.target()?;
        let settings = self.settings.get(&role)?;
        let (model, effort) = match self.run.as_ref().filter(|r| r.role == role) {
            Some(run) => (run.model.as_deref(), run.effort.as_deref()),
            None => (settings.model.as_deref(), settings.effort.as_deref()),
        };
        Some((settings.agent.as_str(), model, effort))
    }

    /// The model list of the target role's agent.
    fn model_list(&self) -> Option<&ModelList> {
        self.models.get(self.run_choice()?.0)
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
    fn cycle_choice(&mut self, delta: isize) {
        let options = self.options();
        let at = options.iter().position(|c| *c == self.choice).unwrap_or(0);
        let len = options.len() as isize;
        let next = (at as isize + delta).rem_euclid(len.max(1)) as usize;
        if let Some(choice) = options.get(next) {
            self.set_choice(*choice);
        }
    }

    /// Keys go into the message.
    pub fn typing(&self) -> bool {
        self.focus == Focus::Input
    }

    pub fn focus_input(&mut self) {
        self.focus = Focus::Input;
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Pasted text goes into the message.
    pub fn paste(&mut self, text: &str) {
        self.input
            .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        self.focus = Focus::Input;
    }

    /// «Send»: starts the work in the background. `Ok` is the message for
    /// the bottom line.
    pub fn send(&mut self, builder: Builder, tr: &I18n) -> Result<String, String> {
        if self.running.is_some() {
            return Err(tr.t("tasks.busy").to_string());
        }
        let notes = self.input.trim().to_string();
        let task = self.current().map(|t| (t.id.clone(), t.state.stage));
        let request = match (self.choice, task) {
            (Choice::NewTask, _) if notes.is_empty() => {
                return Err(tr.t("tasks.need_text").to_string())
            }
            // The same text sent twice does not start a second task.
            (Choice::NewTask, _) if self.same_task(&notes).is_some() => {
                let (task, stage) = self.same_task(&notes).unwrap_or_default();
                return Err(tr.f("tasks.duplicate", &[("task", &task), ("stage", &stage)]));
            }
            (Choice::NewTask, _) => Request::New(notes),
            (_, None) => return Err(tr.t("tasks.no_task").to_string()),
            (Choice::Role(role), Some((task, stage))) => {
                // Sending the design back to the architect rejects it.
                let rejected = role == Role::Architect
                    && stage == Stage::WaitingForHuman(WaitReason::ApproveDesign);
                Request::Decide {
                    task,
                    verdict: if rejected {
                        Verdict::Rejected
                    } else {
                        Verdict::Approved
                    },
                    next: NextStep::To(role),
                    notes,
                }
            }
            (Choice::Finish, Some((task, _))) => Request::Decide {
                task,
                verdict: Verdict::Approved,
                next: NextStep::Done,
                notes,
            },
            (Choice::Continue(_), Some((task, _))) => Request::Continue(task),
        };
        self.log.clear();
        if let (Some(role), Some((agent, model, effort))) = (self.target(), self.run_choice()) {
            let default = tr.t("tasks.default");
            let line = tr.f(
                "tasks.run_with",
                &[
                    ("role", &role_name(role)),
                    ("agent", &agent),
                    ("model", &model.unwrap_or(default)),
                    ("level", &effort.unwrap_or(default)),
                ],
            );
            push_line(&mut self.log, format!("── {line}"));
        }
        let target = self.target();
        let run = self.run.take().filter(|r| Some(r.role) == target);
        self.running = Some(Running::start(&self.root, request, run, builder));
        self.input.clear();
        self.menu = None;
        self.focus = Focus::Tasks;
        Ok(tr.t("tasks.started").to_string())
    }

    /// Takes what the background work sent. When it is over: the message
    /// for the bottom line, and whether it is a problem.
    pub fn tick(&mut self, tr: &I18n) -> Option<(String, bool)> {
        let running = self.running.as_mut()?;
        let known = running.task.clone();
        let result = running.poll(&mut self.log);
        let started = running.task.clone();
        if started != known {
            if let Some(task) = &started {
                self.show_task(task);
            }
        }
        let result = result?;
        self.running = None;
        let (text, problem) = match result {
            Ok(outcome) => {
                self.show_task(&outcome.task);
                outcome_text(&outcome, tr)
            }
            Err(error) => {
                self.reload();
                (error, true)
            }
        };
        push_line(&mut self.log, format!("── {text}"));
        Some((text, problem))
    }

    /// The unfinished task whose text is `text` (spaces and line breaks do
    /// not count), and its stage.
    fn same_task(&self, text: &str) -> Option<(String, String)> {
        let wanted = orchestrator::words(text);
        self.all
            .iter()
            .find(|t| t.state.stage != Stage::Done && orchestrator::words(&t.description) == wanted)
            .map(|t| (t.id.clone(), stage_text(t.state.stage)))
    }

    /// The stage of the task at `index`.
    #[cfg(test)]
    pub(crate) fn tasks_state(&self, index: usize) -> Option<Stage> {
        self.tasks.get(index).map(|t| t.state.stage)
    }

    fn current(&self) -> Option<&TaskView> {
        self.tasks.get(self.task)
    }

    fn last_step(&self) -> usize {
        self.current()
            .map_or(0, |task| task.steps.len().saturating_sub(1))
    }

    /// The files of `step`, as links in its «Files»: what its commit
    /// created, changed or deleted (outside the run records), then its notes
    /// and log. Before the commit is there, what its handoff lists.
    pub fn artifacts(&self, step: &Step) -> Vec<Artifact> {
        let runs = format!("{HARNESS_DIR}/runs/");
        let dir = self.root.join(&step.dir);
        let known = self.commits.borrow().get(&dir).cloned();
        let commit = known.or_else(|| {
            let handoff = dir.join("handoff.json");
            let relative = handoff.strip_prefix(&self.root).ok()?.to_str()?.to_string();
            let files = Repo::open(&self.root)
                .ok()?
                .files_of_commit_adding(&relative)?;
            self.commits.borrow_mut().insert(dir.clone(), files.clone());
            Some(files)
        });
        let changed: Vec<(char, String)> = match commit {
            Some(files) => files
                .into_iter()
                .filter(|(_, path)| !path.starts_with(&runs))
                .map(|(status, path)| {
                    let mark = match status {
                        'A' => '+',
                        'D' => '−',
                        _ => '~',
                    };
                    (mark, path)
                })
                .collect(),
            None => step
                .handoff
                .files
                .iter()
                .filter_map(|file| {
                    let mark = match file.action {
                        FileAction::Created => '+',
                        FileAction::Modified => '~',
                        FileAction::Deleted => '−',
                        FileAction::Read => return None,
                    };
                    Some((mark, file.path.clone()))
                })
                .collect(),
        };
        let mut files: Vec<Artifact> = Vec::new();
        for (mark, shown) in changed {
            let path = self.root.join(&shown);
            if !files.iter().any(|f| f.path == path) {
                files.push(Artifact { mark, shown, path });
            }
        }
        // The step's own files: their name is enough, the folder is long.
        for name in ["notes.md", "agent.log"] {
            let path = dir.join(name);
            if path.is_file() {
                files.push(Artifact {
                    mark: '·',
                    shown: name.to_string(),
                    path,
                });
            }
        }
        files
    }

    /// A click on link `index` of the selected step's «Files»: the app opens it.
    pub fn open_file(&mut self, index: usize) {
        let step = self.current().and_then(|t| t.steps.get(self.step)).cloned();
        if let Some(file) = step.and_then(|s| self.artifacts(&s).into_iter().nth(index)) {
            self.open = Some(file.path);
        }
    }

    /// A click on a window's title: over the whole tab, or back.
    pub fn toggle_zoom(&mut self, zoom: Zoom) {
        self.zoom = if self.zoom == Some(zoom) {
            None
        } else {
            Some(zoom)
        };
    }

    /// The window shown over the whole tab; the log only while there is one.
    fn zoomed(&self) -> Option<Zoom> {
        let log = self.running.is_some() || !self.log.is_empty();
        self.zoom.filter(|z| *z != Zoom::Log || log)
    }

    pub fn on_key(&mut self, key: KeyCode) {
        if self.focus == Focus::Input {
            match key {
                KeyCode::Esc | KeyCode::Tab => {
                    self.focus = Focus::Tasks;
                    self.menu = None;
                }
                KeyCode::Up => self.cycle_choice(-1),
                KeyCode::Down => self.cycle_choice(1),
                KeyCode::Enter => self.input.push('\n'),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            }
            return;
        }
        match key {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tasks => Focus::Steps,
                    Focus::Steps | Focus::Input => Focus::Input,
                }
            }
            KeyCode::Enter => self.focus = Focus::Input,
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Tasks,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Steps,
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
    }

    /// A click on a row of the task or step list.
    pub fn on_click(&mut self, list: ListId, index: usize) {
        match list {
            ListId::Tasks if index < self.tasks.len() => {
                self.focus = Focus::Tasks;
                if index != self.task {
                    self.task = index;
                    self.step = self.last_step();
                    self.sync_choice();
                }
            }
            ListId::Steps if index < self.current().map_or(0, |t| t.steps.len()) => {
                self.focus = Focus::Steps;
                self.step = index;
            }
            _ => return,
        }
        self.scroll = 0;
    }

    /// The wheel over a list moves its selection: that list gets the focus.
    pub fn on_click_list(&mut self, list: ListId) {
        match list {
            ListId::Tasks => self.focus = Focus::Tasks,
            ListId::Steps => self.focus = Focus::Steps,
            ListId::Projects
            | ListId::Roles
            | ListId::Folders
            | ListId::Choices
            | ListId::RoleFilter
            | ListId::Skills
            | ListId::Mcp
            | ListId::McpCatalog
            | ListId::Plugins
            | ListId::PluginCatalog
            | ListId::PluginCatalogs
            | ListId::Retros
            | ListId::RetroProposals
            | ListId::Agents => {}
        }
    }

    /// The mouse wheel over the step details scrolls them.
    pub fn on_wheel(&mut self, down: bool) {
        self.scroll = if down {
            self.scroll.saturating_add(3)
        } else {
            self.scroll.saturating_sub(3)
        };
    }

    fn move_by(&mut self, delta: isize) {
        let moved =
            |at: usize, len: usize| at.saturating_add_signed(delta).min(len.saturating_sub(1));
        match self.focus {
            Focus::Tasks => {
                let task = moved(self.task, self.tasks.len());
                if task != self.task {
                    self.task = task;
                    // A new task opens at its latest step.
                    self.step = self.last_step();
                    self.sync_choice();
                }
            }
            Focus::Input => {}
            Focus::Steps => {
                let len = self.current().map_or(0, |t| t.steps.len());
                self.step = moved(self.step, len);
            }
        }
        self.scroll = 0;
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [area, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(input_height(area))])
                .areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(area);
        let roles_height = u16::try_from(self.roles.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);
        // The role filter above the tasks it filters.
        let [roles_area, tasks_area] =
            Layout::vertical([Constraint::Length(roles_height), Constraint::Min(3)]).areas(left);
        // The live log takes the bottom of the right side while there is one.
        let [right, log_area] = if self.running.is_some() || !self.log.is_empty() {
            Layout::vertical([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(right)
        } else {
            [right, Rect::default()]
        };
        let [steps_area, detail_area] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(right);
        // One window over the whole tab: the others are not drawn.
        let zoomed = self.zoomed();
        let only = |zoom: Zoom, normal: Rect| match zoomed {
            None => normal,
            Some(z) if z == zoom => area,
            Some(_) => Rect::default(),
        };
        let (steps_area, detail_area, log_area) = (
            only(Zoom::Steps, steps_area),
            only(Zoom::Step, detail_area),
            only(Zoom::Log, log_area),
        );
        let (roles_area, tasks_area) = if zoomed.is_some() {
            (Rect::default(), Rect::default())
        } else {
            (roles_area, tasks_area)
        };
        self.draw_log(frame, log_area, hits, tr);

        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|t| {
                let (mark, style) = match t.state.stage {
                    Stage::Done => ("✓", theme::ok()),
                    Stage::Working(_) => ("●", theme::running()),
                    Stage::WaitingForHuman(_) => ("◆", theme::warn()),
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{}  ", t.id)),
                    Span::styled(format!("{mark} {}", short_stage(&t.state)), style),
                ]))
            })
            .collect();
        let title = match self.filter {
            Some(role) => tr.f("tasks.title_filtered", &[("role", &role_name(role))]),
            None => tr.t("tasks.title").to_string(),
        };
        draw_list(
            frame,
            hits,
            tasks_area,
            ListId::Tasks,
            &title,
            items,
            self.task,
            self.focus == Focus::Tasks,
        );
        self.draw_roles(frame, roles_area, hits, tr);

        let Some(task) = self.current() else {
            let empty = match self.filter {
                Some(role) => tr.f("tasks.empty_filtered", &[("role", &role_name(role))]),
                None => tr.t("tasks.empty").to_string(),
            };
            let empty = Paragraph::new(empty)
                .block(panel(tr.t("tasks.title"), false))
                .wrap(Wrap { trim: false });
            frame.render_widget(empty, if zoomed.is_some() { area } else { right });
            self.draw_input(frame, input_area, hits, tr);
            return;
        };

        let failures = match task.failures {
            0 => String::new(),
            n => tr.f("tasks.failures", &[("count", &n)]),
        };
        let title = tr.f(
            "tasks.header",
            &[
                ("task", &task.id),
                ("round", &task.state.round),
                ("max", &task.state.max_rounds),
                ("stage", &stage_text(task.state.stage)),
                ("failures", &failures),
            ],
        );
        let title = self.zoom_title(Zoom::Steps, &title, steps_area, hits);
        let items: Vec<ListItem> = task.steps.iter().map(step_item).collect();
        draw_list(
            frame,
            hits,
            steps_area,
            ListId::Steps,
            &title,
            items,
            self.step,
            self.focus == Focus::Steps,
        );

        let (title, text, links) = match task.steps.get(self.step) {
            Some(step) => {
                let (text, links) = step_text(step, &self.artifacts(step), tr);
                let title = tr.f(
                    "tasks.step",
                    &[
                        ("round", &step.handoff.round),
                        ("role", &role_name(step.handoff.role)),
                    ],
                );
                (title, text, links)
            }
            None => (
                " task.md ".to_string(),
                Text::from(task.description.clone()),
                Vec::new(),
            ),
        };
        let title = self.zoom_title(Zoom::Step, &title, detail_area, hits);
        let block = panel(&title, false);
        let inner = block.inner(detail_area);
        // Where each link is on the screen, after wrapping and scrolling.
        for (index, &line) in links.iter().enumerate() {
            let before = Paragraph::new(Text::from(text.lines[..line].to_vec()))
                .wrap(Wrap { trim: false })
                .line_count(inner.width);
            let row = u16::try_from(before)
                .unwrap_or(u16::MAX)
                .checked_sub(self.scroll);
            if let Some(row) = row.filter(|r| *r < inner.height) {
                let width = u16::try_from(text.lines[line].width()).unwrap_or(u16::MAX);
                hits.add(
                    Rect::new(inner.x, inner.y + row, width.min(inner.width), 1),
                    Target::Button(ButtonId::TaskFile(index)),
                );
            }
        }
        frame.render_widget(
            Paragraph::new(text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            detail_area,
        );
        self.draw_input(frame, input_area, hits, tr);
    }

    /// The agents of the roles; a click on a role filters the tasks.
    fn draw_roles(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let block = panel(tr.t("tasks.roles"), false);
        let inner = block.inner(area);
        let items: Vec<ListItem> = self
            .roles
            .iter()
            .map(|(role, line)| {
                let style = if role.is_some() && *role == self.filter {
                    selected()
                } else {
                    Style::new()
                };
                let mark = role.map_or_else(theme::retro, theme::role);
                ListItem::new(Line::from(vec![
                    Span::styled("■ ", mark),
                    Span::raw(line.clone()),
                ]))
                .style(style)
            })
            .collect();
        frame.render_widget(List::new(items).block(block), area);
        hits.add(
            inner,
            Target::List {
                list: ListId::RoleFilter,
                first: 0,
            },
        );
    }

    /// The label of an option of «To».
    fn choice_label(choice: Choice, tr: &I18n) -> String {
        match choice {
            Choice::NewTask => tr.t("tasks.choice_new").to_string(),
            Choice::Role(role) => role_name(role).to_string(),
            Choice::Continue(role) => tr.f("tasks.choice_continue", &[("role", &role_name(role))]),
            Choice::Finish => tr.t("tasks.choice_finish").to_string(),
        }
    }

    /// The message box: several lines of text, «To ▾» and «Send».
    fn draw_input(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let block = panel(tr.t("tasks.input_title"), self.focus == Focus::Input);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 {
            return;
        }

        // The buttons on the bottom row, on the right.
        let to = format!(
            "{} {} ▾",
            tr.t("tasks.to"),
            Self::choice_label(self.choice, tr)
        );
        let send = if self.running.is_some() {
            tr.t("tasks.working")
        } else {
            tr.t("tasks.send")
        };
        let default = tr.t("tasks.default");
        let mut items: Vec<(String, ButtonId, bool)> = vec![(to, ButtonId::To, true)];
        if let Some((_, model, effort)) = self.run_choice() {
            let levels = !self.level_items().is_empty();
            items.push((
                format!("{} {} ▾", tr.t("tasks.model"), model.unwrap_or(default)),
                ButtonId::Model,
                true,
            ));
            items.push((
                format!("{} {} ▾", tr.t("tasks.level"), effort.unwrap_or(default)),
                ButtonId::Level,
                levels,
            ));
        }
        items.push((send.to_string(), ButtonId::Send, self.running.is_none()));
        let width = |label: &str| crate::ui::button_width(label);
        let total = |items: &[(String, ButtonId, bool)]| {
            items
                .iter()
                .map(|(label, _, _)| width(label) + 1)
                .sum::<u16>()
                .saturating_sub(1)
        };
        // In a narrow window «Send» stays; the model and level go first.
        if total(&items) > inner.width && items.len() > 2 {
            items.drain(1..3);
        }
        let buttons_width = total(&items).min(inner.width);
        let bottom = inner.bottom() - 1;
        let row = Rect::new(inner.right() - buttons_width, bottom, buttons_width, 1);

        // The text above them; with a short window it shares the bottom row.
        let text_rows = if inner.height > 1 {
            inner.height - 1
        } else {
            1
        };
        let text_width = if inner.height > 1 {
            inner.width
        } else {
            inner.width.saturating_sub(buttons_width + 1)
        };
        let field = Rect::new(inner.x, inner.y, text_width, text_rows);
        let typing = self.focus == Focus::Input;
        let lines: Vec<Line> = if self.input.is_empty() && !typing {
            vec![Line::styled(
                format!(" {}", tr.t("tasks.input_hint")),
                theme::dim(),
            )]
        } else {
            let cursor = if typing { "▏" } else { "" };
            let room = usize::from(text_width.saturating_sub(2)).max(1);
            let wrapped = wrap(&format!("{}{cursor}", self.input), room);
            // The end of a long text stays visible: that is where one types.
            let skip = wrapped.len().saturating_sub(usize::from(text_rows));
            wrapped
                .into_iter()
                .skip(skip)
                .map(|l| Line::from(format!(" {l}")))
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), field);
        hits.add(field, Target::Button(ButtonId::Input));

        let shown: Vec<(&str, ButtonId, bool)> = items
            .iter()
            .map(|(label, id, enabled)| (label.as_str(), *id, *enabled))
            .collect();
        buttons(frame, row, hits, &shown);
        let Some(menu) = self.menu else {
            return;
        };
        // The list opens above its own button.
        let wanted = match menu {
            Menu::To => ButtonId::To,
            Menu::Model => ButtonId::Model,
            Menu::Level => ButtonId::Level,
        };
        let mut x = row.x;
        for (label, id, _) in &items {
            if *id == wanted {
                break;
            }
            x += width(label) + 1;
        }
        self.draw_menu(frame, hits, menu, x, area.y, tr);
    }

    /// The open list, above the box; what cannot be chosen now is grey.
    fn draw_menu(
        &self,
        frame: &mut Frame,
        hits: &mut Hits,
        menu: Menu,
        x: u16,
        top: u16,
        tr: &I18n,
    ) {
        let default = || tr.t("tasks.default").to_string();
        let (title, items, current): (&str, Vec<(String, bool)>, usize) = match menu {
            Menu::To => {
                let items = self.menu_items();
                let current = items
                    .iter()
                    .position(|(c, _)| *c == self.choice)
                    .unwrap_or(0);
                let labels = items
                    .iter()
                    .map(|(c, enabled)| (Self::choice_label(*c, tr), *enabled))
                    .collect();
                (tr.t("tasks.to"), labels, current)
            }
            Menu::Model | Menu::Level => {
                let (items, now) = if menu == Menu::Model {
                    (self.model_items(), self.run_choice().and_then(|c| c.1))
                } else {
                    (self.level_items(), self.run_choice().and_then(|c| c.2))
                };
                let current = items.iter().position(|m| m.as_deref() == now).unwrap_or(0);
                let labels = items
                    .into_iter()
                    .map(|m| (m.unwrap_or_else(default), true))
                    .collect();
                let title = if menu == Menu::Model {
                    tr.t("tasks.model")
                } else {
                    tr.t("tasks.level")
                };
                (title, labels, current)
            }
        };
        let width = items
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0)
            .max(title.chars().count())
            + 4;
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        let height = u16::try_from(items.len() + 2).unwrap_or(u16::MAX).min(top);
        let x = x.min(frame.area().right().saturating_sub(width));
        let area = Rect::new(x, top.saturating_sub(height), width, height);
        crate::ui::clear(frame, area);
        let list = items
            .into_iter()
            .map(|(label, enabled)| {
                let style = if enabled { Style::new() } else { theme::dim() };
                ListItem::new(Line::styled(label, style))
            })
            .collect();
        draw_list(
            frame,
            hits,
            area,
            ListId::Choices,
            title,
            list,
            current,
            true,
        );
    }

    /// What the agent prints, the latest lines at the bottom.
    /// `title` with ⤢ (or ⤡ when shown over the whole tab); a click on it
    /// toggles that.
    fn zoom_title(&self, zoom: Zoom, title: &str, area: Rect, hits: &mut Hits) -> String {
        let mark = if self.zoomed() == Some(zoom) {
            '⤡'
        } else {
            '⤢'
        };
        let title = format!(" {mark} {} ", title.trim());
        if area.height > 0 {
            let width = u16::try_from(title.chars().count()).unwrap_or(u16::MAX);
            hits.add(
                Rect::new(
                    area.x + 1,
                    area.y,
                    width.min(area.width.saturating_sub(2)),
                    1,
                ),
                Target::Button(ButtonId::TaskZoom(zoom)),
            );
        }
        title
    }

    fn draw_log(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        if area.height == 0 {
            return;
        }
        let title = match (
            &self.running,
            self.running.as_ref().and_then(|r| r.task.as_ref()),
        ) {
            (Some(_), Some(task)) => tr.f("tasks.log_running", &[("task", task)]),
            (Some(_), None) => tr.f("tasks.log_running", &[("task", &"…")]),
            (None, _) => tr.t("tasks.log_title").to_string(),
        };
        let title = self.zoom_title(Zoom::Log, &title, area, hits);
        let rows = usize::from(area.height.saturating_sub(2));
        let lines: Vec<Line> = self
            .log
            .iter()
            .skip(self.log.len().saturating_sub(rows))
            .map(|l| Line::from(l.clone()))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(panel(&title, self.running.is_some())),
            area,
        );
    }
}

/// Is it `role`'s turn in the task, or is Lisa's answer for `role` awaited?
fn waits_on(task: &TaskView, role: Role) -> bool {
    match task.state.stage {
        Stage::Working(working) => working == role,
        Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(asking)) => asking == role,
        Stage::WaitingForHuman(WaitReason::ApproveDesign) => role == Role::Architect,
        Stage::WaitingForHuman(WaitReason::RoundLimitReached) => task
            .steps
            .last()
            .is_some_and(|s| s.handoff.next_role == NextStep::To(role)),
        Stage::Done => false,
    }
}

/// `text` cut into lines of at most `width` characters; line breaks stay.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        for chunk in chars.chunks(width.max(1)) {
            lines.push(chunk.iter().collect());
        }
    }
    lines
}

/// The message after the work: what happened and what Lisa does next.
fn outcome_text(outcome: &Outcome, tr: &I18n) -> (String, bool) {
    let task = &outcome.task;
    let role = |role: &Role| role_name(*role);
    let Some(stop) = &outcome.stop else {
        return (tr.f("tasks.stop_finished", &[("task", task)]), false);
    };
    match stop {
        StopReason::Done => (tr.f("tasks.stop_done", &[("task", task)]), false),
        StopReason::WaitingForHuman(WaitReason::ApproveDesign) => {
            (tr.f("tasks.stop_design", &[("task", task)]), false)
        }
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(r)) => (
            tr.f("tasks.stop_help", &[("task", task), ("role", &role(r))]),
            false,
        ),
        StopReason::WaitingForHuman(WaitReason::RoundLimitReached) => {
            (tr.f("tasks.stop_rounds", &[("task", task)]), true)
        }
        StopReason::UsageLimitReached(r) => (
            tr.f("tasks.stop_usage", &[("task", task), ("role", &role(r))]),
            true,
        ),
        StopReason::RoleFailed { role: r, problem } => (
            tr.f(
                "tasks.stop_failed",
                &[
                    ("task", task),
                    ("role", &role(r)),
                    ("problem", &problem.lines().next().unwrap_or_default()),
                ],
            ),
            true,
        ),
        StopReason::StepLimitReached => (tr.f("tasks.stop_steps", &[("task", task)]), true),
        StopReason::DirtyWorkingTree(files) => (
            tr.f(
                "tasks.stop_dirty",
                &[("task", task), ("files", &files.join(", "))],
            ),
            true,
        ),
        StopReason::ForbiddenChanges { role: r, files } => (
            tr.f(
                "tasks.stop_forbidden",
                &[
                    ("task", task),
                    ("role", &role(r)),
                    ("files", &files.join(", ")),
                ],
            ),
            true,
        ),
        StopReason::TooLarge { role: r, files } => (
            tr.f(
                "tasks.stop_too_large",
                &[
                    ("task", task),
                    ("role", &role(r)),
                    ("files", &sized_list(files)),
                ],
            ),
            true,
        ),
        StopReason::AgentCommitted(r) => (
            tr.f(
                "tasks.stop_committed",
                &[("task", task), ("role", &role(r))],
            ),
            true,
        ),
    }
}

/// `target (812 MB), data.bin (60 MB)`
fn sized_list(files: &[(String, u64)]) -> String {
    files
        .iter()
        .map(|(path, bytes)| format!("{path} ({} MB)", bytes.div_ceil(1024 * 1024)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Room for five lines of text and the buttons; less in a small window.
fn input_height(area: Rect) -> u16 {
    match area.height {
        0..=15 => 3,
        16..=24 => 5,
        _ => 8,
    }
}

/// A bordered list whose rows can be clicked.
#[allow(clippy::too_many_arguments)]
pub fn draw_list(
    frame: &mut Frame,
    hits: &mut Hits,
    area: Rect,
    id: ListId,
    title: &str,
    items: Vec<ListItem>,
    selected_row: usize,
    focused: bool,
) {
    let block = panel(title, focused);
    let inner = block.inner(area);
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(selected())
            .highlight_symbol("▶ "),
        area,
        &mut state,
    );
    hits.add(
        inner,
        Target::List {
            list: id,
            first: state.offset(),
        },
    );
}

fn load_task(runs: &Path, id: &str) -> Result<TaskView> {
    let (store, state) = TaskStore::open(runs, id)?;
    Ok(TaskView {
        id: id.to_string(),
        state,
        description: store.description().unwrap_or_default(),
        steps: store.steps()?,
        failures: store.failures()?.len(),
    })
}

fn agent_line(who: &str, agent: &str, model: &Option<String>) -> String {
    match model {
        Some(model) => format!("{who:<10} {agent} ({model})"),
        None => format!("{who:<10} {agent}"),
    }
}

/// `done`, `working`, `waiting`: short enough for the task list.
fn short_stage(state: &TaskState) -> String {
    stage_text(state.stage)
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn step_item(step: &Step) -> ListItem<'static> {
    let h = &step.handoff;
    let next = match h.next_role {
        NextStep::To(role) => theme::role(role),
        NextStep::Done => theme::ok(),
    };
    ListItem::new(Line::from(vec![
        Span::raw(format!("r{} ", h.round)),
        Span::styled(format!("{:<10} ", role_name(h.role)), theme::role(h.role)),
        verdict_span(h.verdict),
        Span::styled(" → ", theme::dim()),
        Span::styled(format!("{:<10}", next_name(h.next_role)), next),
        Span::raw(format!(" {}", h.summary)),
    ]))
}

/// The step in full; `files` become links. Also the line of each link.
fn step_text(step: &Step, files: &[Artifact], tr: &I18n) -> (Text<'static>, Vec<usize>) {
    let h = &step.handoff;
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(vec![
            Span::raw(tr.t("tasks.verdict").to_string()),
            verdict_span(h.verdict),
            Span::raw(format!(" → {}", next_name(h.next_role))),
        ]),
        Line::from(h.summary.clone()),
    ];
    if !h.issues.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.issues").to_string(), bold));
        for issue in &h.issues {
            let theme = theme::current();
            let (name, color) = match issue.severity {
                Severity::Low => ("low", theme.dim),
                Severity::Medium => ("medium", theme.warn),
                Severity::High => ("high", theme.bad),
                Severity::Critical => ("critical", theme.bad),
            };
            let location = issue
                .location
                .as_deref()
                .map(|l| format!("{l}  "))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {name:<8} "),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{location}{}", issue.description)),
            ]));
        }
    }
    let mut links = Vec::new();
    if !files.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.files").to_string(), bold));
        for file in files {
            // A deleted file cannot be opened: grey, not a link.
            let look = if file.mark == '−' || !file.path.is_file() {
                theme::dim()
            } else {
                theme::accent().add_modifier(Modifier::UNDERLINED)
            };
            links.push(lines.len());
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", file.mark), theme::dim()),
                Span::styled(file.shown.clone(), look),
            ]));
        }
    }
    if !h.skills_used.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(tr.f(
            "tasks.skills_used",
            &[("skills", &h.skills_used.join(", "))],
        )));
    }
    if !step.notes.trim().is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.notes").to_string(), bold));
        lines.extend(step.notes.lines().map(|l| Line::from(l.to_string())));
    }
    (Text::from(lines), links)
}

fn verdict_span(verdict: Verdict) -> Span<'static> {
    match verdict {
        Verdict::Approved => Span::styled("approved", theme::ok()),
        Verdict::Rejected => Span::styled("rejected", theme::bad()),
        Verdict::NeedsHuman => Span::styled("needs_human", theme::warn()),
    }
}

fn next_name(next: NextStep) -> &'static str {
    match next {
        NextStep::To(role) => role_name(role),
        NextStep::Done => "done",
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Architect => "architect",
        Role::Developer => "developer",
        Role::Tester => "tester",
        Role::Security => "security",
        Role::Human => "human",
    }
}
