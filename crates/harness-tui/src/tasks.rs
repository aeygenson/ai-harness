//! The Tasks tab: tasks, the steps of the selected task, and one step in full.
//!
//! It only reads what is saved in `.harness/` (through the same core functions
//! as `harness status`) and is reloaded every few seconds, so a `harness run`
//! in another terminal shows up here.

use std::path::{Path, PathBuf};

use anyhow::Result;
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;
use harness_core::handoff::{NextStep, Role, Severity, Verdict};
use harness_core::retro::stage_text;
use harness_core::store::{self, Step, TaskStore};
use harness_core::task::TaskState;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::ui::{panel, selected, Hits, ListId, Target};

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
}

#[derive(Debug)]
pub struct TasksTab {
    root: PathBuf,
    tasks: Vec<TaskView>,
    /// `architect  claude (opus)`, one line per role.
    roles: Vec<String>,
    /// A file that could not be read.
    pub problem: Option<String>,
    pub(crate) task: usize,
    pub(crate) step: usize,
    focus: Focus,
    pub(crate) scroll: u16,
}

impl TasksTab {
    pub fn load(root: &Path) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            tasks: Vec::new(),
            roles: Vec::new(),
            problem: None,
            task: 0,
            step: 0,
            focus: Focus::Tasks,
            scroll: 0,
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

        self.tasks.clear();
        match store::task_ids(&runs) {
            Ok(ids) => {
                for id in ids {
                    match load_task(&runs, &id) {
                        Ok(task) => self.tasks.push(task),
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
                    self.roles.push(agent_line(
                        role_name(*role),
                        &settings.agent,
                        &settings.model,
                    ));
                }
                if let Some(retro) = &config.retro {
                    self.roles
                        .push(agent_line("retro", &retro.agent, &retro.model));
                }
            }
            Err(error) => problems.push(error.to_string()),
        }
        self.problem = problems.into_iter().next();

        self.task = selected
            .and_then(|id| self.tasks.iter().position(|t| t.id == id))
            .unwrap_or(0);
        self.step = self.step.min(self.last_step());
    }

    fn current(&self) -> Option<&TaskView> {
        self.tasks.get(self.task)
    }

    fn last_step(&self) -> usize {
        self.current()
            .map_or(0, |task| task.steps.len().saturating_sub(1))
    }

    pub fn on_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tasks => Focus::Steps,
                    Focus::Steps => Focus::Tasks,
                }
            }
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
            ListId::Projects => {}
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
                }
            }
            Focus::Steps => {
                let len = self.current().map_or(0, |t| t.steps.len());
                self.step = moved(self.step, len);
            }
        }
        self.scroll = 0;
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(area);
        let roles_height = u16::try_from(self.roles.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);
        let [tasks_area, roles_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(roles_height)]).areas(left);
        let [steps_area, detail_area] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(right);

        let items: Vec<ListItem> = self
            .tasks
            .iter()
            .map(|t| ListItem::new(format!("{}  {}", t.id, short_stage(&t.state))))
            .collect();
        draw_list(
            frame,
            hits,
            tasks_area,
            ListId::Tasks,
            tr.t("tasks.title"),
            items,
            self.task,
            self.focus == Focus::Tasks,
        );

        let roles: Vec<Line> = self.roles.iter().map(|r| Line::from(r.as_str())).collect();
        frame.render_widget(
            Paragraph::new(roles).block(panel(tr.t("tasks.roles"), false)),
            roles_area,
        );

        let Some(task) = self.current() else {
            let empty = Paragraph::new(tr.t("tasks.empty").to_string())
                .block(panel(tr.t("tasks.title"), false));
            frame.render_widget(empty, right);
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
