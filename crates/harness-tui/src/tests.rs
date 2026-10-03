use std::fs;

use super::*;
use harness_core::git::Repo;
use harness_core::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};
use harness_core::orchestrator;
use harness_core::projects::Projects;
use harness_core::store::TaskStore;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::Duration;

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

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
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

#[test]
fn starts_with_a_project_folder_and_shows_its_tasks() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    assert_eq!(app.tab, Tab::Tasks);
    assert_eq!(app.project.as_deref(), Some(root.as_path()));
    let text = screen(&mut app);
    for part in [
        " ◆ test   1 Tasks │ 2 Roles",
        "7 Projects",
        " + New project   EN ",
        "▶ task-001  ● working",
        "task-002  ● working",
        "architect  claude",
        "r1 architect  approved → human",
        "▶ r1 tester",
        "high     src/parser.rs:42  Panics on empty input",
        "· notes.md",
        "q quit",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // The project is remembered for the next start.
    let saved = env.saved();
    assert_eq!(saved.last.as_deref(), Some(root.as_path()));
    assert_eq!(saved.projects[0].name, "test");

    // Started elsewhere, the TUI opens the last project.
    let app = env.app(env.home.path());
    assert_eq!(app.project.as_deref(), Some(root.as_path()));
}

#[test]
fn without_a_project_it_starts_on_the_projects_tab() {
    let env = Env::new();
    let mut app = env.app(env.code.path());
    assert_eq!((app.tab, app.project.clone()), (Tab::Projects, None));
    let text = screen(&mut app);
    assert!(text.contains("no project"), "{text}");
    assert!(text.contains("No projects in the list yet."), "{text}");

    key(&mut app, KeyCode::Char('1'));
    assert!(screen(&mut app).contains("No project is open"));
    key(&mut app, KeyCode::Char('3'));
    assert!(screen(&mut app).contains("No project is open"));
    key(&mut app, KeyCode::Char('4'));
    assert!(screen(&mut app).contains("No project is open"));
    key(&mut app, KeyCode::Char('5'));
    assert!(screen(&mut app).contains("No project is open"));
    key(&mut app, KeyCode::Char('6'));
    assert!(screen(&mut app).contains("No project is open"));
}

#[test]
fn a_new_project_is_created_in_a_folder_chosen_in_the_browser() {
    let env = Env::new();
    fs::create_dir(env.path("work")).unwrap();
    let mut app = env.app(env.code.path());
    click(&mut app, " New project ");
    let text = screen(&mut app);
    assert!(text.contains("Folder for the new project"), "{text}");
    assert!(text.contains("work/"), "{text}");

    // Into «work», then a new folder «fresh» made there.
    click(&mut app, "work/");
    click(&mut app, "work/");
    assert!(screen(&mut app).contains("No folders inside"));
    click(&mut app, " New folder ");
    type_text(&mut app, "fresh");
    key(&mut app, KeyCode::Enter);
    let root = env.path("work/fresh");
    assert!(root.is_dir());
    click(&mut app, " Choose this folder ");

    // The form shows the folder and asks for the name.
    let text = screen(&mut app);
    assert!(text.contains("The project will be in"), "{text}");
    let (_, form) = app.form.as_ref().unwrap();
    assert_eq!(form.value(0), "fresh");
    fill(&mut app, "Fresh one");
    click(&mut app, " Create ");

    assert!(app.form.is_none());
    assert_eq!(app.project.as_deref(), Some(root.as_path()));
    // A new project opens where its agents are chosen.
    assert_eq!(app.tab, Tab::Roles);
    assert!(has_config(&root));
    assert!(root.join(".git").is_dir());
    let text = screen(&mut app);
    assert!(text.contains("Project «Fresh one» created"), "{text}");
    assert_eq!(env.saved().projects[0].name, "Fresh one");

    // The same folder again is refused. The browser starts next to it now.
    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Char('n'));
    assert!(screen(&mut app).contains("fresh/"));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('c'));
    assert!(app.form.is_none());
    let (text, error) = app.message.clone().unwrap();
    assert!(
        error && text.contains("already is a harness project"),
        "{text}"
    );
}

#[test]
fn a_plain_folder_is_prepared_before_it_opens() {
    let env = Env::new();
    let folder = env.path("plain");
    fs::create_dir(&folder).unwrap();
    let mut app = env.app(env.code.path());

    // Keys work in the browser too; Esc closes it.
    key(&mut app, KeyCode::Char('o'));
    assert!(screen(&mut app).contains("Open a project folder"));
    key(&mut app, KeyCode::Esc);
    assert!(app.browser.is_none());

    key(&mut app, KeyCode::Char('o'));
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.form, Some((Purpose::InitFolder(_), _))));
    // Cancel leaves the folder alone.
    click(&mut app, " Cancel ");
    assert!(app.form.is_none());
    assert!(!has_config(&folder));

    key(&mut app, KeyCode::Char('o'));
    click(&mut app, "plain/");
    click(&mut app, " Choose this folder ");
    key(&mut app, KeyCode::Enter);
    assert!(has_config(&folder));
    assert_eq!(app.project.as_deref(), Some(folder.as_path()));
}

#[test]
fn projects_are_chosen_with_the_mouse_and_removed_from_the_list() {
    let env = Env::new();
    let (a, b) = (env.path("alpha"), env.path("beta"));
    project(&a);
    project(&b);
    let mut list = Projects::default();
    list.add("alpha", &a);
    list.add("beta", &b);
    list.save(env.home.path()).unwrap();

    let mut app = env.app(env.code.path());
    assert_eq!(app.tab, Tab::Projects);
    click(&mut app, "  beta  ");
    assert_eq!(app.projects.selected, 1);
    assert!(screen(&mut app).contains(&b.display().to_string()));

    // A double click opens it.
    click(&mut app, "  beta  ");
    assert_eq!(app.project.as_deref(), Some(b.as_path()));
    assert_eq!(app.tab, Tab::Tasks);

    // A click on a tab switches to it; the open project is marked.
    click(&mut app, "7 Projects");
    assert_eq!(app.tab, Tab::Projects);
    assert!(screen(&mut app).contains("● beta"));

    // Removing asks first and never deletes the folder.
    click(&mut app, "Remove from list ");
    assert!(screen(&mut app).contains("leaves the list"));
    click(&mut app, " Remove   Cancel");
    assert_eq!(env.saved().projects.len(), 1);
    assert_eq!(env.saved().last, None);
    assert_eq!(app.project, None);
    assert!(b.is_dir());

    // A click outside a form closes it.
    key(&mut app, KeyCode::Char('n'));
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 29,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.form.is_none());
}

#[test]
fn the_mouse_moves_through_tasks_and_steps() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "r1 human");
    assert_eq!(app.tasks.as_ref().unwrap().step, 1);
    assert!(screen(&mut app).contains("round 1 · human"));

    // The wheel over the steps moves the selection there.
    let (x, y) = find(&screen(&mut app), "r1 human");
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tasks.as_ref().unwrap().step, 2);

    click(&mut app, "task-002");
    let tasks = app.tasks.as_ref().unwrap();
    assert_eq!((tasks.task, tasks.step), (1, 0));
    assert!(screen(&mut app).contains("Second task"));

    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn the_language_switches_and_is_remembered() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    assert!(screen(&mut app).contains("1 Tasks"));

    click(&mut app, " EN ");
    let text = screen(&mut app);
    for part in [
        "1 Задачи │ 2 Роли",
        " + Новый проект   RU ",
        "q выход",
        "раунд 1 · tester",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // The choice is kept for the next start.
    let mut again = env.app(&root);
    assert!(screen(&mut again).contains("1 Задачи"));

    // The button in the top bar starts a new project from any tab.
    click(&mut again, "+ Новый проект");
    let text = screen(&mut again);
    assert!(text.contains("Папка для нового проекта"), "{text}");
    assert!(text.contains(" Выбрать эту папку "), "{text}");
    key(&mut again, KeyCode::Esc);

    key(&mut again, KeyCode::Char('L'));
    assert!(screen(&mut again).contains("1 Tasks"));
}

