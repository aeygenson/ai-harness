//! The harness's own version: is a newer release out, and «Update Harness»,
//! which installs it the way `harness update` does. Shown in the «Computer»
//! panel and, when there is a newer version, as a button in the top bar.

use std::sync::mpsc::Sender;

use harness_agents::install::update::{self, Releases};

use super::tab::JobEvent;

/// The name the update job and its messages show.
pub const HARNESS_NAME: &str = "AI Harness";

/// What is known about the harness's own releases.
#[derive(Debug, Default)]
pub struct HarnessRelease {
    /// Built from the source: this harness does not replace itself.
    pub from_source: bool,
    /// The last check: the newer version (`None`: this is the newest), or why
    /// it failed. `None` until a check has answered.
    pub newer: Option<Result<Option<String>, String>>,
    /// The version «Update Harness» installed; it runs after a restart.
    pub installed: Option<String>,
}

impl HarnessRelease {
    /// What `Releases::default()` says about this program.
    pub fn of_this_program() -> Self {
        Self {
            from_source: Releases::default().from_source(),
            ..Self::default()
        }
    }

    /// The newer version «Update Harness» would install, if there is one
    /// and this harness can replace itself.
    pub fn available(&self) -> Option<&str> {
        if self.from_source || self.installed.is_some() {
            return None;
        }
        match &self.newer {
            Some(Ok(Some(version))) => Some(version),
            _ => None,
        }
    }
}

/// Asks whether a newer release is out; tests give a fake one.
pub type ReleaseChecker = fn() -> Result<Option<String>, String>;

/// The real check: asks GitHub.
pub fn check_release() -> Result<Option<String>, String> {
    update::newer_version(&Releases::default())
}

/// Installs the newest release in the place of this program; tests give a fake one.
pub type Updater = fn(&Sender<JobEvent>);

/// The real update, with each step sent as a line of the job.
pub fn update_harness(tx: &Sender<JobEvent>) {
    let result = std::env::current_exe()
        .map_err(|e| format!("cannot find the harness's own file: {e}"))
        .and_then(|program| {
            update::update(&Releases::default(), &program, |line| {
                let _ = tx.send(JobEvent::Line(line));
            })
        });
    let _ = tx.send(JobEvent::Done(result.map(|_| ())));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_is_offered_only_for_a_newer_release_build() {
        let mut release = HarnessRelease {
            newer: Some(Ok(Some("0.5.0".into()))),
            ..HarnessRelease::default()
        };
        assert_eq!(release.available(), Some("0.5.0"));

        release.installed = Some("0.5.0".into());
        assert_eq!(release.available(), None);

        release.installed = None;
        release.from_source = true;
        assert_eq!(release.available(), None);

        release.from_source = false;
        release.newer = Some(Ok(None));
        assert_eq!(release.available(), None);
        release.newer = Some(Err("offline".into()));
        assert_eq!(release.available(), None);
    }
}
