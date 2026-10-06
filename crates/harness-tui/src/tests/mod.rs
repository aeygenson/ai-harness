//! Tests of the whole TUI, one file per tab. They drive `App` with keys and clicks on a
//! `TestBackend` screen. This file holds the helpers the tab files share: a temporary
//! `~/.harness` (`Env`), a sample project, screen reading, typing and clicking.

use std::fs;

use super::*;
use crate::tabs::projects::has_config;
use crate::ui::message::{Message, MessageKind};
use crate::ui::ButtonId;
use harness_core::config::projects::Projects;
use harness_core::git::Repo;
use harness_core::models;
use harness_core::task::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};
use harness_core::task::orchestrator;
use harness_core::task::store::TaskStore;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use std::time::Duration;

mod agents_tab;
mod language;
mod mcp_tab;
mod plugins_tab;
mod projects_tab;
mod retro_tab;
mod roles_tab;
mod skills_tab;
mod tasks_tab;

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

/// A harness project with task-001 (four steps) and an empty task-002.
fn project(root: &Path) {
    projects::init(root).unwrap();
    let repo = Repo::open(root).unwrap();
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
}

/// A temporary `~/.harness` and a folder for projects.
struct Env {
    home: tempfile::TempDir,
    code: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            code: tempfile::tempdir().unwrap(),
        }
    }

    /// `name` inside the code folder, as the TUI stores it.
    fn path(&self, name: &str) -> PathBuf {
        self.code.path().canonicalize().unwrap().join(name)
    }

    /// The TUI with its own folder browser, starting in the code folder.
    fn app(&self, start: &Path) -> App {
        let mut app = App::new(Some(self.home.path().to_path_buf()), start);
        app.start_dir = self.code.path().canonicalize().unwrap();
        app
    }

    fn saved(&self) -> Projects {
        Projects::load(self.home.path()).unwrap()
    }
}

/// The bottom-line message as its text and whether it reports a problem.
fn shown(app: &App) -> (String, bool) {
    let message = app.message.clone().expect("a message is shown");
    let problem = message.kind == MessageKind::Error;
    (message.text, problem)
}

fn screen(app: &mut App) -> String {
    screen_of_width(app, 120)
}

/// The screen drawn `width` columns wide: wide enough for a temporary folder
/// path, which on macOS is long (`/private/var/folders/…/T/…`).
fn screen_of_width(app: &mut App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

/// Replaces the focused field's text.
fn fill(app: &mut App, text: &str) {
    let (_, form) = app.form.as_mut().unwrap();
    form.fields[form.focus].value.clear();
    type_text(app, text);
}

/// Where `text` is on the screen (column, row).
fn find(screen: &str, text: &str) -> (u16, u16) {
    for (y, line) in screen.lines().enumerate() {
        if let Some(byte) = line.find(text) {
            let x = line[..byte].chars().count();
            return (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
        }
    }
    panic!("{text:?} not on the screen:\n{screen}");
}

fn click(app: &mut App, text: &str) {
    let screen = screen(app);
    let (x, y) = find(&screen, text);
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x + 1,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
}

/// Lets the background work finish, as the event loop does.
fn wait(app: &mut App) {
    let start = Instant::now();
    while app.tasks.as_ref().unwrap().is_running() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the run did not finish"
        );
        std::thread::sleep(Duration::from_millis(20));
        app.tick();
    }
}

/// Ctrl+S: sends the message (Enter is a new line).
fn send(app: &mut App) {
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
}

fn empty_project(env: &Env) -> (PathBuf, App) {
    let root = env.path("fresh");
    projects::init(&root).unwrap();
    let mut app = env.app(&root);
    app.builder = mock_team;
    (root, app)
}

fn config(root: &Path) -> harness_core::config::Config {
    harness_core::config::Config::load(&root.join(".harness")).unwrap()
}

fn model(id: &str, efforts: &[&str], default_effort: Option<&str>, default: bool) -> models::Model {
    models::Model {
        id: id.into(),
        name: None,
        efforts: efforts.iter().map(ToString::to_string).collect(),
        default_effort: default_effort.map(Into::into),
        default,
    }
}

/// Scripted agents: the architect finishes a design, the others approve.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a fake must match the `Builder` function type"
)]
fn mock_team(
    _: &harness_core::config::Config,
    _: &Path,
) -> Result<harness_agents::Team, harness_agents::build::BuildError> {
    use harness_agents::{AnyAgent, MockAgent, MockStep, Team};
    let agent = |role, next| {
        AnyAgent::Mock(MockAgent::new().then(role, MockStep::finish(Verdict::Approved, next)))
    };
    Ok(Team::new()
        .with(
            Role::Architect,
            agent(Role::Architect, NextStep::To(Role::Human)),
        )
        .with(
            Role::Developer,
            agent(Role::Developer, NextStep::To(Role::Tester)),
        )
        .with(
            Role::Tester,
            agent(Role::Tester, NextStep::To(Role::Security)),
        )
        .with(Role::Security, agent(Role::Security, NextStep::Done)))
}