fn config(root: &Path) -> harness_core::config::Config {
    harness_core::config::Config::load(&root.join(".harness")).unwrap()
}

#[test]
fn roles_get_agents_models_and_skills_and_are_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let skills = root.join(".harness/skills");
    fs::create_dir_all(&skills).unwrap();
    fs::write(
        skills.join("rust-errors.md"),
        "---\ndescription: Errors with thiserror.\n---\nUse thiserror.\n",
    )
    .unwrap();
    fs::write(skills.join("broken.md"), "no header").unwrap();
    let mut app = env.app(&root);
    click(&mut app, "2 Roles");
    let text = screen(&mut app);
    for part in [
        "architect  claude",
        "retro      claude",
        "(•) claude",
        "( ) codex+deepseek",
        "(•) agent's default",
        "No list of claude's models yet",
        "[ ] rust-errors",
        "Errors with thiserror.",
        "the file has no description",
        "No MCP servers yet",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }

    // The developer moves to codex with a model and a skill always in the prompt.
    click(&mut app, "developer  claude");
    click(&mut app, "( ) codex ");
    click(&mut app, "( ) other model…");
    fill(&mut app, "gpt-5.5");
    key(&mut app, KeyCode::Enter);
    click(&mut app, "[ ] rust-errors");
    click(&mut app, "[x] rust-errors");
    let text = screen(&mut app);
    assert!(text.contains("[■] rust-errors"), "{text}");
    assert!(text.contains("developer *codex"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    assert_eq!(
        config(&root).roles[&Role::Developer].agent,
        "claude",
        "not saved yet"
    );

    // Retro on another agent too.
    click(&mut app, "retro      claude");
    click(&mut app, "( ) antigravity");

    click(&mut app, " Save ");
    let text = screen(&mut app);
    assert!(text.contains("Settings saved and committed"), "{text}");
    let saved = config(&root);
    let developer = &saved.roles[&Role::Developer];
    assert_eq!(developer.agent, "codex");
    assert_eq!(developer.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(developer.always_skills, ["rust-errors"]);
    assert_eq!(saved.retro.unwrap().agent, "antigravity");
    let changed = Repo::open(&root).unwrap().changed_files().unwrap();
    assert!(
        !changed.iter().any(|f| f.ends_with("harness.toml")),
        "committed: {changed:?}"
    );
    // The Tasks tab shows the new agent.
    key(&mut app, KeyCode::Char('1'));
    assert!(screen(&mut app).contains("developer  codex (gpt-5.5)"));
}

#[test]
fn settings_that_fail_the_checks_are_not_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let skills = root.join(".harness/skills");
    fs::create_dir_all(&skills).unwrap();
    fs::write(skills.join("broken.md"), "no header").unwrap();
    let before = fs::read_to_string(root.join(".harness/harness.toml")).unwrap();
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('2'));
    click(&mut app, "[ ] broken");
    click(&mut app, " Save ");
    let (text, error) = app.message.clone().unwrap();
    assert!(error, "{text}");
    assert_eq!(
        fs::read_to_string(root.join(".harness/harness.toml")).unwrap(),
        before
    );

    // Quitting asks once while changes are not saved.
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(screen(&mut app).contains("press q again"));
    // With the Russian layout «й» is q.
    key(&mut app, KeyCode::Char('й'));
    assert!(app.quit);

    // Undo brings back what is saved.
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('2'));
    click(&mut app, "[ ] broken");
    click(&mut app, " Undo changes ");
    assert!(screen(&mut app).contains("[ ] broken"));
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

/// Scripted agents: the architect finishes a design, the others approve.
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

fn no_login(
    _: &harness_core::config::Config,
    _: &Path,
) -> Result<harness_agents::Team, harness_agents::build::BuildError> {
    Err(harness_agents::build::BuildError(
        "no Claude token saved; run `harness login claude` first".into(),
    ))
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

fn stage(app: &App) -> harness_core::task::Stage {
    let tasks = app.tasks.as_ref().unwrap();
    tasks.tasks_state(tasks.task).unwrap()
}

#[test]
fn a_new_task_runs_the_architect_then_the_answer_goes_to_the_developer() {
    use harness_core::task::{Stage, WaitReason};
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let text = screen(&mut app);
    assert!(
        text.contains("Write what to do in the line below"),
        "{text}"
    );
    assert!(
        text.contains(" To: architect · new task ▾   Model: default ▾ "),
        "{text}"
    );

    // Sending nothing is refused.
    click(&mut app, " Send ");
    assert_eq!(app.message.as_ref().unwrap().0, "Write the task first");

    click(&mut app, "Write here");
    assert!(app.tasks.as_ref().unwrap().typing());
    // Typing keys are text, not hot keys: q does not quit, 2 stays.
    type_text(&mut app, "Build a CSV parser, q 2");
    assert!(!app.quit);
    assert_eq!(app.tab, Tab::Tasks);
    assert!(screen(&mut app).contains("Build a CSV parser, q 2▏"));
    // Enter starts a new line; Ctrl+S sends.
    // Leaving the box and pressing Esc does not lose the text.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(!app.quit);
    assert!(app.message.as_ref().unwrap().0.contains("not sent"));
    click(&mut app, "Build a CSV");
    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "Second line");
    assert!(!app.tasks.as_ref().unwrap().is_running());
    let text = screen(&mut app);
    assert!(text.contains("│ Build a CSV parser, q 2"), "{text}");
    assert!(text.contains("│ Second line▏"), "{text}");
    send(&mut app);
    assert!(app.tasks.as_ref().unwrap().is_running());
    assert!(screen(&mut app).contains(" Working… "));
    wait(&mut app);

    let store_dir = root.join(".harness/runs/task-001");
    assert_eq!(
        fs::read_to_string(store_dir.join("task.md")).unwrap(),
        "Build a CSV parser, q 2\nSecond line"
    );
    assert_eq!(
        stage(&app),
        Stage::WaitingForHuman(WaitReason::ApproveDesign)
    );
    let (message, problem) = app.message.clone().unwrap();
    assert!(message.contains("the design is ready"), "{message}");
    assert!(!problem);
    // The answer goes to the developer unless Lisa chooses otherwise.
    let text = screen(&mut app);
    assert!(text.contains(" To: developer ▾ "), "{text}");
    assert!(text.contains(" Agent log "), "{text}");

    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "Use serde");
    send(&mut app);
    wait(&mut app);
    assert_eq!(stage(&app), Stage::Done);
    let steps = TaskStore::open(&root.join(".harness/runs"), "task-001")
        .unwrap()
        .0
        .steps()
        .unwrap();
    let who: Vec<Role> = steps.iter().map(|s| s.handoff.role).collect();
    assert_eq!(
        who,
        [
            Role::Architect,
            Role::Human,
            Role::Developer,
            Role::Tester,
            Role::Security
        ]
    );
    assert_eq!(steps[1].handoff.summary, "Use serde");
    assert_eq!(steps[1].handoff.verdict, Verdict::Approved);
    assert!(app.message.as_ref().unwrap().0.contains("done"));
    // A finished task offers only a new one.
    assert!(screen(&mut app).contains(" To: architect · new task ▾ "));
}

