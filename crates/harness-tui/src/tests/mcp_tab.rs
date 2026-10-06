//! Tests of the MCP tab: servers for roles, the form, check, catalog search and web sign-in.

use super::*;

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
        "▶ [ ] context7",
        "✗ secret",
        "[ ] fetch",
        "[ ] ghost",
        "not described",
        "Command: npx -y @upstash/context7-mcp",
        "CONTEXT7_API_KEY = secret:context7  ✗ not saved: harness secret set",
        "claude ✓",
        "antigravity ✓",
        "Roles: developer",
        " Give to the architect ",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }

    // A saved secret shows up; its value never does.
    harness_agents::install::credentials::save_secret(
        &env.home.path().join("credentials"),
        "context7",
        &harness_agents::install::credentials::Secret::new("ctx-secret-value"),
    )
    .unwrap();
    app.mcp.as_mut().unwrap().reload();
    let text = screen(&mut app);
    assert!(text.contains("secret:context7  ✓ saved"), "{text}");
    assert!(!text.contains("ctx-secret-value"), "{text}");
    assert!(!text.contains("✗ secret"), "{text}");

    // The developer has context7; fetch is given with a double click.
    click(&mut app, " developer ");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] context7"), "{text}");
    assert!(text.contains(" Take from the developer "), "{text}");
    click(&mut app, "[ ] fetch");
    click(&mut app, "[ ] fetch");
    let text = screen(&mut app);
    assert!(text.contains("▶ [x] fetch"), "{text}");
    assert!(text.contains("Changes not saved"), "{text}");
    // The Roles tab shows the same change.
    click(&mut app, "2 Roles");
    click(&mut app, "developer");
    assert!(screen(&mut app).contains("[x] fetch"));
    click(&mut app, "4 MCP");
    // Saving checks everything as before a run: the tester's unknown server
    // stops it.
    click(&mut app, " Save ");
    let (message, problem) = shown(&app);
    assert!(problem && message.contains("ghost"), "{message}");

    // A server harness.toml does not describe cannot be given ...
    click(&mut app, " architect ");
    click(&mut app, "[ ] ghost");
    let text = screen(&mut app);
    assert!(text.contains("harness.toml has no [mcp.ghost]"), "{text}");
    key(&mut app, KeyCode::Char(' '));
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("take it from the role"));
    // ... only taken away; then no role lists it and it leaves the list.
    click(&mut app, " tester ");
    click(&mut app, "[x] ghost");
    key(&mut app, KeyCode::Char(' '));
    assert!(!screen(&mut app).contains("ghost"));
    key(&mut app, KeyCode::Char('s'));
    let saved = config(&root);
    assert_eq!(saved.roles[&Role::Developer].mcp, ["context7", "fetch"]);
    assert!(saved.roles[&Role::Tester].mcp.is_empty());
}

#[test]
fn mcp_servers_are_added_changed_and_removed_and_secrets_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "4 MCP");
    assert!(screen(&mut app).contains("No MCP servers yet."));

    click(&mut app, " New server ");
    fill(&mut app, "docs");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "npx -y docs-mcp");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "API_KEY=secret:docs");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    let docs = &config(&root).mcp["docs"];
    assert_eq!(
        (docs.command.as_deref(), docs.args.as_slice()),
        (Some("npx"), &["-y".to_string(), "docs-mcp".to_string()][..])
    );
    assert_eq!(docs.env["API_KEY"], "secret:docs");
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] docs"), "{text}");
    assert!(text.contains("MCP server docs saved"), "{text}");

    // A wrong variable name is refused in the form, nothing is written.
    key(&mut app, KeyCode::Char('n'));
    fill(&mut app, "bad");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "npx bad");
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "api_key=1");
    key(&mut app, KeyCode::Enter);
    assert!(app.form.as_ref().unwrap().1.error.is_some());
    key(&mut app, KeyCode::Esc);
    assert!(!config(&root).mcp.contains_key("bad"));

    // The developer gets it; a rename follows in its list.
    click(&mut app, " developer ");
    key(&mut app, KeyCode::Char(' '));
    // Servers change only when the roles have no unsaved changes.
    key(&mut app, KeyCode::Char('e'));
    key(&mut app, KeyCode::Enter);
    assert!(app
        .form
        .as_ref()
        .unwrap()
        .1
        .error
        .as_ref()
        .unwrap()
        .contains("Save or undo"));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.form.as_ref().unwrap().1.value(1), "npx -y docs-mcp");
    fill(&mut app, "manuals");
    key(&mut app, KeyCode::Enter);
    let saved = config(&root);
    assert!(!saved.mcp.contains_key("docs"));
    assert_eq!(saved.roles[&Role::Developer].mcp, ["manuals"]);
    assert!(screen(&mut app).contains("▶ [x] manuals"));

    // The secret: typed in a hidden field, saved in a private file only.
    assert!(screen(&mut app).contains("✗ secret"));
    click(&mut app, " Set secret ");
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "docs");
    type_text(&mut app, "s3cr3t-value");
    let text = screen(&mut app);
    assert!(text.contains("••••••••••••"), "{text}");
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(!format!("{:?}", app.form).contains("s3cr3t"));
    key(&mut app, KeyCode::Enter);
    let file = env.home.path().join("credentials/secrets/docs");
    assert_eq!(fs::read_to_string(&file).unwrap().trim(), "s3cr3t-value");
    let text = screen(&mut app);
    assert!(text.contains("secret:docs  ✓ saved"), "{text}");
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(!fs::read_to_string(root.join(".harness/harness.toml"))
        .unwrap()
        .contains("s3cr3t"));

    // Removing asks first, then takes it from the roles too.
    key(&mut app, KeyCode::Delete);
    assert!(screen(&mut app).contains("Remove [mcp.manuals]"));
    key(&mut app, KeyCode::Enter);
    let saved = config(&root);
    assert!(saved.mcp.is_empty());
    assert!(saved.roles[&Role::Developer].mcp.is_empty());
    assert!(file.is_file());
}

