//! What happens when a form is sent (Enter or its OK button) or closed.

use harness_core::plugin_ops;
use harness_core::projects;

use crate::{App, Purpose};

impl App {
    /// OK in the open form.
    pub(crate) fn submit(&mut self) {
        let Some((purpose, form)) = self.form.take() else {
            return;
        };
        let result = match &purpose {
            Purpose::NewProject(path) => self.create_project(&path.clone(), &form),
            Purpose::InitFolder(path) => projects::init(path)
                .map(|_| self.open(path))
                .map_err(|e| e.to_string()),
            Purpose::Model => {
                if let Some(roles) = &mut self.roles {
                    roles.set_model(form.value(0));
                }
                Ok(())
            }
            Purpose::NewSkill => self.create_skill(&form),
            Purpose::RestoreSkill(name) => self.restore_skill(&name.clone()),
            Purpose::McpServer(old) => self.save_mcp(old.clone(), &form),
            Purpose::RemoveMcp(name) => self.remove_mcp(&name.clone()),
            Purpose::Secret => self.save_secret(&form),
            Purpose::McpSearch => {
                self.search_registry(form.value(0));
                Ok(())
            }
            Purpose::RemovePlugin(name) => self.remove_plugin(&name.clone()),
            Purpose::AllowPlugin {
                name,
                hooks,
                servers,
                give,
            } => {
                let (name, give) = (name.clone(), *give);
                self.allow_plugin(&name, *hooks, *servers).map(|()| {
                    if let Some(role) = give {
                        self.give_plugin(&name, role);
                    }
                })
            }
            Purpose::PluginSearch => {
                if let Some(view) = self.plugins.as_mut().and_then(|p| p.catalog.as_mut()) {
                    view.query = form.value(0).trim().to_string();
                    view.row = 0;
                }
                Ok(())
            }
            Purpose::AddCatalog => {
                let source = form.value(0).trim().to_string();
                if source.is_empty() {
                    Err(self.tr.t("plugins.catalog_empty_field").to_string())
                } else {
                    self.start_catalog_add(source);
                    Ok(())
                }
            }
            Purpose::RemoveCatalog(name) => {
                let name = name.clone();
                self.home
                    .clone()
                    .ok_or_else(|| "HOME is not set".to_string())
                    .and_then(|home| {
                        plugin_ops::remove_catalog(&home, &name).map_err(|e| e.to_string())
                    })
                    .map(|()| {
                        self.reload_catalog_views();
                        self.message = Some((
                            self.tr.f("plugins.catalog_removed", &[("name", &name)]),
                            false,
                        ));
                    })
            }
            Purpose::ApplyUpdate(prepared) => {
                self.apply_plugin_update(prepared);
                Ok(())
            }
            Purpose::GenerateRetro => {
                self.generate_retro();
                Ok(())
            }
            Purpose::ApplyProposals(dir, ids) => self.apply_proposals(dir, ids),
            Purpose::RunAgentCommand {
                name,
                action,
                command,
            } => {
                self.run_agent_command(name, *action, command.clone());
                Ok(())
            }
            Purpose::Remove(path) => {
                let path = path.clone();
                let result = self.projects.update(|list| list.remove(&path));
                if self.project.as_deref() == Some(path.as_path()) {
                    self.project = None;
                    self.tasks = None;
                    self.roles = None;
                    self.skills = None;
                    self.mcp = None;
                    self.plugins = None;
                    self.retro = None;
                }
                result.map(|()| {
                    self.message = Some((self.tr.t("projects.removed").to_string(), false));
                })
            }
        };
        if let Err(error) = result {
            // Keep the form open with the problem, so nothing typed is lost.
            let mut form = form;
            form.error = Some(error);
            self.form = Some((purpose, form));
        }
    }

    /// Closes the open form without its OK; a new plugin version that was
    /// waiting for it is dropped.
    pub(crate) fn close_form(&mut self) {
        if let Some((Purpose::ApplyUpdate(prepared), _)) = self.form.take() {
            plugin_ops::discard(&prepared);
            self.message = Some((self.tr.t("plugins.update_dropped").to_string(), false));
        }
    }
}