#[test]
fn the_to_list_offers_the_roles_and_finishing() {
    use harness_core::task::Stage;
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    // Before there is a task, only a new one can be sent: the roles are
    // listed, but grey.
    click(&mut app, " To: architect · new task ▾ ");
    let text = screen(&mut app);
    assert!(text.contains("Approve and finish"), "{text}");
    // The list is drawn over the tab: «security» in it, not in «Roles».
    click(&mut app, "│  security");
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("waits for your answer"));
    assert_eq!(app.tasks.as_ref().unwrap().choice, tasks::Choice::NewTask);

    app.tasks.as_mut().unwrap().paste("Line one\nLine two");
    let text = screen(&mut app);
    assert!(text.contains("│ Line one"), "{text}");
    assert!(text.contains("│ Line two▏"), "{text}");
    send(&mut app);
    wait(&mut app);

    click(&mut app, " To: developer ▾ ");
    let text = screen(&mut app);
    for option in ["Approve and finish", "architect · new task", "security"] {
        assert!(text.contains(option), "missing {option:?} in:\n{text}");
    }
    click(&mut app, "Approve and finish");
    assert!(app.tasks.as_ref().unwrap().menu.is_none());
    assert!(screen(&mut app).contains(" To: Approve and finish ▾ "));

    // ↑↓ in the field change whom it goes to.
    click(&mut app, "Write here");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    type_text(&mut app, "Good enough");
    send(&mut app);
    wait(&mut app);
    assert_eq!(stage(&app), Stage::Done);
    assert_eq!(app.message.as_ref().unwrap().0, "task-001: saved");
    let steps = TaskStore::open(&root.join(".harness/runs"), "task-001")
        .unwrap()
        .0
        .steps()
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].handoff.next_role, NextStep::Done);
}

#[test]
fn a_missing_login_stops_before_anything_is_saved() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    app.builder = no_login;
    click(&mut app, "Write here");
    type_text(&mut app, "Something");
    send(&mut app);
    wait(&mut app);
    let (message, problem) = app.message.clone().unwrap();
    assert!(message.contains("harness login claude"), "{message}");
    assert!(problem);
    assert!(!root.join(".harness/runs/task-001").exists());
    // The error is in the log too, where it is not cut off.
    assert!(screen(&mut app).contains("── no Claude token saved"));
}

#[test]
fn quitting_waits_for_the_running_roles() {
    let env = Env::new();
    let (_, mut app) = empty_project(&env);
    click(&mut app, "Write here");
    type_text(&mut app, "Something");
    send(&mut app);
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(app.message.as_ref().unwrap().1);
    wait(&mut app);
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn a_click_on_a_role_shows_only_the_tasks_waiting_on_it() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    // task-001 is back with the developer, task-002 with the architect.
    click(&mut app, "developer  claude");
    let text = screen(&mut app);
    assert!(text.contains(" Tasks of developer "), "{text}");
    assert!(text.contains("▶ task-001"), "{text}");
    assert!(!text.contains("task-002"), "{text}");

    click(&mut app, "architect  claude");
    let text = screen(&mut app);
    assert!(text.contains("▶ task-002"), "{text}");
    assert!(!text.contains("task-001"), "{text}");
    assert!(text.contains("Second task"), "{text}");

    click(&mut app, "tester     claude");
    let text = screen(&mut app);
    assert!(text.contains("No task waits on the tester now."), "{text}");

    // A second click on the same role shows all tasks again.
    click(&mut app, "tester     claude");
    let text = screen(&mut app);
    assert!(
        text.contains("task-001") && text.contains("task-002"),
        "{text}"
    );
    assert_eq!(app.tasks.as_ref().unwrap().filter, None);
}

/// The editor was "closed": what `event_loop` does after the editor returns.
fn close_editor(app: &mut App) -> EditJob {
    let job = app.edit.take().expect("a skill to edit");
    app.finish_edit(&job, Ok(()));
    job
}

fn last_commit(root: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_skills_tab_shows_each_roles_base_and_the_skills_to_choose() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "3 Skills");
    let text = screen(&mut app);
    for part in [
        " architect ",
        "Skills of the architect",
        "Always in the prompt",
        "common",
        "● agent-claude",
        "○ agent-codex",
        "○ agent-antigravity",
        "Optional · click [ ] → [x] → [■]",
        "[ ] crash-recovery",
        "built-in",
        "common · built-in",
        " Edit in Zed ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Another role shows its own skill; the selected skill stays.
    click(&mut app, " tester ");
    let text = screen(&mut app);
    assert!(text.contains("Skills of the tester"), "{text}");
    assert!(text.contains("│  tester "), "{text}");
    assert!(!text.contains("│  architect "), "{text}");
    assert!(text.contains("▶ common"), "{text}");
    // Paragraphs are joined and wrapped at the panel's width.
    assert!(
        text.contains("redesign what the design already decided"),
        "{text}"
    );
    // The note of an agent no role uses can be read and edited too.
    click(&mut app, "○ agent-codex");
    let text = screen(&mut app);
    assert!(text.contains("agent-codex · built-in"), "{text}");
    assert!(text.contains("Not in the tester's prompt"), "{text}");
    // A click on a skill shows its text.
    click(&mut app, "protocol-attacks");
    assert!(screen(&mut app).contains("protocol-attacks · built-in"));
}

#[test]
fn a_click_on_the_mark_of_a_skill_chooses_it_for_the_role() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "3 Skills");
    click(&mut app, " developer ");

    // Not used -> read when needed -> always in the prompt.
    click(&mut app, "[ ] crash-recovery");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] crash-recovery"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    click(&mut app, "[x] crash-recovery");
    assert!(screen(&mut app).contains("[■] crash-recovery"));

    // The Roles tab shows the same change, and «Save» keeps it.
    click(&mut app, " Save ");
    let developer = &config(&root).roles[&Role::Developer];
    assert!(developer
        .always_skills
        .contains(&"crash-recovery".to_string()));
    assert!(!developer.skills.contains(&"crash-recovery".to_string()));

    // Space goes on round the marks; «Undo changes» brings back the saved one.
    key(&mut app, KeyCode::Char(' '));
    assert!(screen(&mut app).contains("▶ [ ] crash-recovery"));
    key(&mut app, KeyCode::Char('u'));
    assert!(screen(&mut app).contains("▶ [■] crash-recovery"));

    // A click on the name only selects the skill.
    click(&mut app, "protocol-attacks");
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] protocol-attacks"), "{text}");
    assert!(text.contains("[■] crash-recovery"), "{text}");
}

#[test]
fn a_built_in_skill_is_copied_edited_committed_and_restored() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let file = root.join(".harness/skills/developer.md");
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('3'));
    click(&mut app, " developer ");
    click(&mut app, "│  developer ");

    // Nothing changed in the editor: the copy goes away again.
    key(&mut app, KeyCode::Char('e'));
    assert!(file.is_file());
    assert!(fs::read_to_string(&file).unwrap().contains("builtin: "));
    close_editor(&mut app);
    assert!(!file.exists());
    assert!(screen(&mut app).contains("Nothing changed"));

    // A change is committed and the skill shows as changed.
    key(&mut app, KeyCode::Char('e'));
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}\nAlso run cargo fmt.\n")).unwrap();
    close_editor(&mut app);
    let text = screen(&mut app);
    assert!(
        text.contains("Skill developer saved and committed"),
        "{text}"
    );
    assert!(text.contains("developer · changed"), "{text}");
    assert_eq!(last_commit(&root), "harness: skill developer");

    // The developer's prompt gets the changed text.
    let skills = harness_core::skills::Skills::load(&root.join(".harness"), &config(&root))
        .unwrap()
        .for_role(Role::Developer);
    assert!(skills
        .base
        .iter()
        .any(|s| s.body.contains("Also run cargo fmt.")));

    // Restore asks first, then deletes the copy.
    click(&mut app, " Restore built-in ");
    let text = screen(&mut app);
    assert!(
        text.contains("Delete this project's copy of developer?"),
        "{text}"
    );
    key(&mut app, KeyCode::Enter);
    assert!(!file.exists());
    let text = screen(&mut app);
    assert!(text.contains("Skill developer is built-in again"), "{text}");
    assert!(text.contains("developer · built-in"), "{text}");
    assert_eq!(
        last_commit(&root),
        "harness: skill developer is built-in again"
    );
}

