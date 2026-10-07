//! What the earlier steps of a task really changed, taken from git by the
//! harness itself, for the prompt of the next role.
//!
//! A handoff is written by an agent, so it can be wrong or even made up: a
//! Developer could leave a file out of `files` so the Tester and Security
//! never look at it. The harness commits every role's work itself (an agent
//! may not commit, see [`crate::task::orchestrator`]), so the commit of a step
//! is a fact no agent can fake. The next role gets these facts, and every
//! difference from what the handoff says is pointed out. A difference is not
//! a reason to refuse the work (agents are often a little inaccurate), only
//! something for the next role to look at.

use std::fmt::Write as _;
use std::path::Path;

use crate::git::{Repo, HARNESS_DIR};
use crate::task::handoff::{FileAction, FileChange, Role};
use crate::task::store::Step;
use crate::text;

/// At most this many files are named for one step, so a step that changed
/// thousands of files does not fill the whole prompt.
const MAX_FILES_SHOWN: usize = 50;

/// A file name in the prompt is cut after this many characters.
const MAX_PATH_CHARS: usize = 300;

/// One file a step's commit changed, as git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Changed {
    /// `A` added, `M` changed, `D` deleted (as `git show --name-status`).
    status: char,
    /// The file's path inside the project, with `/`.
    path: String,
}

/// Where a step's handoff and its commit disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Mismatch {
    /// Files the commit changed that the handoff does not list as changed.
    not_listed: Vec<String>,
    /// Files the handoff lists as created, modified or deleted that the
    /// commit did not change.
    not_changed: Vec<String>,
}

/// Compares what a handoff says the role changed with what its commit changed.
/// Only paths are compared: "created" for a changed file is not a mismatch.
fn compare(listed: &[FileChange], changed: &[Changed]) -> Mismatch {
    // A file listed only as "read" counts as not listed if it was changed.
    let listed_changes: Vec<&str> = listed
        .iter()
        .filter(|file| file.action != FileAction::Read)
        .map(|file| file.path.as_str())
        .collect();
    let not_listed = changed
        .iter()
        .filter(|file| !listed_changes.contains(&file.path.as_str()))
        .map(|file| file.path.clone())
        .collect();
    let not_changed = listed_changes
        .iter()
        .filter(|path| !changed.iter().any(|file| file.path == **path))
        .map(|path| (*path).to_string())
        .collect();
    Mismatch {
        not_listed,
        not_changed,
    }
}

/// The project files the commit of `step` changed, without the harness's own
/// records in `.harness/`. `None` if the step has no commit yet.
fn changed_by(repo: &Repo, step: &Step) -> Option<Vec<Changed>> {
    let handoff = step.dir.join("handoff.json");
    let relative = handoff.strip_prefix(repo.root()).unwrap_or(&handoff);
    let relative = relative.to_str()?.replace('\\', "/");
    let files = repo.files_of_commit_adding(&relative)?;
    Some(
        files
            .into_iter()
            .filter(|(_, path)| !Path::new(path).starts_with(HARNESS_DIR))
            .map(|(status, path)| Changed { status, path })
            .collect(),
    )
}

/// The "facts from the harness" part of the prompt: for every AI step so
/// far, the files its commit changed and where its handoff disagrees.
/// Empty when there is no such step yet.
pub fn text(repo: &Repo, steps: &[Step]) -> String {
    let mut text = String::new();
    // Lisa's decisions change no project files, so they are left out.
    for step in steps.iter().filter(|step| step.handoff.role != Role::Human) {
        let handoff = &step.handoff;
        let _ = write!(text, "- round {}, {}: ", handoff.round, handoff.role);
        let Some(changed) = changed_by(repo, step) else {
            text.push_str("no commit found\n");
            continue;
        };
        text.push_str(&changed_list(&changed));
        let mismatch = compare(&handoff.files, &changed);
        if !mismatch.not_listed.is_empty() {
            let _ = writeln!(
                text,
                "  Its handoff does not mention these changed files: {}",
                paths(&mismatch.not_listed)
            );
        }
        if !mismatch.not_changed.is_empty() {
            let _ = writeln!(
                text,
                "  Its handoff lists these files, but the step did not change them: {}",
                paths(&mismatch.not_changed)
            );
        }
    }
    if text.is_empty() {
        return text;
    }
    format!(
        "Facts from the harness (taken from git; no agent wrote them). \
         Files each earlier step really changed:\n{text}\n"
    )
}

/// `created a.txt, modified b.txt\n`, or `changed no project files\n`.
fn changed_list(changed: &[Changed]) -> String {
    if changed.is_empty() {
        return "changed no project files\n".to_string();
    }
    let names: Vec<String> = changed
        .iter()
        .take(MAX_FILES_SHOWN)
        .map(|file| {
            let how = match file.status {
                'A' => "created",
                'D' => "deleted",
                _ => "modified",
            };
            format!("{how} {}", text::safe_line(&file.path, MAX_PATH_CHARS))
        })
        .collect();
    format!("{}{}\n", names.join(", "), more(changed.len()))
}

