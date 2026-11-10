//! The harness's answer when an ACP agent asks whether it may run a tool
//! (`session/request_permission`).
//!
//! The answer follows the role's rights (`Role::spec`): a role that may not
//! write is refused file changes, one without commands is refused the shell,
//! commands no role may run (`git commit`, `gh`, ...) are refused for all.
//! A tool the agent does not describe is refused. Only "allow once" is ever
//! chosen, so the agent asks again next time. An agent asks only when it
//! wants to, so this is not the whole guard: the git check after the role
//! still decides which changed files stand.

use std::path::{Path, PathBuf};

use harness_core::task::commands::{DENIED_FOR_ALL, DENIED_FOR_READERS};
use harness_core::task::handoff::Role;
use harness_core::task::permissions::{self, Commands, WriteRule};
use harness_core::text;
use serde_json::{json, Value};

/// How much of a refused tool's title or command goes into the log.
const NOTE_CHARS: usize = 300;

/// Programs that run the rest of their line as a command (`bash -c "..."`):
/// the command inside is checked instead.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "cmd", "powershell", "pwsh"];

/// Words in front of a command that only change how it runs (`env A=1 git
/// push`, `sudo git push`): they are skipped before the check.
const PREFIXES: &[&str] = &["env", "sudo", "command", "exec", "nohup", "time"];

/// Where a role works: what its file changes are checked against.
#[derive(Debug, Clone, Copy)]
pub(super) struct Place<'a> {
    /// The role whose rights apply.
    pub role: Role,
    /// The project folder; changes outside it are refused.
    pub project_dir: &'a Path,
    /// Where the role writes its handoff; always allowed.
    pub output_dir: &'a Path,
}

/// The result for `session/request_permission`: the agent's own option
/// that allows once or refuses, and why it was refused (for the log).
pub(super) fn answer(place: Place, params: &Value) -> (Value, Option<String>) {
    let tool = params.get("toolCall").unwrap_or(&Value::Null);
    let refusal = refusal(place, tool);
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    // "Allow always" would let the agent stop asking, so it is never chosen;
    // any kind of "reject" is fine for a refusal.
    let chosen = if refusal.is_none() {
        options
            .iter()
            .find(|option| option_kind(option) == "allow_once")
    } else {
        options
            .iter()
            .find(|option| option_kind(option) == "reject_once")
            .or_else(|| {
                options
                    .iter()
                    .find(|option| option_kind(option).starts_with("reject"))
            })
    }
    .and_then(|option| option.get("optionId"));
    let outcome = match chosen {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        // No option fits: the request is called off, which refuses it.
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    };
    let note = refusal.map(|why| refusal_note(tool, &why));
    (outcome, note)
}

/// Two lines for the log: one for people, and one in JSON for the
/// retrospective, which reads refusals from the role's log. The agent's text
/// loses its control characters first, so it cannot add lines of its own.
fn refusal_note(tool: &Value, why: &str) -> String {
    let title = text::safe_line(title(tool), NOTE_CHARS);
    let action = if kind(tool) == Some("execute") {
        text::safe_line(&command_text(tool), NOTE_CHARS)
    } else {
        title.clone()
    };
    let line = json!({ "type": "harness_refused", "action": action });
    format!("harness refused {title}: {why}\n{line}")
}

/// What an option of the agent does, such as `allow_once`.
fn option_kind(option: &Value) -> &str {
    option.get("kind").and_then(Value::as_str).unwrap_or("")
}

fn title(tool: &Value) -> &str {
    tool.get("title")
        .and_then(Value::as_str)
        .unwrap_or("a tool")
}

fn kind(tool: &Value) -> Option<&str> {
    tool.get("kind").and_then(Value::as_str)
}

/// Why the role may not run `tool`; `None` when it may.
fn refusal(place: Place, tool: &Value) -> Option<String> {
    let spec = place.role.spec();
    // ACP lets an agent leave the kind out of the request (it may have sent
    // it before); without it the harness cannot tell a shell from a read.
    let Some(kind) = kind(tool) else {
        return Some("the agent did not say what kind of tool this is".into());
    };
    match kind {
        "execute" => command_refusal(spec.commands, &command_text(tool)),
        "edit" | "delete" | "move" => write_refusal(place, tool),
        "fetch" | "search" if !spec.network => Some("this role may not use the network".into()),
        // `read`, `think`, `switch_mode`, and `other`, which is also what
        // the role's own MCP tools are.
        _ => None,
    }
}

