//! Tests of the Agents tab: the catalog, install, update, remove and sign in.

use super::*;

/// Codex is installed but old, Claude is installed and new, nothing else.
fn fake_agents(credentials: Option<&Path>) -> Vec<harness_agents::install::catalog::Status> {
    use harness_agents::install::catalog;
    // Codex sits in the user's own npm folder, so the tab may update it.
    let local = harness_platform::home::home_dir().map_or_else(
        || PathBuf::from("/bin/codex"),
        |home| home.join(".local/bin/codex"),
    );
    let find = move |name: &str| match name {
        "claude" => Some(PathBuf::from("/bin/claude")),
        "codex" => Some(local.clone()),
        "dsh" => Some(PathBuf::from("/bin/dsh")),
        _ => None,
    };
    let version = |path: &Path| {
        Ok(if path.ends_with("codex") {
            "codex-cli 0.150.0".to_string()
        } else if path.ends_with("dsh") {
            "0.2.0-rc.2".to_string()
        } else {
            "2.1.300 (Claude Code)".to_string()
        })
    };
    catalog::check_with(catalog::CATALOG, &find, &version, credentials)
}

#[test]
fn the_agents_tab_shows_the_catalog_with_what_is_installed() {
    let env = Env::new();
    // Works without an open project.
    let mut app = env.app(env.code.path());
    app.agent_checker = fake_agents;
    key(&mut app, KeyCode::Char('8'));
    assert_eq!(app.tab, Tab::Agents);
    assert!(app.agents.checking);
    for _ in 0..100 {
        app.tick();
        if app.agents.known {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(app.agents.known && !app.agents.checking);
    for _ in 0..100 {
        app.tick();
        if !app.agents.tools.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = screen_of_width(&mut app, 160);
    // Under the list: the other programs, and how to get what is missing.
    let installer = harness_platform::program::installer_command();
    for part in [
        "Computer",
        "✓ Git 2.43.0",
        "! Node.js 20.11.1 · older than 22.19",
        "✗ npx not found",
        "Install what is missing",
        // The whole command fits in the panel (it wraps at its spaces).
        installer.split(' ').next().unwrap(),
        installer.split(' ').next_back().unwrap(),
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    for part in [
        "8 Agents",
        "✓ Claude Code",
        "! Codex CLI",
        "○ Antigravity CLI",
        "○ GitHub Copilot CLI  · not implemented",
        "Agents installed: 3 of",
        "○ Claude Code + GLM",
        "✓ Installed: /bin/claude · version 2.1.300",
        "No login: press «Sign in».",
        "Update with the maker's command:",
        "claude update",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }

    // An old Codex says so, and offers its update.
    key(&mut app, KeyCode::Down);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("! Older than 0.158.0"), "{text}");
    assert!(text.contains("@openai/codex@latest"), "{text}");

    // Not installed: the install command for this system.
    app.agents.select(2);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("○ Not installed"), "{text}");
    assert!(text.contains("Install with the maker's command:"), "{text}");

    // «Check again» asks once more; a click on the list selects.
    click(&mut app, "Check again");
    assert!(app.agents.checking);
    click(&mut app, "Antigravity CLI");
    assert_eq!(app.agents.current().unwrap().entry.id, "antigravity");
}

/// Prints two lines and succeeds.
fn fake_installer(command: &str, tx: &mpsc::Sender<JobEvent>) {
    let _ = tx.send(JobEvent::Line(format!("running {command}")));
    let _ = tx.send(JobEvent::Line("added 1 package".into()));
    let _ = tx.send(JobEvent::Done(Ok(())));
}

/// Opens the Agents tab with the fake agents and waits for the check.
fn agents_app(env: &Env) -> App {
    let mut app = env.app(env.code.path());
    app.agent_checker = fake_agents;
    app.installer = fake_installer;
    key(&mut app, KeyCode::Char('8'));
    wait_for_agents(&mut app);
    app
}

fn wait_for_agents(app: &mut App) {
    for _ in 0..200 {
        app.tick();
        if !app.agents.checking && app.install_events.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the agents were not checked");
}

#[test]
fn an_agent_is_updated_after_its_command_is_confirmed() {
    let env = Env::new();
    let mut app = agents_app(&env);
    // Codex is installed but old: «Update» shows the command first.
    app.agents.select(1);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains(" Update "), "{text}");
    key(&mut app, KeyCode::Char('i'));
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("Update Codex CLI?"), "{text}");
    assert!(text.contains("@openai/codex@latest"), "{text}");
    // Escape changes its mind: nothing runs.
    key(&mut app, KeyCode::Esc);
    assert!(app.agents.job.is_none());

    key(&mut app, KeyCode::Char('i'));
    key(&mut app, KeyCode::Enter);
    assert!(app.agents.job.as_ref().unwrap().running());
    wait_for_agents(&mut app);
    let job = app.agents.job.as_ref().unwrap();
    assert_eq!(job.done, Some(Ok(())));
    assert_eq!(job.lines.len(), 2);
    let text = screen_of_width(&mut app, 160);
    for part in ["Codex CLI updated", "added 1 package", "$ npm install -g"] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Afterwards the computer was checked again.
    assert!(app.agents.known && !app.agents.checking);
}

#[test]
fn a_codex_in_a_system_folder_shows_the_terminal_commands() {
    fn system_codex(credentials: Option<&Path>) -> Vec<harness_agents::install::catalog::Status> {
        use harness_agents::install::catalog;
        let find = |name: &str| (name == "codex").then(|| PathBuf::from("/usr/bin/codex"));
        let version = |_: &Path| Ok("codex-cli 0.150.0".to_string());
        catalog::check_with(catalog::CATALOG, &find, &version, credentials)
    }
    if harness_platform::program::npm_user_prefix().is_none() {
        // Only Linux keeps npm's global folder out of the user's reach.
        return;
    }
    let env = Env::new();
    let mut app = env.app(env.code.path());
    app.agent_checker = system_codex;
    key(&mut app, KeyCode::Char('8'));
    wait_for_agents(&mut app);
    app.agents.select(1);
    assert!(app.agents.next_step(false).is_none());
    assert!(app.agents.next_step(true).is_none());
    let text = screen_of_width(&mut app, 200);
    for part in [
        "in a terminal",
        "sudo npm install -g @openai/codex@latest",
        "sudo npm uninstall -g @openai/codex",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
}

#[test]
fn claude_alone_does_not_mark_glm_installed() {
    let env = Env::new();
    let mut app = agents_app(&env);
    let glm = app
        .agents
        .statuses
        .iter()
        .position(|s| s.entry.id == "claude+glm")
        .unwrap();
    app.agents.select(glm);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("○ Claude Code + GLM"), "{text}");
    assert!(
        text.contains("Claude Code is installed, but the harness"),
        "{text}"
    );
    // Nothing to install or update for it yet.
    assert!(app.agents.next_step(false).is_none() && app.agents.next_step(true).is_none());
    // A missing agent without the maker's command for this system has none either.
    let kiro = app
        .agents
        .statuses
        .iter()
        .position(|s| s.entry.id == "kiro")
        .unwrap();
    app.agents.select(kiro);
    assert!(app.agents.next_step(false).is_none() && app.agents.next_step(true).is_none());
    let text = screen_of_width(&mut app, 160);
    assert!(
        text.contains("How to install: see https://kiro.dev"),
        "{text}"
    );
}

#[test]
fn an_installed_agent_is_removed_after_confirming() {
    let env = Env::new();
    let mut app = agents_app(&env);
    // Claude Code is installed: «Remove» deletes its program file.
    app.agents.select(0);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains(" Remove "), "{text}");
    key(&mut app, KeyCode::Delete);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("Remove Claude Code?"), "{text}");
    assert!(text.contains("saved logins stay"), "{text}");
    let (_, form) = app.form.as_ref().unwrap();
    assert!(form.text.contains("/bin/claude"), "{}", form.text);
    assert!(form.text.contains("Remote Control"), "{}", form.text);
    key(&mut app, KeyCode::Enter);
    wait_for_agents(&mut app);
    assert_eq!(app.agents.job.as_ref().unwrap().done, Some(Ok(())));
    assert!(screen_of_width(&mut app, 160).contains("Claude Code removed"));

    // Nothing to remove for an agent that is not installed.
    app.agents.select(2);
    assert!(app.agents.next_step(true).is_none());
}

