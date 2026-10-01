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
        " test │ 1 Tasks │ 2 Roles",
        "7 Projects",
        "[ + New project ] [ EN ]",
        "> task-001  working",
        "task-002  working",
        "architect  claude",
        "r1 architect  approved → human",
        "> r1 tester",
        "high     src/parser.rs:42  Panics on empty input",
        "Tester notes here.",
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
    assert!(screen(&mut app).contains("arrives in step 4 of the plan"));
}

#[test]
fn a_new_project_is_created_in_a_folder_chosen_in_the_browser() {
    let env = Env::new();
    fs::create_dir(env.path("work")).unwrap();
    let mut app = env.app(env.code.path());
    click(&mut app, "[ New project ]");
    let text = screen(&mut app);
    assert!(text.contains("Folder for the new project"), "{text}");
    assert!(text.contains("work/"), "{text}");

    // Into «work», then a new folder «fresh» made there.
    click(&mut app, "work/");
    click(&mut app, "work/");
    assert!(screen(&mut app).contains("No folders inside"));
    click(&mut app, "[ New folder ]");
    type_text(&mut app, "fresh");
    key(&mut app, KeyCode::Enter);
    let root = env.path("work/fresh");
    assert!(root.is_dir());
    click(&mut app, "[ Choose this folder ]");

    // The form shows the folder and asks for the name.
    let text = screen(&mut app);
    assert!(text.contains("The project will be in"), "{text}");
    let (_, form) = app.form.as_ref().unwrap();
    assert_eq!(form.value(0), "fresh");
    fill(&mut app, "Fresh one");
    click(&mut app, "[ Create ]");

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
    click(&mut app, "[ Cancel ]");
    assert!(app.form.is_none());
    assert!(!has_config(&folder));

    key(&mut app, KeyCode::Char('o'));
    click(&mut app, "plain/");
    click(&mut app, "[ Choose this folder ]");
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
    click(&mut app, "Remove from list ]");
    assert!(screen(&mut app).contains("leaves the list"));
    click(&mut app, "[ Remove ]");
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

    click(&mut app, "[ EN ]");
    let text = screen(&mut app);
    for part in [
        "1 Задачи │ 2 Роли",
        "[ + Новый проект ] [ RU ]",
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
    assert!(text.contains("[ Выбрать эту папку ]"), "{text}");
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

    click(&mut app, "[ Save ]");
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
    click(&mut app, "[ Save ]");
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
    click(&mut app, "[ Undo changes ]");
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
        text.contains("[ To: architect · new task ▾ ] [ Model: default ▾ ]"),
        "{text}"
    );

    // Sending nothing is refused.
    click(&mut app, "[ Send ]");
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
    assert!(screen(&mut app).contains("[ Working… ]"));
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
    assert!(text.contains("[ To: developer ▾ ]"), "{text}");
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
    assert!(screen(&mut app).contains("[ To: architect · new task ▾ ]"));
}

