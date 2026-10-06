//! Tests of the Projects tab and of the start: choosing, creating and removing projects.

use super::*;

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
    assert!(screen_of_width(&mut app, 400).contains(&b.display().to_string()));

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
