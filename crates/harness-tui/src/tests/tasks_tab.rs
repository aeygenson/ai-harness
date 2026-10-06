//! Tests of the Tasks tab: starting tasks, running roles, answers, windows and files.

use super::*;

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

fn no_login(
    _: &harness_core::config::Config,
    _: &Path,
) -> Result<harness_agents::Team, harness_agents::build::BuildError> {
    Err(harness_agents::build::BuildError(
        "no Claude token saved; sign in on the Agents tab first".into(),
    ))
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
    assert_eq!(
        app.tasks.as_ref().unwrap().choice,
        tabs::tasks::Choice::NewTask
    );

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
    assert!(message.contains("sign in on the Agents tab"), "{message}");
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
        harness_core::task::handoff::FileChange {
            path: "src/parser.rs".into(),
            action: harness_core::task::handoff::FileAction::Modified,
        },
        harness_core::task::handoff::FileChange {
            path: "docs/parser.md".into(),
            action: harness_core::task::handoff::FileAction::Created,
        },
        harness_core::task::handoff::FileChange {
            path: "Cargo.toml".into(),
            action: harness_core::task::handoff::FileAction::Read,
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

#[test]
fn a_click_on_a_title_shows_the_window_over_the_whole_tab() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    let text = screen(&mut app);
    assert!(text.contains("╭ ⤢ round 1 · tester "), "{text}");
    assert!(text.contains("╭ Roles "), "{text}");

    // The step over the whole tab; the message box stays.
    click(&mut app, "⤢ round 1 · tester");
    let text = screen(&mut app);
    assert!(text.contains("╭ ⤡ round 1 · tester "), "{text}");
    assert!(!text.contains("╭ Roles "), "{text}");
    assert!(!text.contains("r1 architect  approved"), "{text}");
    assert!(text.contains("Tester notes here."), "{text}");
    assert!(text.contains("╭ Message "), "{text}");
    // Another click puts it back.
    click(&mut app, "⤡ round 1 · tester");
    assert!(screen(&mut app).contains("╭ Roles "));

    // The steps, then Esc puts them back.
    click(&mut app, "⤢ task-001 · round 2");
    let text = screen(&mut app);
    assert!(text.contains("⤡ task-001 · round 2"), "{text}");
    assert!(!text.contains("round 1 · tester "), "{text}");
    key(&mut app, KeyCode::Esc);
    assert!(!app.quit);
    assert!(screen(&mut app).contains("╭ Roles "));

    // The agent log, while there is one.
    app.tasks
        .as_mut()
        .unwrap()
        .log
        .push_back("cargo test: ok".to_string());
    click(&mut app, "⤢ Agent log");
    let text = screen(&mut app);
    assert!(text.contains("⤡ Agent log"), "{text}");
    assert!(text.contains("cargo test: ok"), "{text}");
    assert!(!text.contains("╭ Roles "), "{text}");
}

#[test]
fn the_same_task_sent_twice_is_not_started_again() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    click(&mut app, "Write here");
    type_text(&mut app, "Fix the demo");
    send(&mut app);
    wait(&mut app);
    assert!(root.join(".harness/runs/task-001").is_dir());

    // The same text again as a new task: refused, the text stays in the box.
    let tasks = app.tasks.as_mut().unwrap();
    tasks.choice = tabs::tasks::Choice::NewTask;
    tasks.focus_input();
    type_text(&mut app, "Fix  the\ndemo ");
    send(&mut app);
    let (text, problem) = app.message.clone().unwrap();
    assert!(problem, "{text}");
    assert!(
        text.starts_with("task-001 already has this text and is not done (waiting"),
        "{text}"
    );
    let tasks = app.tasks.as_ref().unwrap();
    assert_eq!(tasks.input, "Fix  the\ndemo ");
    assert!(!tasks.is_running());
    assert!(!root.join(".harness/runs/task-002").exists());
}
