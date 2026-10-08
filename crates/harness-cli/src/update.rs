//! `harness update`: replaces this harness with the newest release from
//! GitHub; `harness update --check` only says whether there is one. The TUI's
//! «Update Harness» button does the same (see `harness_agents::install::update`).

use anyhow::{bail, Context, Result};
use harness_agents::install::update::{self, Releases};

/// Checks for a newer release and, unless `check_only`, installs it in the
/// place of this program.
pub(crate) fn update(check_only: bool) -> Result<()> {
    let releases = Releases::default();
    if check_only {
        let newer = update::newer_version(&releases).map_err(anyhow::Error::msg)?;
        println!("{}", check_text(&releases.current, newer.as_deref()));
        return Ok(());
    }
    if releases.from_source() {
        bail!("{}", update::FROM_SOURCE);
    }
    let program = std::env::current_exe().context("cannot find the harness's own file")?;
    let updated = update::update(&releases, &program, |line| println!("{line}"))
        .map_err(anyhow::Error::msg)?;
    if updated.is_some() {
        println!("Start the harness again to use the new version.");
    }
    Ok(())
}

/// The answer of `harness update --check`.
fn check_text(current: &str, newer: Option<&str>) -> String {
    match newer {
        Some(newer) => {
            format!("harness {current}; {newer} is out: `harness update` installs it.")
        }
        None => format!("harness {current} is the newest version."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_says_whether_a_newer_version_is_out() {
        assert_eq!(
            check_text("0.4.0", Some("0.5.0")),
            "harness 0.4.0; 0.5.0 is out: `harness update` installs it."
        );
        assert_eq!(
            check_text("0.5.0", None),
            "harness 0.5.0 is the newest version."
        );
    }
}