#[test]
fn sign_in_is_offered_for_installed_agents_the_harness_runs() {
    let env = Env::new();
    let mut app = agents_app(&env);
    // Claude Code is installed: «Sign in» gives the terminal to `harness login claude`.
    app.agents.select(0);
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains(" Sign in "), "{text}");
    assert!(text.contains("No login: press «Sign in»."), "{text}");
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.sign_in.take(), Some(("claude", "Claude Code")));
    // DeepSeek Harness signs in with the DeepSeek key.
    let deepseek = app
        .agents
        .statuses
        .iter()
        .position(|s| s.entry.id == "dsh")
        .unwrap();
    app.agents.select(deepseek);
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.sign_in.take().map(|(login, _)| login), Some("deepseek"));
    // Not installed, or only in the catalog: nothing to sign in to.
    for id in ["antigravity", "copilot"] {
        let index = app
            .agents
            .statuses
            .iter()
            .position(|s| s.entry.id == id)
            .unwrap();
        app.agents.select(index);
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.sign_in, None, "{id}");
    }
}

#[test]
fn roles_offer_only_agents_ready_on_the_agents_tab() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    app.agent_checker = fake_agents;
    key(&mut app, KeyCode::Char('8'));
    wait_for_agents(&mut app);
    click(&mut app, "2 Roles");
    let text = screen_of_width(&mut app, 160);
    // Claude is installed without a login: it stays, as the roles use it, but
    // marked; Codex is installed without a login, Antigravity not at all.
    assert!(text.contains("(•) claude"), "{text}");
    assert!(text.contains("not ready"), "{text}");
    for gone in ["( ) codex ", "( ) dsh", "( ) antigravity"] {
        assert!(!text.contains(gone), "{gone:?} in:\n{text}");
    }

    // Signing in to Claude and Codex makes them ready.
    let credentials = env.home.path().join("credentials");
    harness_agents::install::credentials::save_token(
        &credentials,
        "claude",
        &harness_agents::install::credentials::Secret::new("t"),
    )
    .unwrap();
    fs::create_dir_all(credentials.join("codex")).unwrap();
    fs::write(credentials.join("codex/auth.json"), "{}").unwrap();
    app.finish_sign_in("Claude Code", Ok(()));
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Claude Code: login saved"
    );
    let text = screen_of_width(&mut app, 160);
    assert!(text.contains("( ) codex "), "{text}");
    assert!(!text.contains("not ready"), "{text}");
    assert!(!text.contains("( ) antigravity"), "{text}");
}

