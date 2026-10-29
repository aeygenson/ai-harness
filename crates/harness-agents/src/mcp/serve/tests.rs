//! Tests of the read-only MCP server: whole conversations on a fake project.

use std::fs;
use std::io::Cursor;

use harness_core::task::handoff::{Handoff, NextStep, Role, Verdict, SCHEMA_VERSION};
use harness_core::task::scenario::Workflow;
use harness_core::task::store::TaskStore;

use super::*;

/// A project with `task-001`: the architect finished and the design waits for the user.
fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let harness_dir = root.path().join(HARNESS_DIR);
    let runs = harness_dir.join("runs");
    fs::create_dir_all(&runs).unwrap();
    fs::write(harness_dir.join("harness.toml"), "max_rounds = 5\n").unwrap();
    let (store, mut state) = TaskStore::create(
        &runs,
        "task-001",
        "Build a parser\nfor CSV",
        Workflow::Task,
        5,
    )
    .unwrap();
    let handoff = Handoff {
        schema_version: SCHEMA_VERSION,
        task_id: "task-001".into(),
        round: 1,
        role: Role::Architect,
        verdict: Verdict::Approved,
        next_role: NextStep::To(Role::Human),
        summary: "Design is ready".into(),
        skills_used: vec![],
        files: vec![],
        issues: vec![],
        answer_to: None,
    };
    let step_dir = store.record(&mut state, &handoff, "See design.md").unwrap();
    // A terminal escape in the log must not reach the agent.
    store
        .save_log(&step_dir, "thinking\n\u{1b}[2Jwrote design.md\n")
        .unwrap();
    root
}

/// Sends `messages` (one per line) and returns the answers.
fn talk(observer: &Observer, messages: &[Value]) -> Vec<Value> {
    let mut input = String::new();
    for message in messages {
        input.push_str(&message.to_string());
        input.push('\n');
    }
    let mut output = Vec::new();
    observer.serve(Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Calls one tool and returns its answer's `result`.
fn call(observer: &Observer, name: &str, arguments: &Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    talk(observer, &[request]).remove(0)["result"].clone()
}

/// The JSON a tool returned in its text block.
fn json_text(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_and_list_answer_but_notifications_get_nothing() {
    let root = project();
    let observer = Observer::new(root.path());

    let answers = talk(
        &observer,
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-03-26"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ],
    );

    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "harness");
    assert_eq!(answers[1]["id"], 2);
    assert_eq!(answers[1]["result"]["tools"].as_array().unwrap().len(), 10);
}

#[test]
fn an_unknown_version_gets_the_one_the_server_speaks() {
    assert_eq!(
        initialize(&json!({"protocolVersion": "1999-01-01"}))["protocolVersion"],
        PROTOCOL_VERSION
    );
}

#[test]
fn list_tasks_shows_the_stage_and_the_first_line_as_title() {
    let root = project();
    let tasks = json_text(&call(&Observer::new(root.path()), "list_tasks", &json!({})));

    assert_eq!(tasks[0]["id"], "task-001");
    assert_eq!(tasks[0]["title"], "Build a parser");
    assert_eq!(tasks[0]["stage"], "waiting: approve design");
    assert_eq!(tasks[0]["max_rounds"], 5);
}

#[test]
fn task_status_numbers_the_steps() {
    let root = project();
    let status = json_text(&call(
        &Observer::new(root.path()),
        "task_status",
        &json!({"task": "task-001"}),
    ));

    assert_eq!(status["description"], "Build a parser\nfor CSV");
    assert_eq!(status["steps"][0]["step"], 1);
    assert_eq!(status["steps"][0]["role"], "architect");
    assert_eq!(status["steps"][0]["folder"], "round-01/01-architect");
    assert_eq!(status["failed_attempts"], json!([]));
}

#[test]
fn read_step_gives_the_handoff_notes_and_a_clean_log() {
    let root = project();
    let step = json_text(&call(
        &Observer::new(root.path()),
        "read_step",
        &json!({"task": "task-001", "step": 1}),
    ));

    assert_eq!(step["handoff"]["summary"], "Design is ready");
    assert_eq!(step["notes"], "See design.md");
    assert_eq!(step["agent_log"], "thinking\nwrote design.md\n");
    assert_eq!(step["agent_log_cut"], false);
}

#[test]
fn a_missing_step_or_task_is_a_tool_error_with_the_reason() {
    let root = project();
    let observer = Observer::new(root.path());

    let no_step = call(
        &observer,
        "read_step",
        &json!({"task": "task-001", "step": 0}),
    );
    let no_task = call(&observer, "task_status", &json!({"task": "../etc"}));

    assert_eq!(no_step["isError"], true);
    assert!(no_step["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("1 steps"));
    assert_eq!(no_task["isError"], true);
}

#[test]
fn a_wrong_tool_or_argument_is_a_protocol_error() {
    let root = project();
    let observer = Observer::new(root.path());
    let ask = |name: &str, arguments: Value| {
        talk(
            &observer,
            &[json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                     "params": {"name": name, "arguments": arguments}})],
        )
        .remove(0)
    };

    assert_eq!(ask("approve", json!({}))["error"]["code"], INVALID_PARAMS);
    assert_eq!(
        ask("task_status", json!({}))["error"]["code"],
        INVALID_PARAMS
    );
}

#[test]
fn broken_json_and_unknown_methods_get_errors() {
    let root = project();
    let observer = Observer::new(root.path());
    let mut output = Vec::new();
    let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"resources/list\"}\n";

    observer.serve(Cursor::new(input), &mut output).unwrap();

    let answers: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(answers[0]["error"]["code"], PARSE_ERROR);
    assert_eq!(answers[1]["error"]["code"], METHOD_NOT_FOUND);
}

#[test]
fn read_settings_and_retro_stats_read_without_writing() {
    let root = project();
    let observer = Observer::new(root.path());

    let settings = call(&observer, "read_settings", &json!({}));
    let stats = call(&observer, "retro_stats", &json!({"task": "task-001"}));

    assert_eq!(settings["content"][0]["text"], "max_rounds = 5\n");
    assert_eq!(stats["isError"], false);
    assert!(stats["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("task-001"));
    assert!(!root.path().join(HARNESS_DIR).join("retros").exists());
}

#[test]
fn wait_for_change_returns_when_the_time_is_up() {
    let root = project();
    let mut observer = Observer::new(root.path());
    observer.poll = Duration::from_millis(10);

    let answer = json_text(&call(
        &observer,
        "wait_for_change",
        &json!({"task": "task-001", "seconds": 1}),
    ));

    assert_eq!(answer["changed"], false);
    assert_eq!(answer["stage"], "waiting: approve design");
}

#[test]
fn wait_for_change_returns_when_the_state_changes() {
    let root = project();
    let mut observer = Observer::new(root.path());
    observer.poll = Duration::from_millis(10);
    let runs = root.path().join(HARNESS_DIR).join("runs");
    // Another thread plays the user giving more rounds while the agent waits.
    let lisa = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let (store, mut state) = TaskStore::open(&runs, "task-001").unwrap();
        store.add_rounds(&mut state, 5).unwrap();
    });

    let answer = json_text(&call(
        &observer,
        "wait_for_change",
        &json!({"task": "task-001", "seconds": 30}),
    ));
    lisa.join().unwrap();

    assert_eq!(answer["changed"], true);
    assert_eq!(answer["max_rounds"], 10);
}