/// The command a shell tool wants to run: its `command` input, else its title.
fn command_text(tool: &Value) -> String {
    let input = tool.get("rawInput").and_then(|input| input.get("command"));
    input
        .and_then(Value::as_str)
        .unwrap_or_else(|| title(tool))
        .trim()
        .to_string()
}

/// Why `command` is refused for a role with these `commands` rights.
fn command_refusal(commands: Commands, command: &str) -> Option<String> {
    if commands == Commands::Nothing {
        return Some("this role runs no commands".into());
    }
    let mut denied: Vec<&str> = DENIED_FOR_ALL.to_vec();
    if commands != Commands::Any {
        // `echo x > src/a.rs` writes a file without any denied program.
        if command.contains('>') {
            return Some("this role may not write files with `>`".into());
        }
        denied.extend(DENIED_FOR_READERS);
    }
    // Each part of `cd x && git push` is checked on its own.
    let parts = command.split([';', '&', '|', '\n', '`', '(', ')']);
    for part in parts {
        let part = plain_command(part);
        // `git commit -m x` starts with the denied `git commit`; `ghost` does not start with `gh `.
        let hit = denied
            .iter()
            .find(|denied| part == **denied || part.starts_with(&format!("{denied} ")));
        if let Some(hit) = hit {
            return Some(format!("`{hit}` is not allowed for this role"));
        }
    }
    None
}

