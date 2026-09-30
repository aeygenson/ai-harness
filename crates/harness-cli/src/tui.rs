//! `harness tui`: a full-screen view of the project's tasks in the terminal.
//!
//! ```text
//! ┌ Tasks ─────────────┐┌ task-001 · round 2 of 5 · done ──────────────────┐
//! │> task-001  done    ││  r1 architect  approved → human    Design ready  │
//! │  task-002  working ││> r2 tester     rejected → developer  2 fail      │
//! ├ Roles ─────────────┤├ round 2 · tester ────────────────────────────────┤
//! │ architect  claude  ││ verdict: rejected → developer                    │
//! │ tester     codex   ││ Issues:  high  src/parser.rs:42  Panics on ""    │
//! └────────────────────┘└──────────────────────────────────────────────────┘
//!  ↑↓ move · ←→ tasks/steps · PgUp/PgDn scroll · r reload · q quit
//! ```
//!
//! This first version only shows what is saved in `.harness/`: it reads the
//! same files as `harness status`, through the same core functions. It reloads
//! them every few seconds, so a `harness run` in another terminal shows up here.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;
use harness_core::handoff::{NextStep, Role, Severity, Verdict};
use harness_core::retro::stage_text;
use harness_core::store::{self, Step, TaskStore};
use harness_core::task::TaskState;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

/// How often the files are read again.
const RELOAD_EVERY: Duration = Duration::from_secs(3);

/// Opens the view for the project in `root` and runs until `q`.
pub fn run(root: &Path) -> Result<()> {
    let mut app = App::load(root);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    // Always give the terminal back, even after an error.
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut loaded = Instant::now();
    while !app.quit {
        terminal.draw(|frame| draw(frame, app))?;
        // Wait a little for a key; without one, go on to the reload check.
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code);
                }
            }
        }
        if loaded.elapsed() >= RELOAD_EVERY {
            app.reload();
            loaded = Instant::now();
        }
    }
    Ok(())
}

/// One task as the view shows it.
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

/// Everything on the screen. Keys change it; `draw` only reads it.
#[derive(Debug)]
struct App {
    root: PathBuf,
    tasks: Vec<TaskView>,
    /// `architect  claude (opus)`, one line per role.
    roles: Vec<String>,
    /// A file that could not be read; shown at the bottom.
    problem: Option<String>,
    task: usize,
    step: usize,
    focus: Focus,
    scroll: u16,
    quit: bool,
}

impl App {
    fn load(root: &Path) -> Self {
        let mut app = Self {
            root: root.to_path_buf(),
            tasks: Vec::new(),
            roles: Vec::new(),
            problem: None,
            task: 0,
            step: 0,
            focus: Focus::Tasks,
            scroll: 0,
            quit: false,
        };
        app.reload();
        app.step = app.last_step();
        app
    }

    /// Reads everything again, keeping the selected task and step.
    fn reload(&mut self) {
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

    fn on_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('r') => self.reload(),
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

fn draw(frame: &mut Frame, app: &App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(main);
    let roles_height = u16::try_from(app.roles.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let [tasks_area, roles_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(roles_height)]).areas(left);
    let [steps_area, detail_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(right);

    // Tasks.
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|t| ListItem::new(format!("{}  {}", t.id, short_stage(&t.state))))
        .collect();
    let mut state = ListState::default().with_selected(Some(app.task));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(" Tasks ", app.focus == Focus::Tasks))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> "),
        tasks_area,
        &mut state,
    );

    // Roles.
    let roles: Vec<Line> = app.roles.iter().map(|r| Line::from(r.as_str())).collect();
    frame.render_widget(
        Paragraph::new(roles).block(panel(" Roles ", false)),
        roles_area,
    );

    let Some(task) = app.current() else {
        let empty = Paragraph::new(
            "No tasks yet.\n\nCreate one with:\n  harness task new task-001 \"what to build\"",
        )
        .block(panel(" Tasks ", false));
        frame.render_widget(empty, right);
        draw_footer(frame, footer, app);
        return;
    };

    // Steps of the selected task.
    let title = format!(
        " {} · round {} of {} · {}{} ",
        task.id,
        task.state.round,
        task.state.max_rounds,
        stage_text(task.state.stage),
        match task.failures {
            0 => String::new(),
            n => format!(" · {n} failed attempts"),
        }
    );
    let items: Vec<ListItem> = task.steps.iter().map(step_item).collect();
    let mut state = ListState::default().with_selected(Some(app.step));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(&title, app.focus == Focus::Steps))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> "),
        steps_area,
        &mut state,
    );

    // The selected step, or the task itself before the first step.
    let (title, text) = match task.steps.get(app.step) {
        Some(step) => (
            format!(
                " round {} · {} ",
                step.handoff.round,
                role_name(step.handoff.role)
            ),
            step_text(step),
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
            .scroll((app.scroll, 0)),
        detail_area,
    );
    draw_footer(frame, footer, app);
}

fn draw_footer(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " ↑↓ move · ←→ tasks/steps · PgUp/PgDn scroll · r reload · q quit",
        Style::new().fg(Color::DarkGray),
    )];
    if let Some(problem) = &app.problem {
        spans.push(Span::styled(
            format!("   {problem}"),
            Style::new().fg(Color::Red),
        ));
    }
    frame.render_widget(Line::from(spans), area);
}

