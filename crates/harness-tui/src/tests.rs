use std::fs;

use super::*;
use harness_core::git::Repo;
use harness_core::handoff::{Handoff, Issue, NextStep, Role, Severity, Verdict};
use harness_core::orchestrator;
use harness_core::projects::Projects;
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

    fn app(&self, start: &Path) -> App {
        App::new(Some(self.home.path().to_path_buf()), start)
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
    key(&mut app, KeyCode::Char('2'));
    assert!(screen(&mut app).contains("arrives in step 2 of the plan"));
}

#[test]
fn a_new_project_is_created_from_the_form() {
    let env = Env::new();
    let mut app = env.app(env.code.path());
    click(&mut app, "[ New project ]");
    assert!(screen(&mut app).contains("A new folder with a git repository"));

    // OK with the folder left as it is only explains.
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("Type the folder of the new project"));

    let root = env.path("fresh");
    fill(&mut app, root.to_str().unwrap());
    key(&mut app, KeyCode::Tab);
    type_text(&mut app, "Fresh one");
    click(&mut app, "[ Create ]");

    assert!(app.form.is_none());
    assert_eq!(app.project.as_deref(), Some(root.as_path()));
    assert_eq!(app.tab, Tab::Tasks);
    assert!(has_config(&root));
    assert!(root.join(".git").is_dir());
    let text = screen(&mut app);
    assert!(text.contains("Project «Fresh one» created"), "{text}");
    assert_eq!(env.saved().projects[0].name, "Fresh one");

    // The same folder again is refused, the form stays with the problem.
    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Char('n'));
    fill(&mut app, root.to_str().unwrap());
    key(&mut app, KeyCode::Enter);
    let (_, form) = app.form.as_ref().unwrap();
    assert!(form
        .error
        .as_deref()
        .unwrap()
        .contains("already is a harness project"));
    key(&mut app, KeyCode::Esc);
    assert!(app.form.is_none());
}

#[test]
fn a_plain_folder_is_prepared_before_it_opens() {
    let env = Env::new();
    let folder = env.path("plain");
    fs::create_dir(&folder).unwrap();
    let mut app = env.app(env.code.path());

    key(&mut app, KeyCode::Char('o'));
    fill(&mut app, &env.path("missing").display().to_string());
    key(&mut app, KeyCode::Enter);
    assert!(app
        .form
        .as_ref()
        .unwrap()
        .1
        .error
        .as_deref()
        .unwrap()
        .contains("There is no folder"));

    fill(&mut app, folder.to_str().unwrap());
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.form, Some((Purpose::InitFolder(_), _))));
    // Cancel leaves the folder alone.
    click(&mut app, "[ Cancel ]");
    assert!(app.form.is_none());
    assert!(!has_config(&folder));

    key(&mut app, KeyCode::Char('o'));
    fill(&mut app, folder.to_str().unwrap());
    key(&mut app, KeyCode::Enter);
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

    // The button in the top bar opens the new project form from any tab.
    click(&mut again, "+ Новый проект");
    let (purpose, form) = again.form.as_ref().unwrap();
    assert_eq!(
        (purpose, form.title.as_str()),
        (&Purpose::NewProject, "Новый проект")
    );
    key(&mut again, KeyCode::Esc);

    key(&mut again, KeyCode::Char('L'));
    assert!(screen(&mut again).contains("1 Tasks"));
}
