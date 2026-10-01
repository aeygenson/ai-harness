//! The Tasks tab: tasks, the steps of the selected task, and one step in full.
//! At the bottom a box to act: a text of several lines, whom it goes to,
//! and «Send».
//!
//! ```text
//! ┌ Message ─────────────────────────────────────────────────────┐
//! │ Use serde for the parser.                                    │
//! │ Keep the public API as in the design▏                        │
//! │                               [ To: developer ▾ ] [ Send ]   │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! «To» lists a new task (the architect starts it), Lisa's answer to one of
//! the roles and approving and finishing the task; what the selected task
//! cannot take now is grey. The roles then run in the background (see
//! `runner`) and the live log shows what the agent prints. What is saved in
//! `.harness/` is read again every few seconds, so a `harness run` in another
//! terminal shows up here too.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::Result;
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;
use harness_core::handoff::{NextStep, Role, Severity, Verdict};
use harness_core::orchestrator::StopReason;
use harness_core::retro::stage_text;
use harness_core::store::{self, Step, TaskStore};
use harness_core::task::{Stage, TaskState, WaitReason};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::runner::{push_line, Builder, Outcome, Request, Running};
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

/// One task as the tab shows it.
#[derive(Debug, Clone)]
struct TaskView {
    id: String,
    state: TaskState,
    description: String,
    steps: Vec<Step>,
    failures: usize,
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
    /// The «To» list is open.
    pub(crate) menu: bool,
    /// Roles working in the background.
    pub(crate) running: Option<Running>,
    /// What the agent printed in the last run.
    pub(crate) log: VecDeque<String>,
}

impl TasksTab {
    pub fn load(root: &Path) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
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
            menu: false,
            running: None,
            log: VecDeque::new(),
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
        match Config::load(&harness_dir) {
            Ok(config) => {
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
            self.choice = self.default_choice();
            self.choice_for = now;
        }
    }

    /// Picks the option at `index` of the «To» list; `false` if it is grey.
    pub fn choose(&mut self, index: usize) -> bool {
        self.menu = false;
        match self.menu_items().get(index) {
            Some((choice, true)) => {
                self.choice = *choice;
                true
            }
            Some((_, false)) => false,
            None => true,
        }
    }

    /// The next (`1`) or previous (`-1`) option of «To».
    fn cycle_choice(&mut self, delta: isize) {
        let options = self.options();
        let at = options.iter().position(|c| *c == self.choice).unwrap_or(0);
        let len = options.len() as isize;
        let next = (at as isize + delta).rem_euclid(len.max(1)) as usize;
        if let Some(choice) = options.get(next) {
            self.choice = *choice;
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
        self.running = Some(Running::start(&self.root, request, builder));
        self.input.clear();
        self.menu = false;
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

    pub fn on_key(&mut self, key: KeyCode) {
        if self.focus == Focus::Input {
            match key {
                KeyCode::Esc | KeyCode::Tab => {
                    self.focus = Focus::Tasks;
                    self.menu = false;
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
            | ListId::Skills => {}
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
        self.draw_log(frame, log_area, tr);

        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|t| ListItem::new(format!("{}  {}", t.id, short_stage(&t.state))))
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
            frame.render_widget(empty, right);
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

        let (title, text) = match task.steps.get(self.step) {
            Some(step) => (
                tr.f(
                    "tasks.step",
                    &[
                        ("round", &step.handoff.round),
                        ("role", &role_name(step.handoff.role)),
                    ],
                ),
                step_text(step, tr),
            ),
            None => (
                " task.md ".to_string(),
                Text::from(task.description.clone()),
            ),
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(panel(&title, false))
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
                ListItem::new(Line::styled(line.clone(), style))
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
        let width = |label: &str| u16::try_from(label.chars().count() + 4).unwrap_or(u16::MAX);
        let buttons_width = (width(&to) + 1 + width(send)).min(inner.width);
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
                Style::new().fg(Color::DarkGray),
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

        buttons(
            frame,
            row,
            hits,
            &[
                (&to, ButtonId::To, true),
                (send, ButtonId::Send, self.running.is_none()),
            ],
        );
        if self.menu {
            self.draw_menu(frame, hits, row, area.y, tr);
        }
    }

    /// The open «To» list, above the box; what cannot be chosen now is grey.
    fn draw_menu(&self, frame: &mut Frame, hits: &mut Hits, row: Rect, top: u16, tr: &I18n) {
        let items = self.menu_items();
        let labels: Vec<String> = items
            .iter()
            .map(|(c, _)| Self::choice_label(*c, tr))
            .collect();
        let width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 4;
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        let height = u16::try_from(items.len() + 2).unwrap_or(u16::MAX).min(top);
        let x = row.x.min(frame.area().right().saturating_sub(width));
        let area = Rect::new(x, top.saturating_sub(height), width, height);
        frame.render_widget(Clear, area);
        let current = items
            .iter()
            .position(|(c, _)| *c == self.choice)
            .unwrap_or(0);
        let list = labels
            .into_iter()
            .zip(&items)
            .map(|(label, (_, enabled))| {
                let style = if *enabled {
                    Style::new()
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                ListItem::new(Line::styled(label, style))
            })
            .collect();
        draw_list(
            frame,
            hits,
            area,
            ListId::Choices,
            tr.t("tasks.to"),
            list,
            current,
            true,
        );
    }

    /// What the agent prints, the latest lines at the bottom.
    fn draw_log(&self, frame: &mut Frame, area: Rect, tr: &I18n) {
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
        StopReason::AgentCommitted(r) => (
            tr.f(
                "tasks.stop_committed",
                &[("task", task), ("role", &role(r))],
            ),
            true,
        ),
    }
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
            .highlight_symbol("> "),
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
    ListItem::new(Line::from(vec![
        Span::raw(format!("r{} {:<10} ", h.round, role_name(h.role))),
        verdict_span(h.verdict),
        Span::raw(format!(" → {:<10} {}", next_name(h.next_role), h.summary)),
    ]))
}

fn step_text(step: &Step, tr: &I18n) -> Text<'static> {
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
            let (name, color) = match issue.severity {
                Severity::Low => ("low", Color::Gray),
                Severity::Medium => ("medium", Color::Yellow),
                Severity::High => ("high", Color::LightRed),
                Severity::Critical => ("critical", Color::Red),
            };
            let location = issue
                .location
                .as_deref()
                .map(|l| format!("{l}  "))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("  {name:<8} "), Style::new().fg(color)),
                Span::raw(format!("{location}{}", issue.description)),
            ]));
        }
    }
    if !h.files.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.files").to_string(), bold));
        for file in &h.files {
            let action = serde_json::to_value(file.action)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            lines.push(Line::from(format!("  {action:<9} {}", file.path)));
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
    Text::from(lines)
}

fn verdict_span(verdict: Verdict) -> Span<'static> {
    match verdict {
        Verdict::Approved => Span::styled("approved", Style::new().fg(Color::Green)),
        Verdict::Rejected => Span::styled("rejected", Style::new().fg(Color::Red)),
        Verdict::NeedsHuman => Span::styled("needs_human", Style::new().fg(Color::Yellow)),
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
