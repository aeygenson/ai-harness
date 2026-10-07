//! Preparing Codex's own folder in the project for a role: plugins, the secrets of MCP
//! servers and the saved login, and cleaning up afterwards.

use std::fs;
use std::io;

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

    /// Codex may refresh its login during the run: keep the newest one.
    /// `clean_up` then removes the copy, so other roles' agents cannot read it.
    fn take_auth_back(&self, job: &RoleJob) {
        let copy = job.project_dir.join(CONFIG_DIR).join(AUTH_FILE);
        if let Ok(bytes) = fs::read(&copy) {
            let still_json = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
            let saved = fs::read(self.auth_dir.join(AUTH_FILE)).ok();
            if still_json && saved.as_deref() != Some(bytes.as_slice()) {
                let _ = harness_platform::private::write(&self.auth_dir.join(AUTH_FILE), &bytes);
            }
        }
    }

    /// After the role (or a failed start): keeps a refreshed login, then
    /// removes Codex's whole folder, so nothing reaches the next role.
    pub(super) fn clean_up(&self, job: &RoleJob) {
        self.take_auth_back(job);
        let _ = agent_home::remove(&job.project_dir, CONFIG_DIR);
    }
}