fn fake_check(
    server: &harness_core::mcp::McpServer,
    _: &Path,
) -> Result<Vec<harness_core::mcp::tools::Tool>, String> {
    // The server gets its secret, as a run would give it.
    if server.env["API_KEY"].expose() != "docs-key" {
        return Err("wrong key".into());
    }
    Ok(vec![
        harness_core::mcp::tools::Tool {
            name: "search".into(),
            description: Some("Searches the docs.".into()),
        },
        harness_core::mcp::tools::Tool {
            name: "fetch".into(),
            description: None,
        },
    ])
}

#[test]
fn check_asks_a_server_for_its_tools() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.docs]\ncommand = \"npx\"\nargs = [\"docs-mcp\"]\nenv = { API_KEY = \"secret:docs\" }\n\n\
           [mcp.other]\ncommand = \"npx\"\nenv = { KEY = \"secret:other\" }\n";
    fs::write(&path, text).unwrap();
    harness_agents::install::credentials::save_secret(
        &env.home.path().join("credentials"),
        "docs",
        &harness_agents::install::credentials::Secret::new("docs-key"),
    )
    .unwrap();
    let mut app = env.app(&root);
    app.checker = fake_check;
    click(&mut app, "4 MCP");
    assert!(screen(&mut app).contains("Tools: not checked yet."));

    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("docs: 2 tools"), "{text}");
    assert!(text.contains("Tools (2):"), "{text}");
    assert!(text.contains("search  Searches the docs."), "{text}");
    assert!(!text.contains("docs-key"), "{text}");

    // A server whose secret is not saved is not started.
    click(&mut app, "[ ] other");
    click(&mut app, " Check ");
    assert!(app.checking.is_none());
    let (message, problem) = shown(&app);
    assert!(
        problem && message.contains("harness secret set other"),
        "{message}"
    );
}

fn fake_search(query: &str) -> Result<Vec<harness_core::mcp::registry::Entry>, String> {
    if query != "docs" {
        return Err("offline".into());
    }
    harness_core::mcp::registry::parse(
        r#"{"servers":[
        {"server":{"name":"io.github.someone/docs","title":"Docs","description":"Finds docs.",
          "version":"1.2.0","repository":{"url":"https://github.com/someone/docs"},
          "packages":[{"registryType":"npm","identifier":"docs-mcp","version":"1.2.0",
            "transport":{"type":"stdio"},
            "environmentVariables":[{"name":"DOCS_KEY","description":"Your key.","isSecret":true}]}]}},
        {"server":{"name":"com.example/remote-docs","version":"0.1.0",
          "remotes":[{"type":"sse","url":"https://example.com/sse"}]}},
        {"server":{"name":"com.example/web-docs","version":"0.2.0",
          "remotes":[{"type":"streamable-http","url":"https://example.com/web",
            "headers":[{"name":"Authorization","value":"Bearer {key}","isSecret":true}]}]}}
        ]}"#,
    )
}

