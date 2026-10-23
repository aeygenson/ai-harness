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
    assert_eq!(app.message.as_ref().unwrap().text, "Write the task first");

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
    assert!(app.message.as_ref().unwrap().text.contains("not sent"));
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
    let (message, problem) = shown(&app);
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
    assert!(app.message.as_ref().unwrap().text.contains("done"));
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
        .text
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
    assert_eq!(app.message.as_ref().unwrap().text, "task-001: saved");
    let steps = TaskStore::open(&root.join(".harness/runs"), "task-001")
        .unwrap()
        .0
        .steps()
        .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].handoff.next_role, NextStep::Done);
}

#[test]
fn more_rounds_raise_the_limit_and_send_the_work_on() {
    use harness_core::task::{Stage, WaitReason};
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    // A task with a limit of one round: the tester's rejection stops it.
    let repo = Repo::open(&root).unwrap();
    let (store, mut state) =
        orchestrator::create_task(&repo, "task-001", "Build a parser", Workflow::Task, 1).unwrap();
    let steps = [
        (Role::Architect, Verdict::Approved, Role::Human),
        (Role::Human, Verdict::Approved, Role::Developer),
        (Role::Developer, Verdict::Approved, Role::Tester),
        (Role::Tester, Verdict::Rejected, Role::Developer),
    ];
    for (role, verdict, next) in steps {
        let mut step = handoff(role, verdict, NextStep::To(next), "step");
        if verdict == Verdict::Rejected {
            step.issues = vec![Issue {
                severity: Severity::High,
                location: None,
                description: "Panics on empty input".into(),
            }];
        }
        store.record(&mut state, &step, "notes").unwrap();
    }
    repo.commit_all("task at its round limit").unwrap();
    key(&mut app, KeyCode::F(5));
    assert_eq!(
        stage(&app),
        Stage::WaitingForHuman(WaitReason::RoundLimitReached)
    );

    click(&mut app, " To: developer ▾ ");
    click(&mut app, "developer · 5 more rounds");
    assert!(screen(&mut app).contains(" To: developer · 5 more rounds ▾ "));
    click(&mut app, "Write here");
    type_text(&mut app, "Try once more");
    send(&mut app);
    wait(&mut app);

    // The roles went on to the end instead of stopping at the old limit.
    assert_eq!(stage(&app), Stage::Done);
    let (_, state) = TaskStore::open(&root.join(".harness/runs"), "task-001").unwrap();
    assert_eq!(state.max_rounds, 6);
    let steps = store.steps().unwrap();
    assert_eq!(steps[4].handoff.role, Role::Human);
    assert_eq!(steps[4].handoff.next_role, NextStep::To(Role::Developer));
    assert_eq!(steps.last().unwrap().handoff.role, Role::Security);
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
    let (message, problem) = shown(&app);
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
    assert_eq!(app.message.as_ref().unwrap().kind, MessageKind::Error);
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
            agent: AgentKind::Claude,
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
        "Files (click: open in the editor):",
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
        Some(Message::info("Opened in Zed: docs/parser.md"))
    );
    // A file that is not there any more is not opened.
    click(&mut app, "src/parser.rs");
    app.open_task_file();
    assert_eq!(
        app.message,
        Some(Message::error("src/parser.rs is not there any more"))
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
    let (text, problem) = shown(&app);
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

#[test]
fn the_task_and_each_step_show_what_the_agents_spent() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let round = root.join(".harness/runs/task-001/round-01");
    let claude = "{\"type\":\"result\",\"total_cost_usd\":0.4,\
                  \"usage\":{\"input_tokens\":9000,\"output_tokens\":1000}}\n";
    fs::write(round.join("01-architect/agent.log"), claude).unwrap();
    let codex = "{\"type\":\"turn.completed\",\
                 \"usage\":{\"input_tokens\":4000,\"output_tokens\":1000}}\n";
    fs::write(round.join("03-developer/agent.log"), codex).unwrap();
    let mut app = env.app(&root);

    click(&mut app, "r1 architect");
    let architect = screen(&mut app);
    click(&mut app, "r1 developer");
    let developer = screen(&mut app);
    click(&mut app, "r1 human");
    let human = screen(&mut app);

    // The whole task: both costs added, the cost only from the step that had one.
    assert!(architect.contains("· $0.40 · 15k tokens"), "{architect}");
    assert!(
        architect.contains("Spent: $0.40 · 10k tokens"),
        "{architect}"
    );
    assert!(developer.contains("Spent: 5k tokens"), "{developer}");
    assert!(!human.contains("Spent:"), "{human}");
}

