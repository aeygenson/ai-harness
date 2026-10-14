//! Preparing Codex's own folder in the project for a role: plugins, the secrets of MCP
//! servers and the saved login, and cleaning up afterwards. Also keeping a login
//! Codex refreshed with a copy, which `install::models` uses too.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use harness_core::plugins::copy_dir;
use harness_core::task::agent::RoleJob;
use harness_core::task::handoff::Role;

use super::{Codex, AUTH_FILE, CONFIG_DIR, MARKETPLACE, PLUGINS_DIR, PLUGIN_VERSION};
use crate::{agent_home, launcher};

impl Codex {
    /// Puts this role's plugins into Codex's plugin cache (the folder is empty
    /// at this point, see `agent_home::fresh`).
    pub(super) fn put_plugins(&self, job: &RoleJob) -> io::Result<()> {
        let cache = job
            .project_dir
            .join(CONFIG_DIR)
            .join(PLUGINS_DIR)
            .join("cache")
            .join(MARKETPLACE);
        for plugin in self.settings.plugins(job.role) {
            copy_dir(&plugin.path, &cache.join(&plugin.name).join(PLUGIN_VERSION))?;
        }
        Ok(())
    }

    /// A private temporary folder outside the project with the role's secrets:
    /// one file per MCP server. Deleted when dropped.
    pub(super) fn write_secrets(&self, role: Role) -> io::Result<tempfile::TempDir> {
        let dir = tempfile::Builder::new()
            .prefix("harness-codex-")
            .tempdir()?;
        for server in self.settings.servers(role) {
            launcher::write_server(dir.path(), server)?;
        }
        Ok(dir)
    }

    /// Copies the saved login into the project's config folder for this run.
    /// Returns what was copied, so `clean_up` can tell whether Codex changed it.
    pub(super) fn put_auth(&self, job: &RoleJob) -> io::Result<Vec<u8>> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        let saved = fs::read(self.auth_dir.join(AUTH_FILE)).map_err(|e| {
            io::Error::new(
                e.kind(),
                "no Codex login saved; sign in on the Agents tab first",
            )
        })?;
        harness_platform::private::write(&dir.join(AUTH_FILE), &saved)?;
        Ok(saved)
    }

    /// After the role (or a failed start): keeps a login Codex refreshed from
    /// `given` (`None` when no login was copied), then removes Codex's whole
    /// folder, so nothing reaches the next role.
    ///
    /// # Errors
    ///
    /// The refreshed login could not be saved; the folder is removed anyway.
    pub(super) fn clean_up(&self, job: &RoleJob, given: Option<&[u8]>) -> io::Result<()> {
        let copy = job.project_dir.join(CONFIG_DIR).join(AUTH_FILE);
        let kept = given.map_or(Ok(()), |given| {
            keep_refreshed_login(&copy, given, &self.auth_dir.join(AUTH_FILE))
        });
        let _ = agent_home::remove(&job.project_dir, CONFIG_DIR);
        kept
    }
}

/// Saves the login in `copy` over `saved` when Codex refreshed it from `given`.
///
/// Codex refreshes an expired login by itself, and the server accepts each
/// refresh token only once. So a refreshed copy must be kept: throwing it
/// away leaves `saved` with a used token, and the next run fails with
/// "refresh token already used" until Lisa signs in again.
///
/// Nothing is written when the copy is unchanged, is not a login (cut off,
/// broken, a link, too big, or without a refresh token or API key), or when
/// `saved` no longer holds `given`: then someone else changed it meanwhile
/// (another run refreshed it, or Lisa signed in again) and theirs is newer.
/// The new login goes into a private file next to `saved` that is then
/// renamed over it, so a crash never leaves `saved` half written.
///
/// Known limits: a stopped run never gets here; after a time-out the copy is
/// read as Codex left it. Two runs that refresh the same login at the same
/// time still spend one refresh token: there is no lock between them.
///
/// # Errors
///
/// The refreshed login could not be written; the saved one has a used token.
pub(crate) fn keep_refreshed_login(copy: &Path, given: &[u8], saved: &Path) -> io::Result<()> {
    let Some(bytes) = read_login(copy) else {
        return Ok(());
    };
    let untouched = || fs::read(saved).is_ok_and(|now| now == given);
    if bytes == given || !untouched() {
        return Ok(());
    }
    let folder = saved
        .parent()
        .ok_or_else(|| io::Error::other("the saved Codex login has no folder"))?;
    // A new private file of its own (mode 600, a unique name, so two savers
    // never share it), flushed to disk, then renamed over the saved login.
    // If anything fails, dropping it removes the file.
    let mut new = tempfile::NamedTempFile::new_in(folder)?;
    new.write_all(&bytes)?;
    new.as_file().sync_all()?;
    // Checked again right before the rename: a sign-in may have just landed.
    if !untouched() {
        return Ok(());
    }
    match new.persist(saved) {
        Ok(_file) => Ok(()),
        // On Windows the rename fails while another program (an antivirus)
        // has the file open; writing into it in place still keeps the token.
        Err(failed) => {
            harness_platform::private::write(saved, &bytes).map_err(|_also_failed| failed.error)
        }
    }
}