fn panel(title: &str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    };
    Block::bordered()
        .title(title.to_string())
        .border_style(style)
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
    let verdict = verdict_span(h.verdict);
    ListItem::new(Line::from(vec![
        Span::raw(format!("r{} {:<10} ", h.round, role_name(h.role))),
        verdict,
        Span::raw(format!(" → {:<10} {}", next_name(h.next_role), h.summary)),
    ]))
}

fn step_text(step: &Step) -> Text<'static> {
    let h = &step.handoff;
    let mut lines = vec![
        Line::from(vec![
            Span::raw("verdict: "),
            verdict_span(h.verdict),
            Span::raw(format!(" → {}", next_name(h.next_role))),
        ]),
        Line::from(h.summary.clone()),
    ];
    if !h.issues.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            "Issues:",
            Style::new().add_modifier(Modifier::BOLD),
        ));
        for issue in &h.issues {
            let (name, color) = match issue.severity {
                Severity::Low => ("low", Color::Gray),
                Severity::Medium => ("medium", Color::Yellow),
                Severity::High => ("high", Color::LightRed),
                Severity::Critical => ("critical", Color::Red),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {name:<8} "), Style::new().fg(color)),
                Span::raw(format!(
                    "{}{}",
                    issue
                        .location
                        .as_deref()
                        .map(|l| format!("{l}  "))
                        .unwrap_or_default(),
                    issue.description
                )),
            ]));
        }
    }
    if !h.files.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            "Files:",
            Style::new().add_modifier(Modifier::BOLD),
        ));
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
        lines.push(Line::from(format!(
            "Skills used: {}",
            h.skills_used.join(", ")
        )));
    }
    if !step.notes.trim().is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            "Notes:",
            Style::new().add_modifier(Modifier::BOLD),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::git::Repo;
    use harness_core::handoff::{Handoff, Issue};
    use harness_core::orchestrator;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn handoff(role: Role, verdict: Verdict, next: NextStep, summary: &str) -> Handoff {
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round: 1,
            role,
            verdict,
            next_role: next,
            summary: summary.into(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
        }
    }

    /// A project with task-001 (three steps) and an empty task-002.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        crate::init(dir.path()).unwrap();
        let (store, mut state) =
            orchestrator::create_task(&repo, "task-001", "Build a parser", 5).unwrap();
        let steps = [
            handoff(
                Role::Architect,
                Verdict::Approved,
                NextStep::To(Role::Human),
                "Design ready",
            ),
            handoff(
                Role::Human,
                Verdict::Approved,
                NextStep::To(Role::Developer),
                "Go",
            ),
            handoff(
                Role::Developer,
                Verdict::Approved,
                NextStep::To(Role::Tester),
                "Parser written",
            ),
        ];
        for step in &steps {
            store.record(&mut state, step, "Some notes.").unwrap();
        }
        let mut tester = handoff(
            Role::Tester,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
            "Empty input fails",
        );
        tester.issues = vec![Issue {
            severity: Severity::High,
            location: Some("src/parser.rs:42".into()),
            description: "Panics on empty input".into(),
        }];
        store
            .record(&mut state, &tester, "Tester notes here.")
            .unwrap();
        orchestrator::create_task(&repo, "task-002", "Second task", 5).unwrap();
        dir
    }

    fn screen(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_tasks_roles_and_the_latest_step() {
        let dir = project();
        let app = App::load(dir.path());
        assert_eq!(app.tasks.len(), 2);
        assert_eq!(app.problem, None);
        let text = screen(&app);
        for part in [
            "> task-001  working",
            "task-002  working",
            "architect  claude",
            "retro      claude",
            "task-001 · round 2 of 5 · working: developer",
            "r1 architect  approved → human",
            "> r1 tester",
            "round 1 · tester",
            "verdict: rejected → developer",
            "high     src/parser.rs:42  Panics on empty input",
            "Tester notes here.",
            "q quit",
        ] {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
    }

    #[test]
    fn keys_move_between_tasks_and_steps() {
        let dir = project();
        let mut app = App::load(dir.path());
        assert_eq!((app.task, app.step), (0, 3));

        app.on_key(KeyCode::Right);
        app.on_key(KeyCode::Up);
        app.on_key(KeyCode::Up);
        assert_eq!(app.step, 1);
        assert!(screen(&app).contains("round 1 · human"));

        app.on_key(KeyCode::PageDown);
        assert_eq!(app.scroll, 10);
        app.on_key(KeyCode::Down);
        assert_eq!((app.step, app.scroll), (2, 0));

        // The second task has no steps yet: its task.md is shown.
        app.on_key(KeyCode::Tab);
        app.on_key(KeyCode::Down);
        app.on_key(KeyCode::Down);
        assert_eq!((app.task, app.step), (1, 0));
        let text = screen(&app);
        assert!(text.contains("task.md"), "{text}");
        assert!(text.contains("Second task"), "{text}");

        // A reload keeps the selected task.
        app.reload();
        assert_eq!(app.task, 1);

        app.on_key(KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn an_empty_project_explains_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        Repo::init(dir.path()).unwrap();
        let app = App::load(dir.path());
        let text = screen(&app);
        assert!(text.contains("No tasks yet."), "{text}");
        // Without harness.toml the problem is shown at the bottom.
        assert!(app.problem.is_some());
    }
}