#[test]
fn a_decision_on_an_old_copy_of_a_task_on_another_branch_is_refused() {
    use harness_core::git::Repo;
    use harness_core::task::orchestrator::{self, SameText};
    use harness_core::task::{Stage, WaitReason};
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let repo = Repo::open(&root).unwrap();
    let settings = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(
        &settings,
        format!("{text}\n[github]\nrepo = \"lisa/app\"\n"),
    )
    .unwrap();
    repo.commit_all("on GitHub").unwrap();
    click(&mut app, "Write here");
    type_text(&mut app, "First task");
    send(&mut app);
    wait(&mut app);
    assert_eq!(
        repo.current_branch().unwrap().as_deref(),
        Some("harness/task-001")
    );
    // task-002 starts from task-001's branch, with a copy of task-001 that
    // waits for the design; then task-001 moves on in its own branch.
    orchestrator::create_project_task(
        &repo,
        &config(&root),
        "task-002",
        "Second",
        SameText::Refuse,
    )
    .unwrap();
    repo.switch_branch("harness/task-001").unwrap();
    let (store, mut state) = TaskStore::open(&repo.runs_dir(), "task-001").unwrap();
    orchestrator::record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Developer),
        "go",
    )
    .unwrap();
    repo.switch_branch("harness/task-002").unwrap();
    key(&mut app, KeyCode::F(5));
    click(&mut app, "task-001");
    assert_eq!(
        stage(&app),
        Stage::WaitingForHuman(WaitReason::ApproveDesign)
    );

    key(&mut app, KeyCode::Enter);
    type_text(&mut app, "approve again");
    send(&mut app);
    wait(&mut app);

    let (message, problem) = shown(&app);
    assert!(problem, "{message}");
    assert!(message.contains("has moved on"), "{message}");
    assert_eq!(
        repo.current_branch().unwrap().as_deref(),
        Some("harness/task-001")
    );
    // Nothing was decided twice: the last step is still Lisa's first answer.
    let steps = store.steps().unwrap();
    assert_eq!(steps.last().unwrap().handoff.summary, "go");
}

#[test]
fn a_done_task_on_github_shows_its_pull_request_or_offers_to_send_it() {
    use harness_core::task::pull_request::PullRequest;
    use harness_core::task::store::TaskStore;
    use harness_core::task::Stage;
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let config = root.join(".harness/harness.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{text}\n[github]\nrepo = \"lisa/app\"\n")).unwrap();
    let runs = root.join(".harness/runs");
    for (task, pull_request) in [
        (
            "task-001",
            Some(PullRequest {
                number: 7,
                url: "https://github.com/lisa/app/pull/7".into(),
            }),
        ),
        ("task-002", None),
    ] {
        let (store, mut state) = TaskStore::open(&runs, task).unwrap();
        state.stage = Stage::Done;
        state.pull_request = pull_request;
        let json = serde_json::to_string(&state).unwrap();
        std::fs::write(store.dir().join("state.json"), json).unwrap();
    }
    let mut app = env.app(&root);

    // The pull request is on the task's row; a click opens it.
    click(&mut app, "PR #7");
    let message = &app.message.as_ref().unwrap().text;
    assert_eq!(
        message,
        "Cannot open https://github.com/lisa/app/pull/7: no browser in tests"
    );
    // The key o does the same for the selected task.
    app.message = None;
    key(&mut app, KeyCode::Char('o'));
    assert!(app.message.is_some());

    // A task with a pull request can have it merged; no role runs for that.
    key(&mut app, KeyCode::Up);
    click(&mut app, " To: architect · new task ▾ ");
    click(&mut app, "Merge pull request");
    let text = screen(&mut app);
    assert!(text.contains(" To: Merge pull request ▾ "), "{text}");
    assert!(!text.contains("[ Model:"), "{text}");

    // The other one has no pull request yet: sending it is what comes next,
    // and there is nothing to merge.
    click(&mut app, "task-002");
    let text = screen(&mut app);
    assert!(text.contains("To: Send to GitHub ▾"), "{text}");
    assert!(
        text.contains("│▶ task-002  ✓ done                │"),
        "{text}"
    );
    assert_eq!(app.tasks.as_ref().unwrap().pull_request_url(1), None);
    click(&mut app, " To: Send to GitHub ▾ ");
    assert!(!screen(&mut app).contains("Merge pull request"));
}

