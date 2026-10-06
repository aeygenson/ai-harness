//! Tests of the Plugins tab: giving plugins to roles, the catalog and updates.

use super::*;

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