#[test]
fn an_agent_without_an_adapter_says_loudly_it_is_not_implemented() {
    let env = Env::new();
    let mut app = agents_app(&env);
    let copilot = app
        .agents
        .statuses
        .iter()
        .position(|s| s.entry.id == "copilot")
        .unwrap();
    app.agents.select(copilot);
    let text = screen_of_width(&mut app, 160);
    for part in [
        "⚠ NOT IMPLEMENTED YET",
        "it has no adapter",
        "ask the developer and it will be implemented",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // Installing it is allowed, but the confirm window says it first.
    key(&mut app, KeyCode::Char('i'));
    let (_, form) = app.form.as_ref().unwrap();
    assert!(
        form.text
            .starts_with("Note: the harness cannot work with this agent yet"),
        "{}",
        form.text
    );
    key(&mut app, KeyCode::Esc);

    // In Russian too.
    key(&mut app, KeyCode::Char('L'));
    let text = screen_of_width(&mut app, 160);
    for part in [
        "⚠ ПОКА НЕ РЕАЛИЗОВАНО",
        "· не реализовано",
        "обратитесь к программисту",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    // An agent the harness runs has none of it.
    app.agents.select(0);
    assert!(!screen_of_width(&mut app, 160).contains("ПОКА НЕ РЕАЛИЗОВАНО"));
}