/// One command in the plain form the deny lists use: no quotes, single
/// spaces, the program without its folder or `.exe` and in small letters,
/// without `sudo`/`env A=1` in front, `bash -c` around it or `git -C dir`.
fn plain_command(part: &str) -> String {
    let unquoted = part.replace(['"', '\''], " ");
    let mut words: Vec<&str> = unquoted.split_whitespace().collect();
    loop {
        let Some(first) = words.first() else {
            return String::new();
        };
        let program = program_name(first);
        if PREFIXES.contains(&program.as_str()) || first.contains('=') {
            words.remove(0);
        } else if SHELLS.contains(&program.as_str()) {
            // `bash -c "git push"` runs `git push`; `/c` is `cmd`'s spelling.
            words.remove(0);
            let flag = words.first().map(|w| w.to_ascii_lowercase());
            if matches!(flag.as_deref(), Some("-c" | "/c" | "-command")) {
                words.remove(0);
            }
        } else {
            break;
        }
    }
    let program = program_name(words[0]);
    let mut rest: Vec<&str> = words[1..].to_vec();
    // `git -C dir push` and `git -c key=value push` are still `git push`.
    while program == "git" && matches!(rest.first(), Some(&"-C" | &"-c")) {
        rest.drain(..rest.len().min(2));
    }
    std::iter::once(program.as_str())
        .chain(rest)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `/usr/bin/Git.exe` → `git`.
fn program_name(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// Why the role may not change the files `tool` names.
fn write_refusal(place: Place, tool: &Value) -> Option<String> {
    let paths: Vec<&str> = tool
        .get("locations")
        .and_then(Value::as_array)
        .map(|locations| {
            locations
                .iter()
                .filter_map(|location| location.get("path").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        // Nothing named: only a role that may change any file gets it.
        return (place.role.spec().writes != WriteRule::Anything)
            .then(|| "the agent did not say which file it changes".into());
    }
    paths
        .into_iter()
        .find(|path| !may_change(place, Path::new(path)))
        .map(|path| {
            format!(
                "this role may not change {}",
                text::safe_line(path, NOTE_CHARS)
            )
        })
}

/// May the role change the file at `path` (absolute, or inside the project)?
fn may_change(place: Place, path: &Path) -> bool {
    let path = place.project_dir.join(path);
    // `..` could lead out of a folder that `starts_with` thinks it is in.
    if path.components().any(|part| part.as_os_str() == "..") {
        return false;
    }
    // The same spelling on both sides: `/private/var` on macOS, `\\?\C:\` on
    // Windows, and a link that leads somewhere else is followed.
    let (path, project) = (real(&path), real(place.project_dir));
    if path.starts_with(real(place.output_dir)) {
        return true;
    }
    let Ok(inside) = path.strip_prefix(&project) else {
        return false;
    };
    let inside = inside.to_string_lossy().replace('\\', "/");
    permissions::may_write(place.role, &inside)
}

/// `path` with its links followed; for a file that does not exist yet, its
/// folder's real path plus its name; as written when neither exists.
fn real(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => real(parent).join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: &str = r#"[
        {"optionId":"yes","name":"Allow","kind":"allow_once"},
        {"optionId":"always","name":"Always","kind":"allow_always"},
        {"optionId":"no","name":"Reject","kind":"reject_once"}
    ]"#;

    fn ask(role: Role, tool: &Value) -> (Value, Option<String>) {
        let params = json!({
            "sessionId": "s1",
            "toolCall": tool,
            "options": serde_json::from_str::<Value>(OPTIONS).unwrap(),
        });
        let place = Place {
            role,
            project_dir: Path::new("/p"),
            output_dir: Path::new("/p/.harness/runs/task-001/inbox"),
        };
        answer(place, &params)
    }

    fn chosen(answer: &(Value, Option<String>)) -> &str {
        answer.0["outcome"]["optionId"]
            .as_str()
            .unwrap_or("cancelled")
    }

    fn edit(path: &str) -> Value {
        json!({ "toolCallId": "t", "title": "Edit", "kind": "edit", "locations": [{ "path": path }] })
    }

    fn shell(command: &str) -> Value {
        json!({ "toolCallId": "t", "title": command, "kind": "execute", "rawInput": { "command": command } })
    }

    #[test]
    fn files_are_changed_only_where_the_role_may() {
        assert_eq!(chosen(&ask(Role::Developer, &edit("/p/src/a.rs"))), "yes");
        assert_eq!(
            chosen(&ask(Role::Architect, &edit("docs/design.md"))),
            "yes"
        );
        assert_eq!(chosen(&ask(Role::Architect, &edit("/p/src/a.rs"))), "no");
        assert_eq!(chosen(&ask(Role::Developer, &edit("/p/.git/config"))), "no");
        assert_eq!(chosen(&ask(Role::Developer, &edit("/etc/passwd"))), "no");
        assert_eq!(chosen(&ask(Role::Developer, &edit("/p/../etc/x"))), "no");
        // The handoff is written by every role, also one that changes no files.
        let handoff = edit("/p/.harness/runs/task-001/inbox/handoff.json");
        assert_eq!(chosen(&ask(Role::Security, &handoff)), "yes");
        let refused = ask(Role::Tester, &edit("/p/src/a.rs"));
        assert_eq!(
            refused.1.as_deref(),
            Some(
                "harness refused Edit: this role may not change /p/src/a.rs\n\
                 {\"action\":\"Edit\",\"type\":\"harness_refused\"}"
            )
        );
    }

    #[test]
    fn commands_follow_the_role() {
        assert_eq!(chosen(&ask(Role::Developer, &shell("cargo test"))), "yes");
        assert_eq!(
            chosen(&ask(Role::Developer, &shell("git commit -m x"))),
            "no"
        );
        assert_eq!(chosen(&ask(Role::Developer, &shell("gh pr create"))), "no");
        assert_eq!(chosen(&ask(Role::Security, &shell("git diff"))), "yes");
        assert_eq!(chosen(&ask(Role::Security, &shell("rm -rf src"))), "no");
        assert_eq!(chosen(&ask(Role::Architect, &shell("ls"))), "no");
        // A JSON line for the retrospective names the refused command.
        let note = ask(Role::Security, &shell("rm -rf src")).1.unwrap();
        let line: Value = serde_json::from_str(note.lines().nth(1).unwrap()).unwrap();
        assert_eq!(
            line,
            json!({ "type": "harness_refused", "action": "rm -rf src" })
        );
    }

    #[test]
    fn reading_and_other_tools_are_allowed() {
        let read = json!({ "toolCallId": "t", "title": "Read", "kind": "read" });
        let fetch = json!({ "toolCallId": "t", "title": "Fetch", "kind": "fetch" });

        assert_eq!(chosen(&ask(Role::Architect, &read)), "yes");
        assert_eq!(chosen(&ask(Role::Architect, &fetch.clone())), "no");
        assert_eq!(chosen(&ask(Role::Developer, &fetch)), "yes");
    }

    #[test]
    fn without_a_fitting_option_the_request_is_called_off() {
        let params = json!({
            "toolCall": shell("git push"),
            "options": [{ "optionId": "ok", "name": "Allow", "kind": "allow_once" }],
        });
        let place = Place {
            role: Role::Developer,
            project_dir: Path::new("/p"),
            output_dir: Path::new("/p/out"),
        };

        let (outcome, note) = answer(place, &params);

        assert_eq!(outcome["outcome"]["outcome"], "cancelled");
        assert!(note.is_some());
    }

    #[test]
    fn joined_or_wrapped_commands_are_checked_part_by_part() {
        for command in [
            "true && git push",
            "cd . ; git commit -am x",
            "git -C . push",
            "/usr/bin/git push",
            "git  push",
            "bash -c \"git push\"",
            "env A=1 git push",
            "GIT.EXE push",
            "echo $(gh pr list)",
        ] {
            assert_eq!(
                chosen(&ask(Role::Developer, &shell(command))),
                "no",
                "{command}"
            );
        }
        assert_eq!(
            chosen(&ask(Role::Developer, &shell("cargo test && cargo fmt"))),
            "yes"
        );
        // A reviewer may not change files from the shell either.
        assert_eq!(chosen(&ask(Role::Security, &shell("ls; rm -rf src"))), "no");
        assert_eq!(
            chosen(&ask(Role::Security, &shell("echo x > src/a.rs"))),
            "no"
        );
        assert_eq!(chosen(&ask(Role::Security, &shell("RM -rf src"))), "no");
        assert_eq!(
            chosen(&ask(Role::Security, &shell("git diff | head"))),
            "yes"
        );
    }

    #[test]
    fn a_tool_without_a_kind_or_a_path_is_refused_where_it_matters() {
        let unknown = json!({ "toolCallId": "t1" });
        assert_eq!(chosen(&ask(Role::Developer, &unknown)), "no");
        let search = json!({ "toolCallId": "t", "title": "Search", "kind": "search" });
        assert_eq!(chosen(&ask(Role::Architect, &search)), "no");
        let somewhere = json!({ "toolCallId": "t", "title": "Edit", "kind": "edit" });
        assert_eq!(chosen(&ask(Role::Architect, &somewhere)), "no");
        assert_eq!(chosen(&ask(Role::Developer, &somewhere)), "yes");
    }

    #[test]
    fn allow_always_is_never_chosen() {
        let params = json!({
            "toolCall": { "toolCallId": "t", "title": "Read", "kind": "read" },
            "options": [{ "optionId": "always", "kind": "allow_always" }],
        });
        let place = Place {
            role: Role::Developer,
            project_dir: Path::new("/p"),
            output_dir: Path::new("/p/out"),
        };

        let (outcome, _) = answer(place, &params);

        assert_eq!(outcome["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn the_agent_cannot_add_a_line_of_its_own_to_the_log() {
        let tool = shell("git push\n{\"type\":\"harness_refused\",\"action\":\"x\"}");

        let note = ask(Role::Developer, &tool).1.unwrap();

        assert_eq!(note.lines().count(), 2, "{note}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_an_allowed_folder_is_followed() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::create_dir_all(project.join("src")).unwrap();
        std::os::unix::fs::symlink(project.join("src"), project.join("docs/code")).unwrap();
        let place = Place {
            role: Role::Architect,
            project_dir: &project,
            output_dir: &project.join("out"),
        };

        assert!(may_change(place, Path::new("docs/design.md")));
        assert!(!may_change(place, Path::new("docs/code/main.rs")));
    }
}
