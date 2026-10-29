//! Keeping the work of a failed attempt before it is thrown away: its changes
//! are saved as a patch next to its log, so the user can put them back with
//! `git apply`. A hung agent that was stopped by hand otherwise lost hours of
//! work at the retry.

use std::path::PathBuf;

use crate::text;

use super::changes::RoleAt;
use super::{oversized_changes, RunError, MAX_CHANGE_BYTES};

/// Saves the attempt's changes outside `.harness/` as a patch with the name of
/// its log (the last of `failure_logs`) and adds the patch to `failure_logs`,
/// so it is committed with them. Returns what to tell the user, starting with
/// `; `, or nothing when there was nothing to save.
///
/// Never stops the run: a patch that cannot be made, or one larger than
/// [`MAX_CHANGE_BYTES`], is only reported, and the attempt is thrown away as
/// before.
pub(super) fn save_failed_work(at: RoleAt, failure_logs: &mut Vec<PathBuf>) -> String {
    match try_save(at, failure_logs) {
        Ok(note) => note,
        Err(e) => format!(
            "; its changes could not be saved as a patch ({})",
            text::safe_line(&e.to_string(), 300)
        ),
    }
}

/// [`save_failed_work`] with its errors.
fn try_save(at: RoleAt, failure_logs: &mut Vec<PathBuf>) -> Result<String, RunError> {
    let Some(log) = failure_logs.last().cloned() else {
        return Ok(String::new());
    };
    let files: Vec<String> = at
        .repo
        .changed_files()?
        .into_iter()
        .filter(|path| !failure_logs.contains(&at.repo.root().join(path)))
        .collect();
    // The files are measured first, so a huge build folder is never read
    // into memory; the patch itself is measured too, because deleted files
    // weigh nothing on disk but their whole old text is in the patch.
    let too_large = "; its changes were too large to keep as a patch".to_string();
    if !oversized_changes(at.repo.root(), &files, MAX_CHANGE_BYTES).is_empty() {
        return Ok(too_large);
    }
    let patch = at.repo.changes_as_patch()?;
    if patch.is_empty() {
        return Ok(String::new());
    }
    if u64::try_from(patch.len()).unwrap_or(u64::MAX) > MAX_CHANGE_BYTES {
        return Ok(too_large);
    }
    let saved = at.store.save_failure_patch(&log, &patch)?;
    let relative = saved.strip_prefix(at.repo.root()).unwrap_or(&saved);
    let note = format!(
        "; its changes are saved in {} (`git apply` puts them back)",
        relative.display()
    );
    failure_logs.push(saved);
    Ok(note)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::git::Repo;
    use crate::task::handoff::Role;
    use crate::task::orchestrator::changes::{after_failure, AfterFailure};
    use crate::task::scenario::Workflow;
    use crate::task::store::TaskStore;
    use crate::task::TaskState;

    /// A repository with one commit and a task in it.
    fn setup() -> (tempfile::TempDir, Repo, TaskStore, TaskState) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        fs::create_dir_all(repo.runs_dir()).unwrap();
        let (store, state) =
            TaskStore::create(&repo.runs_dir(), "task-001", "Do it.", Workflow::Task, 5).unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo, store, state)
    }

    #[test]
    fn a_thrown_away_attempt_leaves_a_patch_that_puts_its_work_back() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        let log = store
            .save_failure_log(state.round, Role::Developer, "hung")
            .unwrap();
        let mut failure_logs = vec![log];
        let mut problem = "the agent failed".to_string();
        let at = RoleAt::new(&repo, &store, &state, Role::Developer);

        let after = after_failure(at, false, &mut failure_logs, &mut problem).unwrap();

        assert!(matches!(after, AfterFailure::Retry(None)));
        let patch = store.dir().join("failures/round-01-developer-1.patch");
        assert_eq!(failure_logs.last(), Some(&patch), "committed with the logs");
        assert!(problem.contains("round-01-developer-1.patch"));
        // The folder is clean again, apart from the harness's own records.
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!dir.path().join("new.txt").exists());
        let applied = std::process::Command::new("git")
            .current_dir(dir.path())
            .arg("apply")
            .arg(&patch)
            .status()
            .unwrap();
        assert!(applied.success());
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn a_patch_that_cannot_be_saved_does_not_stop_the_run() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let log = store
            .save_failure_log(state.round, Role::Developer, "boom")
            .unwrap();
        // A patch left over from before: history is never overwritten.
        fs::write(log.with_extension("patch"), "old").unwrap();
        let mut failure_logs = vec![log];
        let mut problem = String::new();
        let at = RoleAt::new(&repo, &store, &state, Role::Developer);

        let after = after_failure(at, false, &mut failure_logs, &mut problem).unwrap();

        assert!(matches!(after, AfterFailure::Retry(None)));
        assert!(problem.contains("could not be saved as a patch"));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
    }

    #[test]
    fn nothing_is_saved_when_the_attempt_changed_nothing() {
        let (_dir, repo, store, state) = setup();
        let log = store
            .save_failure_log(state.round, Role::Developer, "boom")
            .unwrap();
        let mut failure_logs = vec![log];
        let at = RoleAt::new(&repo, &store, &state, Role::Developer);

        let note = save_failed_work(at, &mut failure_logs);

        assert_eq!(note, "");
        assert_eq!(failure_logs.len(), 1);
    }

    #[test]
    fn a_change_too_large_to_commit_is_not_saved() {
        let (dir, repo, store, state) = setup();
        // A sparse file: big by its size, but it takes no time to write.
        let big = fs::File::create(dir.path().join("big.bin")).unwrap();
        big.set_len(MAX_CHANGE_BYTES + 1).unwrap();
        let log = store
            .save_failure_log(state.round, Role::Developer, "boom")
            .unwrap();
        let mut failure_logs = vec![log];
        let at = RoleAt::new(&repo, &store, &state, Role::Developer);

        let note = save_failed_work(at, &mut failure_logs);

        assert!(note.contains("too large"));
        assert_eq!(failure_logs.len(), 1);
    }
}