/// Waits until the registry search running in the background has answered.
fn wait_search(app: &mut App) {
    let start = Instant::now();
    while app.searching.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
}

#[test]
fn a_server_from_the_catalog_opens_in_the_form_before_it_is_saved() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    app.searcher = fake_search;
    click(&mut app, "4 MCP");

    // A failed search says why.
    click(&mut app, " From catalog ");
    fill(&mut app, "anything");
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    assert!(screen(&mut app).contains("the search failed: offline"));

    key(&mut app, KeyCode::Char('/'));
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "anything");
    fill(&mut app, "docs");
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    let text = screen(&mut app);
    assert!(text.contains("3 servers for «docs»"), "{text}");
    assert!(text.contains("Finds docs."), "{text}");
    assert!(text.contains("npx -y docs-mcp@1.2.0"), "{text}");
    assert!(text.contains("DOCS_KEY = secret:docs"), "{text}");
    assert!(text.contains("cannot add"), "{text}");

    // A server with only an old SSE address cannot be added.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none());
    let (message, problem) = shown(&app);
    assert!(problem && message.contains("Cannot be added"), "{message}");

    // The first opens filled in; nothing is written before Save.
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    let form = &app.form.as_ref().unwrap().1;
    assert_eq!(
        (form.value(0), form.value(1), form.value(2)),
        ("docs", "npx -y docs-mcp@1.2.0", "DOCS_KEY=secret:docs")
    );
    assert!(config(&root).mcp.is_empty());
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    assert_eq!(config(&root).mcp["docs"].args, ["-y", "docs-mcp@1.2.0"]);
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] docs"), "{text}");
    assert!(text.contains("✗ secret"), "{text}");

    // Added again, it gets a name of its own; Esc goes back to the list.
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Enter);
    wait_search(&mut app);
    assert!(screen(&mut app).contains("already added"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.form.as_ref().unwrap().1.value(0), "docs-2");
    key(&mut app, KeyCode::Esc);

    // A server on the web: its address and headers, the key as a secret.
    key(&mut app, KeyCode::Down);
    let text = screen(&mut app);
    assert!(text.contains("Address: https://example.com/web"), "{text}");
    assert!(
        text.contains("Authorization = Bearer secret:web-docs"),
        "{text}"
    );
    key(&mut app, KeyCode::Enter);
    let form = &app.form.as_ref().unwrap().1;
    assert_eq!(
        (form.value(0), form.value(1), form.value(2), form.value(3)),
        (
            "web-docs",
            "https://example.com/web",
            "\"Authorization=Bearer secret:web-docs\"",
            ""
        )
    );
    key(&mut app, KeyCode::Enter);
    let web = &config(&root).mcp["web-docs"];
    assert_eq!(web.url.as_deref(), Some("https://example.com/web"));
    assert_eq!(web.headers["Authorization"], "Bearer secret:web-docs");
    assert!(!app.quit);
    let text = screen(&mut app);
    assert!(text.contains("▶ [ ] web-docs"), "{text}");
    assert!(text.contains("Address: https://example.com/web"), "{text}");
}

#[test]
fn a_web_server_is_written_and_changed_in_the_form() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = env.app(&root);
    click(&mut app, "4 MCP");
    click(&mut app, " New server ");
    fill(&mut app, "notion");
    key(&mut app, KeyCode::Tab);
    // The fields of a new server start empty: typing alone is enough.
    assert_eq!(app.form.as_ref().unwrap().1.value(1), "");
    type_text(&mut app, "https://mcp.notion.com/mcp");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    // The sign-in is a box: Space ticks it, typed letters do nothing.
    type_text(&mut app, "yes");
    assert!(!app.form.as_ref().unwrap().1.is_checked(3));
    key(&mut app, KeyCode::Char(' '));
    assert!(screen(&mut app).contains("[x]"));
    key(&mut app, KeyCode::Enter);
    assert!(app.form.is_none(), "{:?}", app.form);
    let notion = &config(&root).mcp["notion"];
    assert_eq!(notion.url.as_deref(), Some("https://mcp.notion.com/mcp"));
    assert_eq!(notion.auth, Some(harness_core::config::McpAuth::OAuth));
    assert!(screen(&mut app).contains(" Sign in "));

    // Changed back to a key in a header, in the same form.
    key(&mut app, KeyCode::Char('e'));
    assert!(app.form.as_ref().unwrap().1.is_checked(3));
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Tab);
    fill(&mut app, "Authorization=\"Bearer secret:notion\"");
    // A click on the box takes the tick away.
    click(&mut app, "[x]");
    assert!(!app.form.as_ref().unwrap().1.is_checked(3));
    key(&mut app, KeyCode::Enter);
    let notion = &config(&root).mcp["notion"];
    assert!(notion.auth.is_none());
    assert_eq!(notion.headers["Authorization"], "Bearer secret:notion");
    let text = fs::read_to_string(root.join(".harness/harness.toml")).unwrap();
    assert!(!text.contains("auth"), "{text}");
}

