use std::fs;

use super::*;
use harness_core::git::Repo;
use harness_core::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};
use harness_core::orchestrator;
use harness_core::projects::Projects;
use harness_core::store::TaskStore;
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
    assert!(screen(&mut app).contains("arrives in step 3 of the plan"));
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
        "[ agent's default ]",
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
    click(&mut app, "[ agent's default ]");
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
    assert!(text.contains("[ To: New task ▾ ] [ Send ]"), "{text}");

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
    key(&mut app, KeyCode::Enter);
    assert!(app.tasks.as_ref().unwrap().is_running());
    assert!(screen(&mut app).contains("[ Working… ]"));
    wait(&mut app);

    let store_dir = root.join(".harness/runs/task-001");
    assert_eq!(
        fs::read_to_string(store_dir.join("task.md")).unwrap(),
        "Build a CSV parser, q 2"
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
    key(&mut app, KeyCode::Enter);
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
    assert!(screen(&mut app).contains("[ To: New task ▾ ]"));
}

#[test]
fn the_to_list_offers_the_roles_and_finishing() {
    use harness_core::task::Stage;
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    app.tasks.as_mut().unwrap().paste("Line one\nLine two");
    assert!(screen(&mut app).contains("Line one ⏎ Line two▏"));
    key(&mut app, KeyCode::Enter);
    wait(&mut app);

    click(&mut app, "[ To: developer ▾ ]");
    let text = screen(&mut app);
    for option in ["Approve and finish", "New task", "security"] {
        assert!(text.contains(option), "missing {option:?} in:\n{text}");
    }
    click(&mut app, "Approve and finish");
    assert!(!app.tasks.as_ref().unwrap().menu);
    assert!(screen(&mut app).contains("[ To: Approve and finish ▾ ]"));

    // ↑↓ in the field change whom it goes to.
    click(&mut app, "Write here");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    type_text(&mut app, "Good enough");
    key(&mut app, KeyCode::Enter);
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
    key(&mut app, KeyCode::Enter);
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
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(app.message.as_ref().unwrap().1);
    wait(&mut app);
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}
