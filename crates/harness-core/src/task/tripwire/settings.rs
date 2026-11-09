//! The project's own harness settings, watched while a task works in a
//! folder of its own (a git worktree).
//!
//! The roles' settings (`harness.toml`, skills, MCP servers) are read from
//! the user's project folder, but the agents work in the task's folder,
//! where `git status` cannot see the project folder. An agent that changed
//! those settings could give the next role more rights, so they are watched.

use std::fs;
use std::path::{Path, PathBuf};

use crate::git::{GitError, Repo, HARNESS_DIR};

/// Folders in `.harness/` that hold the harness's own records, not settings:
/// they change as tasks run and are left out.
const NOT_SETTINGS: &[&str] = &["runs", "retros", "agents"];

/// Every settings file in the project folder's `.harness/` when `repo` is a
/// task's own folder; nothing when `repo` is the project folder itself
/// (there `git status` already sees those files).
pub(super) fn project_settings(repo: &Repo) -> Result<Vec<PathBuf>, GitError> {
    let Some(main) = repo.worktrees()?.into_iter().next() else {
        return Ok(Vec::new());
    };
    if same_folder(&main.path, repo.root()) {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let harness = main.path.join(HARNESS_DIR);
    // `harness.toml` is watched even when it is missing: creating it is a change.
    files.push(harness.join("harness.toml"));
    add_files(&harness, true, &mut files);
    files.sort();
    files.dedup();
    Ok(files)
}

/// Adds the files under `dir` to `files`, without following links to
/// folders. At the top (`top`), the folders in [`NOT_SETTINGS`] are skipped.
fn add_files(dir: &Path, top: bool, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let skipped = top
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| NOT_SETTINGS.contains(&name));
        if skipped {
            continue;
        }
        // `symlink_metadata` looks at the link itself, not where it points.
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => add_files(&path, false, files),
            Ok(_) => files.push(path),
            Err(_) => {}
        }
    }
}

/// Do `a` and `b` name the same folder? Git may spell a path differently.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        fs::create_dir_all(root.join(".harness/skills")).unwrap();
        fs::create_dir_all(root.join(".harness/runs/task-001")).unwrap();
        fs::write(root.join(".harness/harness.toml"), "max_rounds = 5\n").unwrap();
        fs::write(root.join(".harness/skills/dev.md"), "be careful\n").unwrap();
        fs::write(root.join(".harness/runs/task-001/state.json"), "{}").unwrap();
        let repo = Repo::init(&root).unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    #[test]
    fn a_task_folder_watches_the_project_folder_s_settings() {
        let (dir, repo) = project();
        let folder = dir.path().join("app.tasks/task-001");
        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();
        let task = Repo::open(&folder).unwrap();

        let files = project_settings(&task).unwrap();

        let names: Vec<String> = files
            .iter()
            .map(|f| {
                // Git spells the folder its own way on Windows (`/`, short names).
                let top = dir.path().canonicalize().unwrap();
                let file = f.canonicalize().unwrap();
                let rest = file.strip_prefix(&top).unwrap().to_path_buf();
                rest.to_string_lossy().replace('\\', "/")
            })
            .collect();
        assert_eq!(
            names,
            ["app/.harness/harness.toml", "app/.harness/skills/dev.md"]
        );
    }

    #[test]
    fn the_project_folder_itself_needs_no_extra_watch() {
        let (_dir, repo) = project();

        assert_eq!(project_settings(&repo).unwrap(), Vec::<PathBuf>::new());
    }
}