#[test]
fn a_new_skill_gets_a_file_and_can_be_chosen_on_roles() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('3'));
    click(&mut app, " New skill ");
    fill(&mut app, "Bad Name");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "Errors with thiserror.");
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("is not allowed"));
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "crash-recovery");
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("There is already a skill named crash-recovery"));
    fill(&mut app, "rust-errors");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none());

    let file = root.join(".harness/skills/rust-errors.md");
    let job = close_editor(&mut app);
    assert_eq!(job.path, file);
    assert!(fs::read_to_string(&file)
        .unwrap()
        .starts_with("---\ndescription: Errors with thiserror.\n---\n"));
    assert_eq!(last_commit(&root), "harness: skill rust-errors");
    let text = screen(&mut app);
    assert!(text.contains("[ ] rust-errors"), "{text}");
    assert!(text.contains("rust-errors · own"), "{text}");
    // An own skill has nothing to restore.
    key(&mut app, KeyCode::Delete);
    assert!(app.form.is_none());

    click(&mut app, "2 Roles");
    click(&mut app, "developer  claude");
    assert!(screen(&mut app).contains("[ ] rust-errors"));
}

fn model(id: &str, efforts: &[&str], default_effort: Option<&str>, default: bool) -> models::Model {
    models::Model {
        id: id.into(),
        name: None,
        efforts: efforts.iter().map(|e| e.to_string()).collect(),
        default_effort: default_effort.map(Into::into),
        default,
    }
}

fn codex_models() -> Vec<(String, Result<ModelList, String>)> {
    vec![
        (
            "codex".into(),
            Ok(ModelList {
                agent: "codex".into(),
                fetched: 1,
                models: vec![
                    model("gpt-6.1-sol", &["low", "medium", "high"], Some("low"), true),
                    model("gpt-5.5", &["medium", "xhigh"], Some("medium"), false),
                ],
            }),
        ),
        (
            "antigravity".into(),
            Err("agy did not answer in 90 s".into()),
        ),
    ]
}

#[test]
fn models_and_levels_come_from_the_agents_lists() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    app.asker = |_| codex_models();
    click(&mut app, "2 Roles");
    click(&mut app, " Refresh models ");
    for _ in 0..100 {
        app.tick();
        if app.asking.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = screen(&mut app);
    assert!(text.contains("Models updated: codex 2"), "{text}");
    assert!(text.contains("antigravity: agy did not answer"), "{text}");
    assert!(env.home.path().join("models/codex.json").is_file());

    // Moving to codex takes its default model at its default level.
    click(&mut app, "developer  claude");
    click(&mut app, "( ) codex ");
    let text = screen(&mut app);
    assert!(text.contains("(•) gpt-6.1-sol"), "{text}");
    assert!(text.contains("(agent's default)"), "{text}");
    assert!(text.contains("[low]"), "{text}");
    // Another model keeps a level it takes, otherwise gets its own default.
    click(&mut app, "( ) gpt-5.5");
    assert!(screen(&mut app).contains("[medium]"));
    // A click on the level moves to the next one, then back to the default.
    click(&mut app, "Effort");
    assert!(screen(&mut app).contains("[xhigh]"));
    click(&mut app, "Effort");
    assert!(screen(&mut app).contains("[agent's default]"));
    click(&mut app, "Effort");
    click(&mut app, " Save ");
    let developer = &config(&root).roles[&Role::Developer];
    assert_eq!(developer.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(developer.effort.as_deref(), Some("medium"));

    // The tester on codex too: it gets the developer's choice.
    click(&mut app, "tester     claude");
    click(&mut app, "( ) codex ");
    assert!(screen(&mut app).contains("(•) gpt-5.5"));
    // Back to claude, which has no list: the agent's default, no level.
    click(&mut app, "( ) claude ");
    let text = screen(&mut app);
    assert!(text.contains("(•) agent's default"), "{text}");
    assert!(!text.contains("Effort"), "{text}");
}

/// The architect's model and level each time the recording builder ran.
static ARCHITECT_RUNS: std::sync::Mutex<Vec<(Option<String>, Option<String>)>> =
    std::sync::Mutex::new(Vec::new());

fn recording_team(
    config: &harness_core::config::Config,
    root: &Path,
) -> Result<harness_agents::Team, harness_agents::build::BuildError> {
    let architect = &config.roles[&Role::Architect];
    ARCHITECT_RUNS
        .lock()
        .unwrap()
        .push((architect.model.clone(), architect.effort.clone()));
    mock_team(config, root)
}

#[test]
fn a_model_and_level_can_be_chosen_for_one_run() {
    let env = Env::new();
    harness_core::models::save(
        env.home.path(),
        &ModelList {
            agent: "claude".into(),
            fetched: 1,
            models: vec![
                model("sonnet", &["low", "high"], Some("high"), true),
                model("opus", &["low", "high", "max"], Some("high"), false),
                model("haiku", &[], None, false),
            ],
        },
    )
    .unwrap();
    let (root, mut app) = empty_project(&env);
    app.builder = recording_team;
    // The architect starts a new task, with what harness.toml says.
    let text = screen(&mut app);
    assert!(text.contains(" Model: default ▾ "), "{text}");
    assert!(text.contains(" Level: default ▾ "), "{text}");

    click(&mut app, " Model: default ▾ ");
    let text = screen(&mut app);
    assert!(text.contains("│  haiku"), "{text}");
    click(&mut app, "│  opus");
    click(&mut app, " Level: default ▾ ");
    click(&mut app, "│  max");
    let text = screen(&mut app);
    assert!(text.contains(" Model: opus ▾ "), "{text}");
    assert!(text.contains(" Level: max ▾ "), "{text}");
    // A model without that level gets its own default; haiku takes none.
    click(&mut app, " Model: opus ▾ ");
    click(&mut app, "│  sonnet");
    assert!(screen(&mut app).contains(" Level: high ▾ "));
    click(&mut app, " Model: sonnet ▾ ");
    click(&mut app, "│  opus");

    app.tasks.as_mut().unwrap().paste("Build a parser");
    send(&mut app);
    wait(&mut app);
    assert_eq!(
        ARCHITECT_RUNS.lock().unwrap().last().unwrap(),
        &(Some("opus".into()), Some("high".into()))
    );
    let text = screen(&mut app);
    assert!(
        text.contains("architect runs on claude · model opus · level high"),
        "{text}"
    );
    // Only for that run: harness.toml is as it was, and the developer,
    // who gets the answer now, has its own settings.
    let architect = &config(&root).roles[&Role::Architect];
    assert_eq!((&architect.model, &architect.effort), (&None, &None));
    assert!(text.contains(" Model: default ▾ "), "{text}");

    // A choice made for one role is dropped when the message goes to another.
    click(&mut app, " Model: default ▾ ");
    click(&mut app, "│  haiku");
    let text = screen(&mut app);
    assert!(text.contains(" Model: haiku ▾ "), "{text}");
    assert!(text.contains(" Level: default ▾ "), "{text}");
    click(&mut app, " To: developer ▾ ");
    click(&mut app, "│  security");
    assert!(screen(&mut app).contains(" Model: default ▾ "));
    // Finishing runs no role: no model to choose.
    click(&mut app, " To: security ▾ ");
    click(&mut app, "Approve and finish");
    assert!(!screen(&mut app).contains("[ Model:"));
}

#[test]
fn the_mcp_tab_gives_servers_to_roles() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap();
    let text =
        text.replace(
            "\n[roles.developer]\n",
            "\n[roles.developer]\nmcp = [\"context7\"]\n",
        )
        .replace(
            "\n[roles.tester]\n",
            "\n[roles.tester]\nmcp = [\"ghost\"]\n",
        ) + "\n[mcp.context7]\ncommand = \"npx\"\nargs = [\"-y\", \"@upstash/context7-mcp\"]\n\
           env = { CONTEXT7_API_KEY = \"secret:context7\" }\n\n\
           [mcp.fetch]\ncommand = \"uvx\"\nargs = [\"mcp-server-fetch\"]\n";
    fs::write(&path, text).unwrap();
    let mut app = env.app(&root);
    click(&mut app, "4 MCP");
    let text = screen(&mut app);
    for part in [
        " MCP servers of the architect ",
        "▶ [ ] context7",
        "✗ secret",
        "[ ] fetch",
        "[ ] ghost",
        "not described",
        "Command: npx -y @upstash/context7-mcp",
        "CONTEXT7_API_KEY = secret:context7  ✗ not saved: harness secret set",
        "claude ✓",
        "antigravity ✓",
        "Roles: developer",
        " Give to the architect ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }

    // A saved secret shows up; its value never does.
    harness_agents::credentials::save_secret(
        &env.home.path().join("credentials"),
        "context7",
        &harness_agents::credentials::Secret::new("ctx-secret-value"),
    )
    .unwrap();
    app.mcp.as_mut().unwrap().reload();
    let text = screen(&mut app);
    assert!(text.contains("secret:context7  ✓ saved"), "{text}");
    assert!(!text.contains("ctx-secret-value"), "{text}");
    assert!(!text.contains("✗ secret"), "{text}");

    // The developer has context7; fetch is given with a double click.
    click(&mut app, " developer ");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] context7"), "{text}");
    assert!(text.contains(" Take from the developer "), "{text}");
    click(&mut app, "[ ] fetch");
    click(&mut app, "[ ] fetch");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] fetch"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    // The Roles tab shows the same change.
    click(&mut app, "2 Roles");
    click(&mut app, "developer");
    assert!(screen(&mut app).contains("[x] fetch"));
    click(&mut app, "4 MCP");
    // Saving checks everything as before a run: the tester's unknown server
    // stops it.
    click(&mut app, " Save ");
    let (message, problem) = app.message.clone().unwrap();
    assert!(problem && message.contains("ghost"), "{message}");

    // A server harness.toml does not describe cannot be given ...
    click(&mut app, " architect ");
    click(&mut app, "[ ] ghost");
    let text = screen(&mut app);
    assert!(text.contains("harness.toml has no [mcp.ghost]"), "{text}");
    key(&mut app, KeyCode::Char(' '));
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("take it from the role"));
    // ... only taken away; then no role lists it and it leaves the list.
    click(&mut app, " tester ");
    click(&mut app, "[x] ghost");
    key(&mut app, KeyCode::Char(' '));
    assert!(!screen(&mut app).contains("ghost"));
    key(&mut app, KeyCode::Char('s'));
    let saved = config(&root);
    assert_eq!(saved.roles[&Role::Developer].mcp, ["context7", "fetch"]);
    assert!(saved.roles[&Role::Tester].mcp.is_empty());
}