/// The paths joined with commas, at most [`MAX_FILES_SHOWN`] of them.
fn paths(list: &[String]) -> String {
    let names: Vec<String> = list
        .iter()
        .take(MAX_FILES_SHOWN)
        .map(|path| text::safe_line(path, MAX_PATH_CHARS))
        .collect();
    format!("{}{}", names.join(", "), more(list.len()))
}

/// `, and 7 more` when `count` is over [`MAX_FILES_SHOWN`].
fn more(count: usize) -> String {
    if count > MAX_FILES_SHOWN {
        format!(", and {} more", count - MAX_FILES_SHOWN)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, action: FileAction) -> FileChange {
        FileChange {
            path: path.to_string(),
            action,
        }
    }

    fn changed(status: char, path: &str) -> Changed {
        Changed {
            status,
            path: path.to_string(),
        }
    }

    #[test]
    fn a_handoff_that_matches_its_commit_has_no_mismatch() {
        let files = [
            listed("a.txt", FileAction::Created),
            listed("b.txt", FileAction::Modified),
            listed("c.txt", FileAction::Read),
        ];
        // "created" for a modified file is not a mismatch: only paths count.
        let commit = [changed('M', "a.txt"), changed('M', "b.txt")];

        assert_eq!(compare(&files, &commit), Mismatch::default());
    }

    #[test]
    fn files_left_out_or_made_up_are_found() {
        let files = [
            listed("a.txt", FileAction::Modified),
            listed("ghost.txt", FileAction::Created),
            listed("secret.txt", FileAction::Read),
        ];
        let commit = [changed('M', "a.txt"), changed('M', "secret.txt")];

        let mismatch = compare(&files, &commit);

        assert_eq!(mismatch.not_listed, ["secret.txt"]);
        assert_eq!(mismatch.not_changed, ["ghost.txt"]);
    }

    #[test]
    fn a_long_list_of_changes_is_cut() {
        let commit: Vec<Changed> = (0..MAX_FILES_SHOWN + 3)
            .map(|i| changed('A', &format!("f{i}.txt")))
            .collect();

        let list = changed_list(&commit);

        assert!(list.starts_with("created f0.txt, "));
        assert!(list.ends_with(", and 3 more\n"));
    }

    #[test]
    fn file_names_cannot_bring_control_characters_into_the_prompt() {
        let list = changed_list(&[changed('D', "evil\u{1b}[2J\nname.txt")]);

        assert!(!list.trim_end().contains('\n'));
        assert!(!list.contains('\u{1b}'));
    }

    /// Writes `text` into `path` inside the repository, making folders.
    fn write(repo: &Repo, path: &str, text: &str) {
        let path = repo.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A committed step of `role` whose handoff lists `files`.
    fn committed_step(repo: &Repo, folder: &str, role: Role, files: &[FileChange]) -> Step {
        let dir = repo
            .root()
            .join(HARNESS_DIR)
            .join("runs/task-001/round-01")
            .join(folder);
        let handoff = crate::task::handoff::Handoff {
            schema_version: 1,
            task_id: "task-001".to_string(),
            round: 1,
            role,
            verdict: crate::task::handoff::Verdict::Approved,
            next_role: crate::task::handoff::NextStep::Done,
            summary: "Done.".to_string(),
            skills_used: vec![],
            files: files.to_vec(),
            issues: vec![],
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("handoff.json"), handoff.to_json().unwrap()).unwrap();
        repo.commit_all(folder).unwrap();
        Step {
            dir,
            handoff,
            notes: String::new(),
        }
    }

    #[test]
    fn the_facts_show_what_each_step_really_changed_and_what_its_handoff_hid() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        write(&repo, "README.md", "app\n");
        repo.commit_all("first").unwrap();
        // The Developer changes two files but lists only one, plus one it never made.
        write(&repo, "src/app.txt", "code");
        write(&repo, "src/backdoor.txt", "oops");
        let developer = committed_step(
            &repo,
            "01-developer",
            Role::Developer,
            &[
                listed("src/app.txt", FileAction::Created),
                listed("tests/app_test.txt", FileAction::Created),
            ],
        );
        let lisa = committed_step(&repo, "02-human", Role::Human, &[]);
        let not_committed = Step {
            dir: repo.root().join("missing"),
            ..developer.clone()
        };

        let facts = text(&repo, &[developer, lisa, not_committed]);

        assert_eq!(
            facts,
            "Facts from the harness (taken from git; no agent wrote them). \
             Files each earlier step really changed:\n\
             - round 1, developer: created src/app.txt, created src/backdoor.txt\n\
             \x20 Its handoff does not mention these changed files: src/backdoor.txt\n\
             \x20 Its handoff lists these files, but the step did not change them: \
             tests/app_test.txt\n\
             - round 1, developer: no commit found\n\n"
        );
    }

    #[test]
    fn there_are_no_facts_before_the_first_step() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();

        assert_eq!(text(&repo, &[]), "");
    }
}
