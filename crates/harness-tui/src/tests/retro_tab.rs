//! Tests of the Retro tab: generating a retrospective and applying its proposals.

use super::*;

/// The retrospective's agent: it writes its lessons and one proposal.
#[expect(
    clippy::unnecessary_wraps,
    reason = "a fake must match the `RetroBuilder` function type"
)]
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

/// Shows the proposal, chooses it and applies it: a new skill given to the developer.
fn apply_the_proposal(app: &mut App, root: &Path, repo: &Repo) {
    // A proposal shows what it changes; chosen ones are applied after a question.
    click(app, "[ ] 1 Teach");
    let text = screen(app);
    assert!(
        text.contains("New file .harness/skills/empty-input.md"),
        "{text}"
    );
    assert!(text.contains("+ Check it first."), "{text}");
    key(app, KeyCode::Char(' '));
    assert!(screen(app).contains("[x] 1 Teach"));
    click(app, " Apply chosen (1) ");
    let text = screen(app);
    assert!(text.contains("new skill empty-input"), "{text}");
    assert!(
        text.contains("empty-input is given to: developer"),
        "{text}"
    );
    key(app, KeyCode::Enter);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .starts_with("Applied and committed: 1."));
    assert!(root.join(".harness/skills/empty-input.md").is_file());
    assert!(config(root).roles[&Role::Developer]
        .skills
        .contains(&"empty-input".to_string()));
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    let text = screen(app);
    assert!(text.contains("✓   1 Teach"), "{text}");
    assert!(text.contains("✓ Applied: the skill empty-input"), "{text}");
    key(app, KeyCode::Char(' '));
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "This proposal is already applied"
    );
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
    // The tab says which project the retrospectives are of.
    assert!(text.contains("Retrospectives · fresh"), "{text}");

    // «Generate» first asks about the open project; Esc changes nothing.
    click(&mut app, " Generate ");
    let text = screen(&mut app);
    assert!(
        text.contains("Make a retrospective of the project «fresh»?"),
        "{text}"
    );
    key(&mut app, KeyCode::Esc);
    assert!(app.form.is_none());
    assert!(!app.retro.as_ref().unwrap().is_generating());

    // Without tasks there is nothing to learn from.
    click(&mut app, " Generate ");
    key(&mut app, KeyCode::Enter);
    wait_retro(&mut app);
    let (message, problem) = shown(&app);
    assert!(
        problem && message.contains("there are no tasks yet"),
        "{message}"
    );

    orchestrator::create_task(&repo, "task-001", "Build a parser", 5).unwrap();
    click(&mut app, " Generate ");
    key(&mut app, KeyCode::Enter);
    assert!(app.retro.as_ref().unwrap().is_generating());
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("agent is still working"));
    // The roles wait for the retrospective.
    app.tab = Tab::Tasks;
    app.press(ButtonId::Send);
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("retrospective's agent is working"));
    app.tab = Tab::Retro;
    wait_retro(&mut app);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Retrospective 001 is ready"
    );
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    let text = screen(&mut app);
    assert!(text.contains("001  "), "{text}");
    assert!(text.contains("whole project"), "{text}");
    assert!(text.contains("Went well: small steps."), "{text}");
    assert!(text.contains("── Statistics ──"), "{text}");
    assert!(text.contains("[ ] 1 Teach the developer"), "{text}");

    apply_the_proposal(&mut app, &root, &repo);

    // «Open in editor»: what Lisa writes there is committed.
    key(&mut app, KeyCode::Char('e'));
    let job = app.edit.take().unwrap();
    assert!(job.kind == EditKind::Retro && job.path.ends_with(".harness/retros/001/retro.md"));
    app.finish_edit(&job, Ok(()));
    assert_eq!(app.message.as_ref().unwrap().text, "Nothing changed");
    fs::write(&job.path, "Lesson: write tests first.").unwrap();
    app.finish_edit(&job, Ok(()));
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Retrospective 001 saved and committed"
    );
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    click(&mut app, "001  ");
    assert!(screen(&mut app).contains("Lesson: write tests first."));
}
