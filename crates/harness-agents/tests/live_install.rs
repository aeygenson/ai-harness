//! The Agents tab's real commands, on the real system: install an agent with
//! the maker's command, find it, update it, remove it, and see it gone. It
//! downloads, so it is ignored by default; CI runs it on Linux, macOS and
//! Windows with `cargo test -p harness-agents --test live_install -- --ignored`.

use harness_agents::install::catalog::{self, Action, Status};

fn status(id: &str) -> Status {
    catalog::check_all(None)
        .into_iter()
        .find(|s| s.entry.id == id)
        .unwrap()
}

fn run(what: &str, command: &str) {
    println!("$ {command}");
    let result = catalog::run_command(command, |line| println!("  {line}"));
    assert_eq!(result, Ok(()), "{what} failed: {command}");
}

/// What the tab's buttons do, one after another.
fn install_update_remove(id: &str) {
    let first = status(id);
    if first.installed() {
        println!("{id} is already installed here; not touching it");
        return;
    }
    let (action, command) = first.action().unwrap();
    assert_eq!(action, Action::Install);
    run("install", &command.unwrap());

    let installed = status(id);
    assert!(
        installed.installed(),
        "{id} not found after install: {installed:?}"
    );
    println!("found {:?} version {:?}", installed.path, installed.version);
    assert!(installed.version.is_some(), "{installed:?}");

    let (action, command) = installed.action().unwrap();
    assert_eq!(action, Action::Update);
    run(
        "update",
        &command.expect("an update command for this system"),
    );
    assert!(status(id).installed());

    let removal = status(id)
        .removal()
        .expect("a remove command for this system");
    run("remove", &removal);
    let gone = status(id);
    assert!(!gone.installed(), "{id} still found after remove: {gone:?}");
}

#[test]
#[ignore = "downloads OpenCode from npm"]
fn opencode_from_npm() {
    install_update_remove("opencode");
}

#[test]
#[ignore = "downloads Claude Code with its official installer"]
fn claude_code_with_its_installer() {
    install_update_remove("claude");
}
