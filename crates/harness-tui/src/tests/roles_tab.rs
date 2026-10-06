//! Tests of the Roles tab: agents, models, levels and skills for each role.

use super::*;

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
        "( ) dsh",
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
        AgentKind::Claude,
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
    assert_eq!(developer.agent, AgentKind::Codex);
    assert_eq!(developer.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(developer.always_skills, ["rust-errors"]);
    assert_eq!(saved.retro.unwrap().agent, AgentKind::Antigravity);
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

fn codex_models() -> Vec<(AgentKind, Result<ModelList, String>)> {
    vec![
        (
            AgentKind::Codex,
            Ok(ModelList {
                agent: AgentKind::Codex,
                fetched: 1,
                models: vec![
                    model("gpt-6.1-sol", &["low", "medium", "high"], Some("low"), true),
                    model("gpt-5.5", &["medium", "xhigh"], Some("medium"), false),
                ],
            }),
        ),
        (
            AgentKind::Antigravity,
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