#[test]
fn the_to_list_offers_the_roles_and_finishing() {
    use harness_core::task::Stage;
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    // Before there is a task, only a new one can be sent: the roles are
    // listed, but grey.
    click(&mut app, "[ To: architect · new task ▾ ]");
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

    click(&mut app, "[ To: developer ▾ ]");
    let text = screen(&mut app);
    for option in ["Approve and finish", "architect · new task", "security"] {
        assert!(text.contains(option), "missing {option:?} in:\n{text}");
    }
    click(&mut app, "Approve and finish");
    assert!(app.tasks.as_ref().unwrap().menu.is_none());
    assert!(screen(&mut app).contains("[ To: Approve and finish ▾ ]"));

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
    assert!(text.contains("> task-001"), "{text}");
    assert!(!text.contains("task-002"), "{text}");

    click(&mut app, "architect  claude");
    let text = screen(&mut app);
    assert!(text.contains("> task-002"), "{text}");
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
        "[ architect ]",
        "Skills of the architect",
        "Always in the prompt",
        "common",
        "● agent-claude",
        "○ agent-codex",
        "○ agent-antigravity",
        "Can be chosen on «Roles»",
        "[ ] crash-recovery",
        "built-in",
        "common · built-in",
        "[ Edit in Zed ]",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Another role shows its own skill; the selected skill stays.
    click(&mut app, "[ tester ]");
    let text = screen(&mut app);
    assert!(text.contains("Skills of the tester"), "{text}");
    assert!(text.contains("│  tester "), "{text}");
    assert!(!text.contains("│  architect "), "{text}");
    assert!(text.contains("> common"), "{text}");
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
fn a_built_in_skill_is_copied_edited_committed_and_restored() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let file = root.join(".harness/skills/developer.md");
    let mut app = env.app(&root);
    key(&mut app, KeyCode::Char('3'));
    click(&mut app, "[ developer ]");
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
    click(&mut app, "[ Restore built-in ]");
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
    click(&mut app, "[ New skill ]");
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
    click(&mut app, "[ Refresh models ]");
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
    click(&mut app, "[ Save ]");
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
    assert!(text.contains("[ Model: default ▾ ]"), "{text}");
    assert!(text.contains("[ Level: default ▾ ]"), "{text}");

    click(&mut app, "[ Model: default ▾ ]");
    let text = screen(&mut app);
    assert!(text.contains("│  haiku"), "{text}");
    click(&mut app, "│  opus");
    click(&mut app, "[ Level: default ▾ ]");
    click(&mut app, "│  max");
    let text = screen(&mut app);
    assert!(text.contains("[ Model: opus ▾ ]"), "{text}");
    assert!(text.contains("[ Level: max ▾ ]"), "{text}");
    // A model without that level gets its own default; haiku takes none.
    click(&mut app, "[ Model: opus ▾ ]");
    click(&mut app, "│  sonnet");
    assert!(screen(&mut app).contains("[ Level: high ▾ ]"));
    click(&mut app, "[ Model: sonnet ▾ ]");
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
    assert!(text.contains("[ Model: default ▾ ]"), "{text}");

    // A choice made for one role is dropped when the message goes to another.
    click(&mut app, "[ Model: default ▾ ]");
    click(&mut app, "│  haiku");
    let text = screen(&mut app);
    assert!(text.contains("[ Model: haiku ▾ ]"), "{text}");
    assert!(text.contains("[ Level: default ▾ ]"), "{text}");
    click(&mut app, "[ To: developer ▾ ]");
    click(&mut app, "│  security");
    assert!(screen(&mut app).contains("[ Model: default ▾ ]"));
    // Finishing runs no role: no model to choose.
    click(&mut app, "[ To: security ▾ ]");
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
        "> [ ] context7",
        "✗ secret",
        "[ ] fetch",
        "[ ] ghost",
        "not described",
        "Command: npx -y @upstash/context7-mcp",
        "CONTEXT7_API_KEY = secret:context7  ✗ not saved: harness secret set",
        "claude ✓",
        "antigravity ✓",
        "Roles: developer",
        "[ Give to the architect ]",
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
    click(&mut app, "[ developer ]");
    let text = screen(&mut app);
    assert!(text.contains("> [x] context7"), "{text}");
    assert!(text.contains("[ Take from the developer ]"), "{text}");
    click(&mut app, "[ ] fetch");
    click(&mut app, "[ ] fetch");
    let text = screen(&mut app);
    assert!(text.contains("> [x] fetch"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    // The Roles tab shows the same change.
    click(&mut app, "2 Roles");
    click(&mut app, "developer");
    assert!(screen(&mut app).contains("[x] fetch"));
    click(&mut app, "4 MCP");
    // Saving checks everything as before a run: the tester's unknown server
    // stops it.
    click(&mut app, "[ Save ]");
    let (message, problem) = app.message.clone().unwrap();
    assert!(problem && message.contains("ghost"), "{message}");

    // A server harness.toml does not describe cannot be given ...
    click(&mut app, "[ architect ]");
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
    click(&mut app, "[ tester ]");
    click(&mut app, "[x] ghost");
    key(&mut app, KeyCode::Char(' '));
    assert!(!screen(&mut app).contains("ghost"));
    key(&mut app, KeyCode::Char('s'));
    let saved = config(&root);
    assert_eq!(saved.roles[&Role::Developer].mcp, ["context7", "fetch"]);
    assert!(saved.roles[&Role::Tester].mcp.is_empty());
}