#[test]
fn mcp_servers_are_added_changed_and_removed_and_secrets_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "4 MCP");
    assert!(screen(&mut app).contains("No MCP servers yet."));

    click(&mut app, " New server ");
    fill(&mut app, "docs");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "npx -y docs-mcp");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "API_KEY=secret:docs");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    let docs = &config(&root).mcp["docs"];
    assert_eq!(
        (docs.command.as_str(), &docs.args[..]),
        ("npx", &["-y".to_string(), "docs-mcp".to_string()][..])
    );
    assert_eq!(docs.env["API_KEY"], "secret:docs");
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] docs"), "{text}");
    assert!(text.contains("MCP server docs saved"), "{text}");

    // A wrong variable name is refused in the form, nothing is written.
    key(&mut app, KeyCode::Char('n'));
    fill(&mut app, "bad");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "npx bad");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "api_key=1");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.as_ref().unwrap().1.error.is_some());
    key(&mut app, KeyCode::Esc);
    assert!(!config(&root).mcp.contains_key("bad"));

    // The developer gets it; a rename follows in its list.
    click(&mut app, " developer ");
    key(&mut app, KeyCode::Char(' '));
    // Servers change only when the roles have no unsaved changes.
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    assert!(app
        .form
        .as_ref()
        .unwrap()
        .1
        .error
        .as_ref()
        .unwrap()
        .contains("Save or undo"));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.form.as_ref().unwrap().1.value(1), "npx -y docs-mcp");
    fill(&mut app, "manuals");
    key(&mut app, KeyCode::Enter);
    let saved = config(&root);
    assert!(!saved.mcp.contains_key("docs"));
    assert_eq!(saved.roles[&Role::Developer].mcp, ["manuals"]);
    assert!(screen(&mut app).contains("▶ [x] manuals"));

    // The secret: typed in a hidden field, saved in a private file only.
    assert!(screen(&mut app).contains("✗ secret"));
    click(&mut app, " Set secret ");
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "docs");
    type_text(&mut app, "s3cr3t-value");
    let text = screen(&mut app);
    assert!(text.contains("••••••••••••"), "{text}");
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(!format!("{:?}", app.form).contains("s3cr3t"));
    key(&mut app, KeyCode::Enter);
    let file = env.home.path().join("credentials/secrets/docs");
    assert_eq!(fs::read_to_string(&file).unwrap().trim(), "s3cr3t-value");
    let text = screen(&mut app);
    assert!(text.contains("secret:docs  ✓ saved"), "{text}");
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(!fs::read_to_string(root.join(".harness/harness.toml"))
        .unwrap()
        .contains("s3cr3t"));

    // Removing asks first, then takes it from the roles too.
    key(&mut app, KeyCode::Delete);
    assert!(screen(&mut app).contains("Remove [mcp.manuals]"));
    key(&mut app, KeyCode::Enter);
    let saved = config(&root);
    assert!(saved.mcp.is_empty());
    assert!(saved.roles[&Role::Developer].mcp.is_empty());
    assert!(file.is_file());
}

fn fake_check(
    server: &harness_core::mcp::McpServer,
    _: &Path,
) -> Result<Vec<harness_core::mcp_tools::Tool>, String> {
    // The server gets its secret, as a run would give it.
    if server.env["API_KEY"].expose() != "docs-key" {
        return Err("wrong key".into());
    }
    Ok(vec![
        harness_core::mcp_tools::Tool {
            name: "search".into(),
            description: Some("Searches the docs.".into()),
        },
        harness_core::mcp_tools::Tool {
            name: "fetch".into(),
            description: None,
        },
    ])
}

#[test]
fn check_asks_a_server_for_its_tools() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.docs]\ncommand = \"npx\"\nargs = [\"docs-mcp\"]\nenv = { API_KEY = \"secret:docs\" }\n\n\
           [mcp.other]\ncommand = \"npx\"\nenv = { KEY = \"secret:other\" }\n";
    fs::write(&path, text).unwrap();
    harness_agents::credentials::save_secret(
        &env.home.path().join("credentials"),
        "docs",
        &harness_agents::credentials::Secret::new("docs-key"),
    )
    .unwrap();
    let mut app = env.app(&root);
    app.checker = fake_check;
    click(&mut app, "4 MCP");
    assert!(screen(&mut app).contains("Tools: not checked yet."));

    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("docs: 2 tools"), "{text}");
    assert!(text.contains("Tools (2):"), "{text}");
    assert!(text.contains("search  Searches the docs."), "{text}");
    assert!(!text.contains("docs-key"), "{text}");

    // A server whose secret is not saved is not started.
    click(&mut app, "[ ] other");
    click(&mut app, " Check ");
    assert!(app.checking.is_none());
    let (message, problem) = app.message.clone().unwrap();
    assert!(
        problem && message.contains("harness secret set other"),
        "{message}"
    );
}

fn fake_search(query: &str) -> Result<Vec<harness_core::mcp_registry::Entry>, String> {
    if query != "docs" {
        return Err("offline".into());
    }
    harness_core::mcp_registry::parse(
        r#"{"servers":[
        {"server":{"name":"io.github.someone/docs","title":"Docs","description":"Finds docs.",
          "version":"1.2.0","repository":{"url":"https://github.com/someone/docs"},
          "packages":[{"registryType":"npm","identifier":"docs-mcp","version":"1.2.0",
            "transport":{"type":"stdio"},
            "environmentVariables":[{"name":"DOCS_KEY","description":"Your key.","isSecret":true}]}]}},
        {"server":{"name":"com.example/remote-docs","version":"0.1.0",
          "remotes":[{"type":"sse","url":"https://example.com/sse"}]}},
        {"server":{"name":"com.example/web-docs","version":"0.2.0",
          "remotes":[{"type":"streamable-http","url":"https://example.com/web",
            "headers":[{"name":"Authorization","value":"Bearer {key}","isSecret":true}]}]}}
        ]}"#,
    )
}

