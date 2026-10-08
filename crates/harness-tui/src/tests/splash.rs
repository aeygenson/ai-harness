//! Tests of the start window: version, first steps, closing it, updating from it.

use super::*;
use harness_agents::install::update::VERSION;

/// The TUI with the start window open and release `newer` known.
fn start(env: &Env, newer: Option<&str>) -> App {
    let mut app = env.app(env.code.path());
    app.agents.release.from_source = false;
    app.agents.release.newer = Some(Ok(newer.map(str::to_string)));
    app.splash.open = true;
    app
}

#[test]
fn the_start_window_shows_the_version_and_first_steps() {
    let env = Env::new();
    let mut app = start(&env, None);

    let text = screen(&mut app);

    for part in [
        format!("AI Harness {VERSION}"),
        format!("Version {VERSION} · the newest"),
        "First steps".to_string(),
        "1. Agents (8)".to_string(),
        "H this window".to_string(),
        " Start ".to_string(),
    ] {
        assert!(text.contains(&part), "missing {part:?} in:\n{text}");
    }
    assert!(!text.contains("↑ Update to"), "{text}");
}

#[test]
fn any_key_closes_the_start_window_and_question_mark_opens_it_again() {
    let env = Env::new();
    let mut app = start(&env, None);
    let tab = app.tab;

    // The key only closes the window: it does not also switch the tab.
    key(&mut app, KeyCode::Char('8'));
    assert!(!app.splash.open);
    assert_eq!(app.tab, tab);
    assert!(!screen(&mut app).contains("First steps"));

    key(&mut app, KeyCode::Char('H'));
    assert!(app.splash.open);
    // A click anywhere closes it too.
    click(&mut app, "First steps");
    assert!(!app.splash.open);
}

#[test]
fn a_newer_version_can_be_installed_from_the_start_window() {
    let env = Env::new();
    let mut app = start(&env, Some("9.9.9"));
    let text = screen(&mut app);
    assert!(text.contains("9.9.9 is out"), "{text}");

    click(&mut app, "↑ Update to 9.9.9");

    assert!(!app.splash.open);
    assert_eq!(app.tab, Tab::Agents);
    let (_, form) = app.form.as_ref().unwrap();
    assert!(
        form.title.contains("Update AI Harness to 9.9.9?"),
        "{}",
        form.title
    );
}

#[test]
fn u_in_the_start_window_asks_to_update() {
    let env = Env::new();
    let mut app = start(&env, Some("9.9.9"));

    key(&mut app, KeyCode::Char('u'));

    assert!(!app.splash.open);
    assert!(app.form.is_some());
}

#[test]
fn the_box_keeps_the_window_closed_at_the_next_start() {
    let env = Env::new();
    let mut app = start(&env, None);
    assert!(screen(&mut app).contains("[x] Show at start"));

    // Space ticks the box off and leaves the window open.
    key(&mut app, KeyCode::Char(' '));
    assert!(app.splash.open && !app.splash.at_start);
    assert!(screen(&mut app).contains("[ ] Show at start"));
    let next = env.app(env.code.path());
    assert!(!next.splash.at_start);

    // A click on it ticks it on again.
    click(&mut app, "[ ] Show at start");
    assert!(app.splash.open && app.splash.at_start);
    assert!(env.app(env.code.path()).splash.at_start);
}
