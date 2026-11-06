//! The locks that decide how many TUI windows may be open: one main window
//! per user, and one window pinned to each project.
//!
//! Two windows on one project (the main window and the one pinned to it)
//! are allowed. Starting roles is still one at a time per project, guarded
//! by the run lock (`harness_core::task::run_lock`). Settings saved in both
//! windows at the same moment are not guarded: the one saved last wins.

use std::path::Path;

use anyhow::{bail, Context, Result};
use harness_core::config::projects::name_of;
use harness_core::git::Repo;
use harness_platform::lock::{self, Held};

use crate::tabs::projects::has_config;

/// The lock file in `~/.harness` that the open main window holds.
const WINDOW_LOCK: &str = "tui.lock";

/// The lock file a pinned window holds, in the project's `.git/harness/`
/// (next to the run lock): git never shows it as a change, and it stays
/// when a branch is switched.
const PINNED_LOCK: &str = "harness/tui.lock";

/// Stops with a clear error when another main window is already open.
///
/// The main window sees and switches every project, so one per user is
/// enough. Without a home folder there is nothing to share and nothing is
/// locked.
pub(crate) fn hold_the_window(harness_home: Option<&Path>) -> Result<Option<Held>> {
    let Some(home) = harness_home else {
        return Ok(None);
    };
    let path = home.join(WINDOW_LOCK);
    match lock::try_hold(&path) {
        Ok(Some(held)) => Ok(Some(held)),
        Ok(None) => bail!(
            "the harness is already open in another window; switch to that window, \
             or close it there with `q` and start `harness tui` again"
        ),
        Err(error) => Err(error).with_context(|| format!("cannot lock {}", path.display())),
    }
}

/// Stops with a clear error when `root` is not a harness project, or when
/// another window is already pinned to it.
///
/// A pinned window does not take the main window's lock, so the main window
/// and one pinned window per project can be open at once. Two pinned windows
/// on one project would only get in each other's way, and starting roles is
/// guarded by the project's run lock anyway.
pub(crate) fn hold_the_project(root: &Path) -> Result<Held> {
    if !has_config(root) {
        bail!(
            "{} is not a harness project; run `harness init` there first, \
             or start `harness tui --pin` in a project's folder",
            root.display()
        );
    }
    let repo = Repo::open(root)?;
    let path = repo.git_path(PINNED_LOCK)?;
    match lock::try_hold(&path) {
        Ok(Some(held)) => Ok(held),
        Ok(None) => bail!(
            "the harness is already pinned to {} in another window; switch to that \
             window, or close it there with `q` and start `harness tui --pin` again",
            name_of(root)
        ),
        Err(error) => Err(error).with_context(|| format!("cannot lock {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_window_is_refused_while_the_first_is_open() {
        let home = tempfile::tempdir().unwrap();

        let first = hold_the_window(Some(home.path())).unwrap();
        let second = hold_the_window(Some(home.path()));

        assert!(first.is_some());
        let error = second.unwrap_err().to_string();
        assert!(error.contains("already open"), "{error}");
    }

    #[test]
    fn without_a_home_folder_nothing_is_locked() {
        assert!(hold_the_window(None).unwrap().is_none());
    }

    #[test]
    fn a_second_pinned_window_on_one_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("alpha");
        harness_core::config::projects::init(&root).unwrap();

        let first = hold_the_project(&root).unwrap();
        let error = hold_the_project(&root).unwrap_err().to_string();

        assert!(error.contains("already pinned to alpha"), "{error}");
        drop(first);
        hold_the_project(&root).expect("a closed window lets go");
    }

    #[test]
    fn pinned_windows_on_two_projects_and_the_main_window_open_together() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let (alpha, beta) = (dir.path().join("alpha"), dir.path().join("beta"));
        harness_core::config::projects::init(&alpha).unwrap();
        harness_core::config::projects::init(&beta).unwrap();

        let main = hold_the_window(Some(&home)).unwrap();
        let pinned_alpha = hold_the_project(&alpha);
        let pinned_beta = hold_the_project(&beta);

        assert!(main.is_some());
        pinned_alpha.expect("pinned to alpha");
        pinned_beta.expect("pinned to beta");
        // The main window stays one per user.
        hold_the_window(Some(&home)).unwrap_err();
    }

    #[test]
    fn a_folder_without_harness_settings_is_not_pinned() {
        let dir = tempfile::tempdir().unwrap();

        let error = hold_the_project(dir.path()).unwrap_err().to_string();

        assert!(error.contains("harness init"), "{error}");
    }
}