#[test]
fn a_fix_asked_for_by_security_goes_to_the_developer_by_default() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let repo = Repo::open(&root).unwrap();
    let (store, mut state) =
        orchestrator::create_task(&repo, "task-001", "Build a parser", Workflow::Task, 5).unwrap();
    for (role, next) in [
        (Role::Architect, Role::Human),
        (Role::Human, Role::Developer),
        (Role::Developer, Role::Tester),
        (Role::Tester, Role::Security),
    ] {
        let step = handoff(role, Verdict::Approved, NextStep::To(next), "step");
        store.record(&mut state, &step, "notes").unwrap();
    }
    // Security may not change files; it found something that needs one.
    let mut asked = handoff(
        Role::Security,
        Verdict::NeedsHuman,
        NextStep::To(Role::Human),
        "notices missing",
    );
    asked.issues = vec![Issue {
        severity: Severity::Medium,
        location: None,
        description: "A licence notice is missing".into(),
    }];
    store.record(&mut state, &asked, "notes").unwrap();
    repo.commit_all("security asked for a fix").unwrap();
    key(&mut app, KeyCode::F(5));

    assert!(screen(&mut app).contains(" To: developer ▾ "));
}

#[test]
fn a_question_from_security_without_issues_goes_back_to_security() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let repo = Repo::open(&root).unwrap();
    let (store, mut state) =
        orchestrator::create_task(&repo, "task-001", "Build a parser", Workflow::Task, 5).unwrap();
    for (role, next) in [
        (Role::Architect, Role::Human),
        (Role::Human, Role::Developer),
        (Role::Developer, Role::Tester),
        (Role::Tester, Role::Security),
    ] {
        let step = handoff(role, Verdict::Approved, NextStep::To(next), "step");
        store.record(&mut state, &step, "notes").unwrap();
    }
    let asked = handoff(
        Role::Security,
        Verdict::NeedsHuman,
        NextStep::To(Role::Human),
        "may this port be opened?",
    );
    store.record(&mut state, &asked, "notes").unwrap();
    repo.commit_all("security asked a question").unwrap();
    key(&mut app, KeyCode::F(5));

    assert!(screen(&mut app).contains(" To: security ▾ "));
}

/// A task whose architect asked Lisa for help; `design` says whether its
/// handoff lists a design it wrote.
fn architect_asked(env: &Env, design: bool) -> (PathBuf, App) {
    let (root, mut app) = empty_project(env);
    let repo = Repo::open(&root).unwrap();
    let (store, mut state) =
        orchestrator::create_task(&repo, "task-001", "Build a parser", Workflow::Task, 5).unwrap();
    let mut asked = handoff(
        Role::Architect,
        Verdict::NeedsHuman,
        NextStep::To(Role::Human),
        "JSON or TOML?",
    );
    if design {
        asked.files = vec![harness_core::task::handoff::FileChange {
            path: "docs/design/task-001.md".into(),
            action: harness_core::task::handoff::FileAction::Created,
        }];
    }
    store
        .record(&mut state, &asked, "Which format should the parser read?")
        .unwrap();
    repo.commit_all("the architect asked").unwrap();
    key(&mut app, KeyCode::F(5));
    (root, app)
}

#[test]
fn an_architect_that_wrote_a_design_hands_the_answer_to_the_developer() {
    let env = Env::new();
    let (_, mut app) = architect_asked(&env, true);
    assert!(screen(&mut app).contains(" To: developer ▾ "));
}

#[test]
fn a_question_from_the_architect_without_a_design_goes_back_to_it() {
    let env = Env::new();
    let (_, mut app) = architect_asked(&env, false);
    assert!(screen(&mut app).contains(" To: architect ▾ "));
}

#[test]
fn the_questions_of_the_waiting_role_open_from_the_message_box() {
    let env = Env::new();
    let (root, mut app) = architect_asked(&env, false);
    // Zed is not started in the tests: a command that does nothing.
    app.viewer = |_| Some(std::process::Command::new("true"));
    let link = "Questions: architect notes.md — open (n)";
    let text = screen(&mut app);
    // The link is on the top border of the message box.
    let (_, row) = find(&text, link);
    assert_eq!(row, find(&text, " Message ").1, "{text}");

    click(&mut app, link);
    let opened = app.tasks.as_ref().unwrap().open.clone().unwrap();
    assert!(opened.starts_with(&root), "{opened:?}");
    assert!(opened.ends_with("notes.md"), "{opened:?}");
    app.open_task_file();
    let (message, problem) = shown(&app);
    assert!(
        !problem && message.starts_with("Opened in Zed: "),
        "{message}"
    );

    // The key does the same.
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.tasks.as_ref().unwrap().open, Some(opened.clone()));
    app.open_task_file();

    // Without the notes there is no link, and the key does nothing.
    fs::remove_file(&opened).unwrap();
    assert!(!screen(&mut app).contains(link));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.tasks.as_ref().unwrap().open, None);
}

#[test]
fn no_questions_link_while_a_role_works() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    assert!(!screen(&mut app).contains("Questions:"));
}
