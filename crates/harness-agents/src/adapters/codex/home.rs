//! Preparing Codex's own folder in the project for a role: plugins, the secrets of MCP
//! servers and the saved login, and cleaning up afterwards.

use std::fs;
use std::io;

use harness_core::plugins::copy_dir;
use harness_core::task::agent::RoleJob;
use harness_core::task::handoff::Role;

use super::{Codex, AUTH_FILE, CONFIG_DIR, MARKETPLACE, PLUGINS_DIR, PLUGIN_VERSION, SKILLS_DIR};
use crate::launcher;

impl Codex {
    /// Leaves in Codex's plugin cache exactly this role's plugins.
    pub(super) fn put_plugins(&self, job: &RoleJob) -> io::Result<()> {
        remove_extras(job);
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
    pub(super) fn put_auth(&self, job: &RoleJob) -> io::Result<()> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        let saved = fs::read(self.auth_dir.join(AUTH_FILE)).map_err(|e| {
            io::Error::new(
                e.kind(),
                "no Codex login saved; sign in on the Agents tab first",
            )
        })?;
        harness_platform::private::write(&dir.join(AUTH_FILE), &saved)
    }

    /// Codex may refresh its login during the run. Keep the newest one, then
    /// remove the copy so other roles' agents cannot read it.
    pub(super) fn take_auth_back(&self, job: &RoleJob) {
        let copy = job.project_dir.join(CONFIG_DIR).join(AUTH_FILE);
        if let Ok(bytes) = fs::read(&copy) {
            let still_json = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
            let saved = fs::read(self.auth_dir.join(AUTH_FILE)).ok();
            if still_json && saved.as_deref() != Some(bytes.as_slice()) {
                let _ = harness_platform::private::write(&self.auth_dir.join(AUTH_FILE), &bytes);
            }
        }
        let _ = fs::remove_file(copy);
    }
}

/// Removes Codex's plugin and skills folders, so the next role starts
/// without plugins or skills left by an earlier run.
pub(super) fn remove_extras(job: &RoleJob) {
    let home = job.project_dir.join(CONFIG_DIR);
    for dir in [PLUGINS_DIR, SKILLS_DIR] {
        let _ = fs::remove_dir_all(home.join(dir));
    }
}