/// The biggest login file read back: a real one is a few kilobytes.
const MAX_LOGIN_BYTES: u64 = 1024 * 1024;

/// Reads the copy Codex used, if it is still a login. The copy sits in the
/// project, where an agent with a shell could replace it, so a link (to
/// `/dev/zero` or a pipe) or a huge file is not read.
fn read_login(copy: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(copy).ok()?;
    if !meta.is_file() || meta.len() > MAX_LOGIN_BYTES {
        return None;
    }
    let bytes = fs::read(copy).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let filled = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    (filled("/tokens/refresh_token") || filled("/OPENAI_API_KEY")).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Writes `saved` and `copy` (when given) into a new folder.
    fn logins(saved: &str, copy: Option<&str>) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (copy_path, saved_path) = (dir.path().join("copy.json"), dir.path().join("auth.json"));
        fs::write(&saved_path, saved).unwrap();
        if let Some(copy) = copy {
            fs::write(&copy_path, copy).unwrap();
        }
        (dir, copy_path, saved_path)
    }

    const ONE: &str = r#"{"tokens":{"refresh_token":"one"}}"#;
    const TWO: &str = r#"{"tokens":{"refresh_token":"two"}}"#;
    const THREE: &str = r#"{"tokens":{"refresh_token":"three"}}"#;

    #[test]
    fn a_refreshed_login_is_kept() {
        let (dir, copy, saved) = logins(ONE, Some(TWO));

        keep_refreshed_login(&copy, ONE.as_bytes(), &saved).unwrap();

        assert_eq!(fs::read_to_string(&saved).unwrap(), TWO);
        assert!(harness_platform::private::is_private(&saved).unwrap());
        let files = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(
            files, 2,
            "only the copy and the saved login, no leftover file"
        );
    }

    #[test]
    fn a_missing_broken_or_empty_copy_is_not_kept() {
        for copy in [None, Some(r#"{"token":"#), Some("{}"), Some("null")] {
            let (_dir, copy_path, saved) = logins(r#"{"token":1}"#, copy);

            keep_refreshed_login(&copy_path, br#"{"token":1}"#, &saved).unwrap();

            assert_eq!(
                fs::read_to_string(&saved).unwrap(),
                r#"{"token":1}"#,
                "{copy:?}"
            );
        }
    }

    #[test]
    fn a_login_changed_by_someone_else_meanwhile_is_not_overwritten() {
        // Another run refreshed the login (one to three) while this copy, given
        // one, was unchanged or refreshed itself: the saved login stays three.
        for copy in [ONE, TWO] {
            let (_dir, copy_path, saved) = logins(THREE, Some(copy));

            keep_refreshed_login(&copy_path, ONE.as_bytes(), &saved).unwrap();

            assert_eq!(fs::read_to_string(&saved).unwrap(), THREE, "{copy}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_copy_is_not_followed() {
        let (dir, copy, saved) = logins(ONE, None);
        let target = dir.path().join("other.json");
        fs::write(&target, TWO).unwrap();
        std::os::unix::fs::symlink(&target, &copy).unwrap();

        keep_refreshed_login(&copy, ONE.as_bytes(), &saved).unwrap();

        assert_eq!(fs::read_to_string(&saved).unwrap(), ONE);
    }
}