/// Waits until the registry search running in the background has answered.
fn wait_search(app: &mut App) {
    let start = Instant::now();
    while app.searching.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
}

#[test]
fn a_server_from_the_catalog_opens_in_the_form_before_it_is_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    app.searcher = fake_search;
    click(&mut app, "4 MCP");

    // A failed search says why.
    click(&mut app, " From catalog ");
    fill(&mut app, "anything");
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    assert!(screen(&mut app).contains("the search failed: offline"));

    key(&mut app, KeyCode::Char('/'));
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "anything");
    fill(&mut app, "docs");
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    let text = screen(&mut app);
    assert!(text.contains("3 servers for «docs»"), "{text}");
    assert!(text.contains("Finds docs."), "{text}");
    assert!(text.contains("npx -y docs-mcp@1.2.0"), "{text}");
    assert!(text.contains("DOCS_KEY = secret:docs"), "{text}");
    assert!(text.contains("cannot add"), "{text}");

    // A server with only an old SSE address cannot be added.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none());
    let (message, problem) = app.message.clone().unwrap();
    assert!(problem && message.contains("Cannot be added"), "{message}");

    // The first opens filled in; nothing is written before Save.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    let form = &app.form.as_ref().unwrap().1;
    assert_eq!(
        (form.value(0), form.value(1), form.value(2)),
        ("docs", "npx -y docs-mcp@1.2.0", "DOCS_KEY=secret:docs")
    );
    assert!(config(&root).mcp.is_empty());
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    assert_eq!(config(&root).mcp["docs"].args, ["-y", "docs-mcp@1.2.0"]);
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] docs"), "{text}");
    assert!(text.contains("✗ secret"), "{text}");

    // Added again, it gets a name of its own; Esc goes back to the list.
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    assert!(screen(&mut app).contains("already added"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "docs-2");
    key(&mut app, KeyCode::Esc);

    // A server on the web: its address and headers, the key as a secret.
    key(&mut app, KeyCode::Down);
    let text = screen(&mut app);
    assert!(text.contains("Address: https://example.com/web"), "{text}");
    assert!(
        text.contains("Authorization = Bearer secret:web-docs"),
        "{text}"
    );
    key(&mut app, KeyCode::Enter);
    let form = &app.form.as_ref().unwrap().1;
    assert_eq!(
        (form.value(0), form.value(1), form.value(2), form.value(3)),
        (
            "web-docs",
            "https://example.com/web",
            "\"Authorization=Bearer secret:web-docs\"",
            ""
        )
    );
    key(&mut app, KeyCode::Enter);
    let web = &config(&root).mcp["web-docs"];
    assert_eq!(web.url.as_deref(), Some("https://example.com/web"));
    assert_eq!(web.headers["Authorization"], "Bearer secret:web-docs");
    assert!(!app.quit);
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] web-docs"), "{text}");
    assert!(text.contains("Address: https://example.com/web"), "{text}");
}

#[test]
fn a_web_server_is_written_and_changed_in_the_form() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "4 MCP");
    click(&mut app, " New server ");
    fill(&mut app, "notion");
    key(&mut app, KeyCode::Tab);
    // The fields of a new server start empty: typing alone is enough.
    assert_eq!(app.form.as_ref().unwrap().1.value(1), "");
    type_text(&mut app, "https://mcp.notion.com/mcp");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "да");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    let notion = &config(&root).mcp["notion"];
    assert_eq!(notion.url.as_deref(), Some("https://mcp.notion.com/mcp"));
    assert_eq!(notion.auth.as_deref(), Some("oauth"));
    assert!(screen(&mut app).contains(" Sign in "));

    // Changed back to a key in a header, in the same form.
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.form.as_ref().unwrap().1.value(3), "yes");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "Authorization=\"Bearer secret:notion\"");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "");
    key(&mut app, KeyCode::Enter);
    let notion = &config(&root).mcp["notion"];
    assert!(notion.auth.is_none());
    assert_eq!(notion.headers["Authorization"], "Bearer secret:notion");
    let text = fs::read_to_string(root.join(".harness/harness.toml")).unwrap();
    assert!(!text.contains("auth"), "{text}");
}

fn fake_web_check(
    server: &harness_core::mcp::McpServer,
    _: &Path,
) -> Result<Vec<harness_core::mcp_tools::Tool>, String> {
    // The web server is reached through this program, with its header.
    assert_eq!(server.args, ["mcp-remote"]);
    assert_eq!(Path::new(&server.command), std::env::current_exe().unwrap());
    assert_eq!(
        server.env["HARNESS_MCP_HEADER_1"].expose(),
        "Authorization: Bearer web-key"
    );
    Ok(vec![harness_core::mcp_tools::Tool {
        name: "ask".into(),
        description: None,
    }])
}

#[test]
fn a_web_server_shows_its_address_and_is_checked_through_the_bridge() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.wiki]\nurl = \"https://mcp.example.com/mcp\"\n\
           headers = { Authorization = \"Bearer secret:wiki\" }\n";
    fs::write(&path, text).unwrap();
    let mut app = env.app(&root);
    app.checker = fake_web_check;
    click(&mut app, "4 MCP");
    let text = screen(&mut app);
    assert!(
        text.contains("Address: https://mcp.example.com/mcp"),
        "{text}"
    );
    assert!(text.contains("Authorization: Bearer secret:wiki"), "{text}");
    assert!(text.contains("✗ secret"), "{text}");
    assert!(!text.contains("Command:"), "{text}");

    harness_agents::credentials::save_secret(
        &env.home.path().join("credentials"),
        "wiki",
        &harness_agents::credentials::Secret::new("web-key"),
    )
    .unwrap();
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("wiki: 1 tools"), "{text}");
    assert!(!text.contains("web-key"), "{text}");
}

fn fake_sign_in(dir: &Path, name: &str, url: &str) -> Result<(), String> {
    // As a real sign-in would leave it.
    let file = harness_agents::mcp_oauth::path(dir, name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let saved = serde_json::json!({
        "url": url, "token_endpoint": "https://auth.example.com/token",
        "client_id": "c", "access_token": "oauth-token-1",
    });
    fs::write(&file, saved.to_string()).unwrap();
    Ok(())
}

#[test]
fn a_web_server_with_a_sign_in_is_signed_in_from_the_tab() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.notion]\nurl = \"https://mcp.example.com/mcp\"\nauth = \"oauth\"\n";
    fs::write(&path, text).unwrap();
    let mut app = env.app(&root);
    app.signer = fake_sign_in;
    app.checker = |server, _| {
        assert_eq!(
            server.env["HARNESS_MCP_HEADER_1"].expose(),
            "Authorization: Bearer oauth-token-1"
        );
        Ok(Vec::new())
    };
    click(&mut app, "4 MCP");
    let text = screen(&mut app);
    assert!(text.contains("✗ sign-in"), "{text}");
    assert!(text.contains("Sign-in: ✗ not signed in"), "{text}");
    // Without a sign-in, «Check» says what to do.
    key(&mut app, KeyCode::Char('c'));
    let (message, problem) = app.message.clone().unwrap();
    assert!(
        problem && message.contains("harness mcp login notion"),
        "{message}"
    );

    key(&mut app, KeyCode::Char('i'));
    let start = Instant::now();
    while app.signing.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("Signed in to notion"), "{text}");
    assert!(text.contains("Sign-in: ✓ signed in"), "{text}");
    assert!(!text.contains("oauth-token-1"), "{text}");

    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    assert!(screen(&mut app).contains("notion: 0 tools"));
}