fn fake_web_check(
    server: &harness_core::mcp::McpServer,
    _: &Path,
) -> Result<Vec<harness_core::mcp::tools::Tool>, String> {
    // The web server is reached through this program, with its header.
    assert_eq!(server.args, ["mcp-remote"]);
    assert_eq!(Path::new(&server.command), std::env::current_exe().unwrap());
    assert_eq!(
        server.env["HARNESS_MCP_HEADER_1"].expose(),
        "Authorization: Bearer web-key"
    );
    Ok(vec![harness_core::mcp::tools::Tool {
        name: "ask".into(),
        description: None,
    }])
}

#[test]
fn a_web_server_shows_its_address_and_is_checked_through_the_bridge() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.wiki]\nurl = \"https://mcp.example.com/mcp\"\n\
           headers = { Authorization = \"Bearer secret:wiki\" }\n";
    fs::write(&path, text).unwrap();
    let mut app = env.app(&root);
    app.checker = fake_web_check;
    click(&mut app, "4 MCP");
    let text = screen(&mut app);
    assert!(
        text.contains("Address: https://mcp.example.com/mcp"),
        "{text}"
    );
    assert!(text.contains("Authorization: Bearer secret:wiki"), "{text}");
    assert!(text.contains("✗ secret"), "{text}");
    assert!(!text.contains("Command:"), "{text}");

    harness_agents::install::credentials::save_secret(
        &env.home.path().join("credentials"),
        "wiki",
        &harness_agents::install::credentials::Secret::new("web-key"),
    )
    .unwrap();
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("wiki: 1 tools"), "{text}");
    assert!(!text.contains("web-key"), "{text}");
}

fn fake_sign_in(dir: &Path, name: &str, url: &str) -> Result<(), String> {
    // As a real sign-in would leave it.
    let file = harness_agents::mcp::oauth::path(dir, name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let saved = serde_json::json!({
        "url": url, "token_endpoint": "https://auth.example.com/token",
        "client_id": "c", "access_token": "oauth-token-1",
    });
    fs::write(&file, saved.to_string()).unwrap();
    Ok(())
}

#[test]
fn a_web_server_with_a_sign_in_is_signed_in_from_the_tab() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let path = root.join(".harness/harness.toml");
    let text = fs::read_to_string(&path).unwrap()
        + "\n[mcp.notion]\nurl = \"https://mcp.example.com/mcp\"\nauth = \"oauth\"\n";
    fs::write(&path, text).unwrap();
    let mut app = env.app(&root);
    app.signer = fake_sign_in;
    app.checker = |server, _| {
        assert_eq!(
            server.env["HARNESS_MCP_HEADER_1"].expose(),
            "Authorization: Bearer oauth-token-1"
        );
        Ok(Vec::new())
    };
    click(&mut app, "4 MCP");
    let text = screen(&mut app);
    assert!(text.contains("✗ sign-in"), "{text}");
    assert!(text.contains("Sign-in: ✗ not signed in"), "{text}");
    // Without a sign-in, «Check» says what to do.
    key(&mut app, KeyCode::Char('c'));
    let (message, problem) = shown(&app);
    assert!(
        problem && message.contains("harness mcp login notion"),
        "{message}"
    );

    key(&mut app, KeyCode::Char('i'));
    let start = Instant::now();
    while app.signing.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    let text = screen(&mut app);
    assert!(text.contains("Signed in to notion"), "{text}");
    assert!(text.contains("Sign-in: ✓ signed in"), "{text}");
    assert!(!text.contains("oauth-token-1"), "{text}");

    key(&mut app, KeyCode::Char('c'));
    let start = Instant::now();
    while app.checking.is_some() {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        app.tick();
    }
    assert!(screen(&mut app).contains("notion: 0 tools"));
}
