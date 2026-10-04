//! The real `harness` program with a fake `claude` on every system CI runs
//! on (Linux, macOS, Windows): `harness agents` finds it, and the hidden
//! `harness login claude` that the Agents tab's «Sign in» runs saves the
//! token in the harness's own folder.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A folder with a fake `claude`: it prints a version and, for
/// `setup-token`, a token, like the real one does at the end.
fn fake_claude(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    if cfg!(windows) {
        fs::write(
            bin.join("claude.cmd"),
            "@echo off\r\n\
             if \"%1\"==\"--version\" echo 2.1.300 (Claude Code)\r\n\
             if \"%1\"==\"setup-token\" echo Your token: sk-ant-oat01-test\r\n",
        )
        .unwrap();
    } else {
        let file = bin.join("claude");
        fs::write(
            &file,
            "#!/bin/sh\n\
             case \"$1\" in\n\
             --version) echo '2.1.300 (Claude Code)' ;;\n\
             setup-token) echo 'Your token: sk-ant-oat01-test' ;;\n\
             esac\n",
        )
        .unwrap();
        make_executable(&file);
    }
    bin
}

#[cfg(unix)]
fn make_executable(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn make_executable(_: &Path) {}

/// `harness <args>` with `home` as the home folder and `bin` first in PATH.
fn harness(home: &Path, bin: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    command
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.to_path_buf()];
    dirs.extend(std::env::split_paths(&path));
    command.env("PATH", std::env::join_paths(dirs).unwrap());
    command
}

#[test]
fn agents_lists_the_fake_claude_without_a_login() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let bin = fake_claude(dir.path());
    let output = harness(&home, &bin, &["agents"]).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Claude Code"), "{text}");
    assert!(text.contains("version 2.1.300"), "{text}");
    assert!(
        text.contains("no login: sign in on the Agents tab"),
        "{text}"
    );
}

#[test]
fn login_claude_runs_setup_token_and_saves_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let bin = fake_claude(dir.path());
    let mut child = harness(&home, &bin, &["login", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-ant-oat01-test\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}\n{errors}");
    // The fake's own output came through: setup-token really ran.
    assert!(text.contains("Your token: sk-ant-oat01-test"), "{text}");
    let token = home.join(".harness/credentials/claude/oauth-token");
    assert_eq!(
        fs::read_to_string(&token).unwrap().trim(),
        "sk-ant-oat01-test"
    );

    // Now `harness agents` sees the login.
    let output = harness(&home, &bin, &["agents"]).output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("login saved"), "{text}");
}