/// A plugin folder for `agent` in the project, with extra files.
fn plugin_folder(root: &Path, name: &str, agent: &str, extra: &[(&str, &str)]) {
    let folder = root.join(".harness/plugins").join(name);
    let manifest = folder.join(harness_core::plugins::manifest(agent));
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        manifest,
        format!(r#"{{"name": "{name}", "description": "The {name} plugin", "version": "1.0.0"}}"#),
    )
    .unwrap();
    for (file, text) in extra {
        let path = folder.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

#[test]
fn the_plugins_tab_gives_allows_and_removes_plugins() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    plugin_folder(
        &root,
        "review",
        "claude",
        &[
            ("skills/audit/SKILL.md", "---\n---\n"),
            ("commands/review.md", "# review"),
            ("hooks/hooks.json", "{}"),
        ],
    );
    plugin_folder(&root, "lint", "codex", &[]);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[plugins.review]\nagent = \"claude\"\nsource = \"official/review\"\n\
           commit = \"0123456789abcdef\"\n\n[plugins.lint]\nagent = \"codex\"\n";
    let text = text
        .replace(
            "[roles.developer]\nagent = \"claude\"",
            "[roles.developer]\nagent = \"codex\"",
        )
        .replace(
            "[roles.security]\nagent = \"claude\"",
            "[roles.security]\nagent = \"antigravity\"",
        );
    fs::write(&path, text).unwrap();
    let repo = Repo::open(&root).unwrap();
    repo.commit_all("plugins").unwrap();

    let mut app = env.app(&root);
    click(&mut app, "5 Plugins");
    let text = screen(&mut app);
    for part in [
        " Plugins of the architect ",
        "▶ [ ] review",
        "✗ not allowed",
        "[ ] lint",
        "Agent: claude",
        "From: official/review · 0123456",
        "Inside: 1 skills, 1 commands, 0 subagents",
        "Hooks: yes, not allowed",
        "The review plugin",
        " Give to the architect ",
        " Allow hooks ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }

    // Saving checks the plugin as a run would: its hooks are not allowed.
    key(&mut app, KeyCode::Char(' '));
    assert!(screen(&mut app).contains("▶ [x] review"));
    key(&mut app, KeyCode::Char('s'));
    let (message, problem) = app.message.clone().unwrap();
    assert!(problem && message.contains("hooks"), "{message}");
    key(&mut app, KeyCode::Char('u'));

    // Allowing hooks asks first, then writes and commits.
    click(&mut app, " Allow hooks ");
    assert!(screen(&mut app).contains("Allow the plugin to run programs"));
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    assert!(config(&root).plugins["review"].allow_hooks);
    let text = screen(&mut app);
    assert!(text.contains("Hooks: yes, allowed"), "{text}");
    assert!(text.contains(" Forbid hooks "), "{text}");
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(config(&root).roles[&Role::Architect].plugins, ["review"]);
    assert!(repo.changed_files().unwrap().is_empty());

    // Forbidding hooks the architect needs is refused, and nothing changes.
    click(&mut app, " Forbid hooks ");
    let (message, problem) = app.message.clone().unwrap();
    assert!(
        problem && message.contains("Take review from its roles first"),
        "{message}"
    );
    assert!(config(&root).plugins["review"].allow_hooks);

    // A Codex role gets Codex plugins only.
    click(&mut app, " developer ");
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] lint"), "{text}");
    click(&mut app, "[ ] review");
    assert!(screen(&mut app).contains("The developer runs on codex"));
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "This plugin is for another agent"
    );
    // Antigravity has none at all.
    click(&mut app, " security ");
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "The role's agent has no plugins"
    );

    // «Open in Zed» opens the folder; what was changed there is committed.
    click(&mut app, " architect ");
    key(&mut app, KeyCode::Char('e'));
    let job = app.edit.take().unwrap();
    assert!(job.kind == EditKind::Plugin && job.path.ends_with(".harness/plugins/review"));
    app.finish_edit(&job, Ok(()));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "Nothing changed in the plugin"
    );
    fs::write(job.path.join("commands/new.md"), "# new").unwrap();
    app.finish_edit(&job, Ok(()));
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("changes committed"));
    assert!(repo.changed_files().unwrap().is_empty());
    assert!(screen(&mut app).contains("2 commands"));

    // Removing asks first; the folder and the settings go in one commit.
    click(&mut app, " developer ");
    key(&mut app, KeyCode::Delete);
    assert!(screen(&mut app).contains("Remove plugin lint from the project?"));
    key(&mut app, KeyCode::Enter);
    assert!(!config(&root).plugins.contains_key("lint"));
    assert!(!root.join(".harness/plugins/lint").exists());
    assert!(repo.changed_files().unwrap().is_empty());
    let text = screen(&mut app);
    assert!(
        !text.contains("] lint") && text.contains("Plugin lint removed"),
        "{text}"
    );
}

/// Waits for a plugin or catalog download.
fn wait_job(app: &mut App) {
    let start = Instant::now();
    while app.plugin_job.is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the download hangs"
        );
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
}

#[test]
fn plugins_come_from_the_catalog_and_are_updated() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let repo = Repo::open(&root).unwrap();
    repo.commit_all("tasks").unwrap();
    // A catalog folder on this computer stands in for the official one.
    let catalog = tempfile::tempdir().unwrap();
    let write = |file: &str, text: &str| {
        let path = catalog.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        ".claude-plugin/marketplace.json",
        r#"{"name": "official", "plugins": [
            {"name": "review", "description": "Reviews code for bugs", "source": "./plugins/review"},
            {"name": "notes", "description": "Keeps notes", "source": "./plugins/notes"},
            {"name": "pkg", "description": "From npm", "source": {"source": "npm", "package": "x"}}
        ]}"#,
    );
    write(
        "plugins/review/.claude-plugin/plugin.json",
        r#"{"name": "review"}"#,
    );
    write("plugins/review/hooks/hooks.json", "{}");
    write(
        "plugins/notes/.claude-plugin/plugin.json",
        r#"{"name": "notes"}"#,
    );
    write("plugins/notes/commands/note.md", "# note");

    let mut app = env.app(&root);
    app.official_catalog = catalog.path().display().to_string();
    click(&mut app, "5 Plugins");
    // With no catalog, the official one is added by itself.
    click(&mut app, " From catalog ");
    wait_job(&mut app);
    let text = screen(&mut app);
    for part in [
        "Plugins in the catalogs",
        "Found 3",
        "▶ review",
        "notes",
        "pkg",
        "cannot be added",
        "Reviews code for bugs",
        " Add and give to the architect ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("Catalog official added: 3 plugins"));

    // Searching keeps only what matches.
    key(&mut app, KeyCode::Char('/'));
    fill(&mut app, "bugs");
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app);
    assert!(
        text.contains("Found 1") && text.contains("▶ review"),
        "{text}"
    );

    // A plugin with hooks: added, then asked about its hooks, then given.
    click(&mut app, " Add and give to the architect ");
    wait_job(&mut app);
    let text = screen(&mut app);
    assert!(text.contains("Allow the plugin to run programs"), "{text}");
    key(&mut app, KeyCode::Enter);
    let saved = config(&root);
    assert!(saved.plugins["review"].allow_hooks);
    assert_eq!(
        saved.plugins["review"].source.as_deref(),
        Some("official/review")
    );
    assert_eq!(saved.roles[&Role::Architect].plugins, ["review"]);
    assert!(repo.changed_files().unwrap().is_empty());
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] review"), "{text}");

    // A newer version in the catalog: «Update» shows the files, then takes it.
    write("plugins/review/commands/fix.md", "# fix");
    key(&mut app, KeyCode::Char('U'));
    wait_job(&mut app);
    let text = screen(&mut app);
    assert!(text.contains("+ commands/fix.md"), "{text}");
    key(&mut app, KeyCode::Enter);
    assert!(root
        .join(".harness/plugins/review/commands/fix.md")
        .is_file());
    assert!(repo.changed_files().unwrap().is_empty());
    key(&mut app, KeyCode::Char('U'));
    wait_job(&mut app);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("already up to date"));

    // An update that is not taken leaves nothing behind.
    write("plugins/review/commands/more.md", "# more");
    key(&mut app, KeyCode::Char('U'));
    wait_job(&mut app);
    key(&mut app, KeyCode::Esc);
    assert!(!root
        .join(".harness/plugins/review/commands/more.md")
        .exists());
    assert!(repo.changed_files().unwrap().is_empty());

    // «Catalogs» lists it; removing it keeps the plugin.
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('c'));
    let text = screen(&mut app);
    assert!(
        text.contains("official") && text.contains("3 plugins"),
        "{text}"
    );
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("No catalogs yet"));
    assert!(root.join(".harness/plugins/review").is_dir());
}

