//! Tests of a window pinned to one project (`harness tui --pin`).

use super::*;

/// The TUI started in `root` and pinned to it, as `run` does.
fn pinned_app(env: &Env, root: &Path) -> App {
    env.window(root, Window::Pinned)
}

#[test]
fn a_pinned_window_shows_its_project_without_the_projects_tab() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = pinned_app(&env, &root);

    let text = screen(&mut app);

    assert!(text.contains(" ◆ test · pinned "), "{text}");
    assert!(text.contains("6 Retro │ 7 Agents"), "{text}");
    assert!(!text.contains("Projects"), "{text}");
    assert!(!text.contains("New project"), "{text}");
    assert!(text.contains("1–7 tabs"), "{text}");
}

#[test]
fn the_help_window_of_a_pinned_window_has_its_own_steps_and_keys() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = pinned_app(&env, &root);

    key(&mut app, KeyCode::Char('H'));

    let text = screen_of_size(&mut app, 200, 50);
    assert!(text.contains("1. Agents (7)"), "{text}");
    assert!(!text.contains("Projects (7)"), "{text}");
    assert!(text.contains("1–7 tabs · ↑↓ select · Enter open"), "{text}");
}

#[test]
fn number_keys_and_clicks_follow_the_shorter_tab_list() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = pinned_app(&env, &root);

    key(&mut app, KeyCode::Char('7'));
    assert_eq!(app.tab, Tab::Agents);
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('8'));
    assert_eq!(app.tab, Tab::Tasks, "there is no eighth tab");

    click(&mut app, "7 Agents");
    assert_eq!(app.tab, Tab::Agents);
}

#[test]
fn a_pinned_window_opens_no_other_project() {
    let env = Env::new();
    let (root, other) = (env.path("test"), env.path("other"));
    project(&root);
    project(&other);
    let mut app = pinned_app(&env, &root);

    app.open(&other);

    assert_eq!(app.project.as_deref(), Some(root.as_path()));
    let (message, problem) = shown(&app);
    assert!(problem, "{message}");
    assert!(message.contains("pinned to one project"), "{message}");
}

#[test]
fn a_pinned_window_does_not_change_the_main_windows_start_project() {
    let env = Env::new();
    let (main, pinned) = (env.path("main"), env.path("pinned"));
    project(&main);
    project(&pinned);
    drop(env.app(&main));

    drop(pinned_app(&env, &pinned));

    let saved = env.saved();
    assert_eq!(saved.last.as_deref(), Some(main.as_path()));
    assert_eq!(
        saved.projects.len(),
        2,
        "the main window lists the pinned project"
    );
}

#[test]
fn a_pinned_window_never_falls_back_to_another_project() {
    let env = Env::new();
    let (last, plain) = (env.path("last"), env.path("plain"));
    project(&last);
    drop(env.app(&last));
    fs::create_dir_all(&plain).unwrap();

    let mut app = pinned_app(&env, &plain);

    assert_eq!(app.project, None);
    assert_eq!(app.tab, Tab::Tasks);
    app.press(ButtonId::OpenFolder);
    assert!(app.browser.is_none(), "no folder picker");
    let (message, problem) = shown(&app);
    assert!(
        problem && message.contains("pinned to one project"),
        "{message}"
    );
}

#[test]
fn a_pinned_window_in_russian_says_so() {
    let env = Env::new();
    let root = env.path("test");
    project(&root);
    let mut app = pinned_app(&env, &root);

    key(&mut app, KeyCode::Char('L'));

    let text = screen(&mut app);
    assert!(text.contains(" ◆ test · закреплён "), "{text}");
}
