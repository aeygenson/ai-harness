//! Tests of work another window (or `harness run` in a terminal) does: the
//! Projects tab shows it, the Tasks tab does not start a second run and its
//! stop buttons ask the other one to stop.

use harness_core::task::run_lock::RunLock;
use harness_core::task::stop_request::{StopRequest, StopWhen};

use super::*;

/// What another window holds while it runs roles of `task`.
fn other_run(root: &Path, task: &str) -> (RunLock, StopRequest) {
    let repo = Repo::open(root).unwrap();
    let mut lock = RunLock::hold(&repo).unwrap();
    lock.set_task(task).unwrap();
    let stop = StopRequest::new();
    lock.connect(&stop).unwrap();
    (lock, stop)
}

#[test]
fn the_tasks_tab_shows_the_run_of_another_window_and_starts_nothing() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let (lock, _stop) = other_run(&root, "task-007");

    key(&mut app, KeyCode::Char('r'));
    let text = screen(&mut app);

    assert!(text.contains("task-007 runs in another window"), "{text}");
    assert!(text.contains("Stop now (x)"), "{text}");
    // The other run ends: the tab can send again.
    drop(lock);
    key(&mut app, KeyCode::Char('r'));
    let text = screen(&mut app);
    assert!(!text.contains("another window"), "{text}");
    assert!(text.contains(" Send "), "{text}");
}

#[test]
fn the_stop_buttons_ask_the_other_window_to_stop() {
    let env = Env::new();
    let (root, mut app) = empty_project(&env);
    let (_lock, stop) = other_run(&root, "task-007");
    key(&mut app, KeyCode::Char('r'));

    key(&mut app, KeyCode::Char('s'));
    assert_eq!(stop.asked(), Some(StopWhen::AfterStep));
    assert!(shown(&app)
        .0
        .contains("Asked the other window to stop task-007"));
    key(&mut app, KeyCode::Char('s'));
    assert!(
        shown(&app).0.contains("cannot be taken back"),
        "{:?}",
        shown(&app)
    );
    assert_eq!(stop.asked(), Some(StopWhen::AfterStep));

    key(&mut app, KeyCode::Char('x'));
    assert!(screen(&mut app).contains("Stop the agent now?"));
    key(&mut app, KeyCode::Enter);

    assert_eq!(stop.asked(), Some(StopWhen::Now));
    let text = screen(&mut app);
    assert!(!text.contains("Stop now (x)"), "asked once: {text}");
}

#[test]
fn the_projects_tab_shows_running_and_waiting_tasks_of_every_project() {
    let env = Env::new();
    let (busy, other) = (env.path("busy"), env.path("other"));
    project(&busy);
    project(&other);
    // A design waits for the user's approval in «busy».
    let repo = Repo::open(&busy).unwrap();
    let (store, mut state) =
        orchestrator::create_task(&repo, "task-003", "Waits", Workflow::Task, 5).unwrap();
    let mut design = handoff(
        Role::Architect,
        Verdict::Approved,
        NextStep::To(Role::Human),
        "Plan",
    );
    design.task_id = "task-003".into();
    store.record(&mut state, &design, "notes").unwrap();
    drop(env.app(&other));
    let mut app = env.app(&busy);
    let (lock, _stop) = other_run(&other, "task-002");

    key(&mut app, KeyCode::Char('7'));
    let text = screen_of_width(&mut app, 160);

    assert!(text.contains("other  ▶ task-002"), "{text}");
    assert!(text.contains("busy  waits for you: 1"), "{text}");
    assert!(text.contains("Running now: task-002"), "{text}");
    // The other window has finished: the list says so after a refresh.
    drop(lock);
    key(&mut app, KeyCode::Char('r'));
    assert!(!screen_of_width(&mut app, 160).contains("▶ task-002"));
}