/// The retrospective's agent: it writes its lessons and one proposal.
fn mock_retro(
    _: &harness_core::config::Config,
) -> Result<harness_agents::AnyAgent, harness_agents::build::BuildError> {
    use harness_agents::{AnyAgent, MockAgent, MockStep};
    let proposals = r#"{"proposals": [{
        "id": 1,
        "summary": "Teach the developer to check empty input",
        "reason": "task-001 failed on empty input",
        "skill": "empty-input",
        "content": "---\ndescription: Check empty input.\n---\nCheck it first.\n",
        "roles": [{"role": "developer", "list": "skills"}]
    }]}"#;
    Ok(AnyAgent::Mock(MockAgent::new().then(
        Role::Security,
        MockStep::WriteOutput(vec![
            ("retro.md".into(), "Went well: small steps.".into()),
            ("proposals.json".into(), proposals.into()),
        ]),
    )))
}

fn wait_retro(app: &mut App) {
    let start = Instant::now();
    while app.retro.as_ref().unwrap().is_generating() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the retrospective did not finish"
        );
        std::thread::sleep(Duration::from_millis(20));
        app.tick();
    }
}

#[test]
fn a_retrospective_is_generated_edited_and_its_proposals_applied() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    app.retro_builder = mock_retro;
    let repo = Repo::open(&root).unwrap();
    key(&mut app, KeyCode::Char('6'));
    let text = screen(&mut app);
    assert!(text.contains("No retrospectives yet."), "{text}");
    assert!(text.contains(" Generate "), "{text}");

    // Without tasks there is nothing to learn from.
    click(&mut app, " Generate ");
    wait_retro(&mut app);
    let (message, problem) = app.message.clone().unwrap();
    assert!(
        problem && message.contains("there are no tasks yet"),
        "{message}"
    );

    orchestrator::create_task(&repo, "task-001", "Build a parser", 5).unwrap();
    click(&mut app, " Generate ");
    assert!(app.retro.as_ref().unwrap().is_generating());
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("agent is still working"));
    // The roles wait for the retrospective.
    app.tab = Tab::Tasks;
    app.press(ButtonId::Send);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .contains("retrospective's agent is working"));
    app.tab = Tab::Retro;
    wait_retro(&mut app);
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "Retrospective 001 is ready"
    );
    assert!(repo.changed_files().unwrap().is_empty());
    let text = screen(&mut app);
    assert!(text.contains("001  "), "{text}");
    assert!(text.contains("whole project"), "{text}");
    assert!(text.contains("Went well: small steps."), "{text}");
    assert!(text.contains("── Statistics ──"), "{text}");
    assert!(text.contains("[ ] 1 Teach the developer"), "{text}");

    // A proposal shows what it changes; chosen ones are applied after a question.
    click(&mut app, "[ ] 1 Teach");
    let text = screen(&mut app);
    assert!(
        text.contains("New file .harness/skills/empty-input.md"),
        "{text}"
    );
    assert!(text.contains("+ Check it first."), "{text}");
    key(&mut app, KeyCode::Char(' '));
    assert!(screen(&mut app).contains("[x] 1 Teach"));
    click(&mut app, " Apply chosen (1) ");
    let text = screen(&mut app);
    assert!(text.contains("new skill empty-input"), "{text}");
    assert!(
        text.contains("empty-input is given to: developer"),
        "{text}"
    );
    key(&mut app, KeyCode::Enter);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .0
        .starts_with("Applied and committed: 1."));
    assert!(root.join(".harness/skills/empty-input.md").is_file());
    assert!(config(&root).roles[&Role::Developer]
        .skills
        .contains(&"empty-input".to_string()));
    assert!(repo.changed_files().unwrap().is_empty());
    let text = screen(&mut app);
    assert!(text.contains("✓   1 Teach"), "{text}");
    assert!(text.contains("✓ Applied: the skill empty-input"), "{text}");
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "This proposal is already applied"
    );

    // «Open in Zed»: what Lisa writes there is committed.
    key(&mut app, KeyCode::Char('e'));
    let job = app.edit.take().unwrap();
    assert!(job.kind == EditKind::Retro && job.path.ends_with(".harness/retros/001/retro.md"));
    app.finish_edit(&job, Ok(()));
    assert_eq!(app.message.as_ref().unwrap().0, "Nothing changed");
    fs::write(&job.path, "Lesson: write tests first.").unwrap();
    app.finish_edit(&job, Ok(()));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "Retrospective 001 saved and committed"
    );
    assert!(repo.changed_files().unwrap().is_empty());
    click(&mut app, "001  ");
    assert!(screen(&mut app).contains("Lesson: write tests first."));
}

#[test]
fn files_of_a_step_open_in_zed_with_a_click() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    // The developer's step created one file and changed another.
    let (store, mut state) = TaskStore::open(&root.join(".harness/runs"), "task-001").unwrap();
    let mut developer = handoff(
        Role::Developer,
        Verdict::Approved,
        NextStep::To(Role::Tester),
        "Empty input fixed",
    );
    developer.round = 2;
    developer.files = vec![
        harness_core::handoff::FileChange {
            path: "src/parser.rs".into(),
            action: harness_core::handoff::FileAction::Modified,
        },
        harness_core::handoff::FileChange {
            path: "docs/parser.md".into(),
            action: harness_core::handoff::FileAction::Created,
        },
        harness_core::handoff::FileChange {
            path: "Cargo.toml".into(),
            action: harness_core::handoff::FileAction::Read,
        },
    ];
    store.record(&mut state, &developer, "Fixed.").unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/parser.md"), "# Parser\n").unwrap();

    // Before the step's commit: the files its handoff lists are links.
    let mut app = env.app(&root);
    // Zed is not started in the tests: a command that does nothing.
    app.viewer = |_| Some(std::process::Command::new("true"));
    let text = screen(&mut app);
    for part in [
        "Files (click: open in Zed):",
        "~ src/parser.rs",
        "+ docs/parser.md",
        "· notes.md",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Files that were only read are not listed.
    assert!(!text.contains("Cargo.toml"), "{text}");

    click(&mut app, "docs/parser.md");
    app.open_task_file();
    assert_eq!(
        app.message,
        Some(("Opened in Zed: docs/parser.md".to_string(), false))
    );
    // A file that is not there any more is not opened.
    click(&mut app, "src/parser.rs");
    app.open_task_file();
    assert_eq!(
        app.message,
        Some(("src/parser.rs is not there any more".to_string(), true))
    );

    // After the commit the list comes from git: also what the handoff forgot,
    // and nothing of the run records.
    fs::write(root.join("docs/forgotten.md"), "x").unwrap();
    Repo::open(&root)
        .unwrap()
        .commit_all("task-001 round 2")
        .unwrap();
    let mut app = env.app(&root);
    app.viewer = |_| None;
    let text = screen(&mut app);
    assert!(text.contains("+ docs/forgotten.md"), "{text}");
    assert!(text.contains("+ docs/parser.md"), "{text}");
    assert!(!text.contains("~ src/parser.rs"), "{text}");
    assert!(!text.contains("handoff.json"), "{text}");

    // Without Zed the file goes to the editor in the terminal, and nothing
    // is committed after.
    click(&mut app, "notes.md");
    app.open_task_file();
    let job = app.edit.take().expect("the notes in the editor");
    assert!(job.path.ends_with("notes.md"), "{job:?}");
    app.finish_edit(&job, Ok(()));
    assert_eq!(app.message, None);
    assert!(app.tasks.as_ref().unwrap().open.is_none());
}
