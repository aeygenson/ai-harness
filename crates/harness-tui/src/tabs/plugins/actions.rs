//! The Plugins tab's work: the catalog, giving a plugin to a role, allowing its
//! hooks and servers, and removing it.

use harness_core::git::Repo;
use harness_core::task::handoff::Role;
use harness_core::{config, plugins};

use crate::tabs::plugins::catalog;
use crate::ui::message::Message;
use crate::ui::Form;
use crate::{App, EditJob, EditKind, PluginJob, Purpose};

impl App {
    /// What the Plugins tab asks for.
    pub(crate) fn plugin_action(&mut self, action: super::Action) {
        use super::Action as A;
        let tr = &self.tr;
        match action {
            A::None => {}
            A::Refused(key) => self.message = Some(Message::error(tr.t(key))),
            A::Remove(name) => {
                let text = tr.f("plugins.remove_text", &[("name", &name)]);
                self.form = Some((
                    Purpose::RemovePlugin(name),
                    Form::new(tr.t("plugins.remove_title"), &text, tr.t("plugins.remove")),
                ));
            }
            A::Allow {
                name,
                hooks,
                servers,
            } => {
                let old = self.roles.as_ref().and_then(|r| r.plugins().get(&name));
                let more =
                    old.is_none_or(|p| (hooks && !p.allow_hooks) || (servers && !p.allow_mcp));
                if !more {
                    // Forbidding needs no question; it fails while a role
                    // still has the plugin.
                    if self.allow_plugin(&name, hooks, servers).is_err() {
                        let text = self.tr.f("plugins.forbid_used", &[("name", &name)]);
                        self.message = Some(Message::error(text));
                    }
                    return;
                }
                let key = if old.is_some_and(|p| hooks && !p.allow_hooks) {
                    "plugins.allow_hooks_text"
                } else {
                    "plugins.allow_servers_text"
                };
                let text = tr.f(key, &[("name", &name)]);
                self.form = Some((
                    Purpose::AllowPlugin {
                        name,
                        hooks,
                        servers,
                        give: None,
                    },
                    Form::new(tr.t("plugins.allow_title"), &text, tr.t("plugins.allow")),
                ));
            }
            A::OpenCatalog => self.open_plugin_catalog(),
            A::Search(query) => {
                self.form = Some((
                    Purpose::PluginSearch,
                    Form::new(
                        tr.t("plugins.search_title"),
                        tr.t("plugins.search_text"),
                        tr.t("plugins.search"),
                    )
                    .field(tr.t("plugins.search_field"), &query),
                ));
            }
            A::Unusable(why) => {
                self.message = Some(Message::error(tr.f("plugins.cannot_add", &[("why", &why)])));
            }
            A::Add { entry, give } => self.add_plugin(entry, give),
            A::Update(name) => self.prepare_plugin_update(name),
            A::OpenCatalogs => {
                let view = catalog::CatalogsView::load(self.home.as_deref());
                if let Some(plugins) = &mut self.plugins {
                    plugins.catalogs = Some(view);
                }
            }
            A::AddCatalog(offered) => {
                self.form = Some((
                    Purpose::AddCatalog,
                    Form::new(
                        tr.t("plugins.add_catalog_title"),
                        tr.t("plugins.add_catalog_text"),
                        tr.t("plugins.add"),
                    )
                    .field(tr.t("plugins.catalog_field"), &offered),
                ));
            }
            A::UpdateCatalog(name) => {
                let Some(home) = self.home.clone() else {
                    return;
                };
                let label = tr.f("plugins.updating_catalog", &[("name", &name)]);
                self.start_plugin_job(label, move || {
                    let result =
                        plugins::ops::update_catalog(&home, &name).map_err(|e| e.to_string());
                    PluginJob::CatalogUpdated(name, result)
                });
            }
            A::RemoveCatalog(name) => {
                let text = tr.f("plugins.remove_catalog_text", &[("name", &name)]);
                self.form = Some((
                    Purpose::RemoveCatalog(name),
                    Form::new(
                        tr.t("plugins.remove_catalog_title"),
                        &text,
                        tr.t("plugins.remove"),
                    ),
                ));
            }
            A::Open(name) => {
                let path = self.plugins.as_ref().and_then(|tab| {
                    let plugin = self.roles.as_ref()?.plugins().get(&name)?;
                    Some(tab.folder(&name, plugin))
                });
                match path {
                    Some(path) if path.is_dir() => {
                        self.edit = Some(EditJob {
                            name,
                            path,
                            copied: false,
                            kind: EditKind::Plugin,
                        });
                    }
                    Some(path) => {
                        let text = tr.f("plugins.no_folder", &[("path", &path.display())]);
                        self.message = Some(Message::error(text));
                    }
                    None => {}
                }
            }
        }
    }

    /// «From catalog»; with no catalog at all, the official one is added
    /// by itself.
    fn open_plugin_catalog(&mut self) {
        let agent = match (&self.plugins, &self.roles) {
            (Some(plugins), Some(roles)) => roles.settings(plugins.role()).map(|s| s.agent),
            _ => return,
        };
        let view = catalog::CatalogView::load(self.home.as_deref(), agent);
        let none = self
            .home
            .as_deref()
            .is_some_and(|home| plugins::ops::catalogs(home).is_ok_and(|c| c.is_empty()));
        if let Some(plugins) = &mut self.plugins {
            plugins.catalog = Some(view);
        }
        if none {
            self.start_catalog_add(self.official_catalog.clone());
        }
    }

    /// Downloads the plugin catalog at `source` in the background.
    pub(crate) fn start_catalog_add(&mut self, source: String) {
        let Some(home) = self.home.clone() else {
            return;
        };
        let label = self
            .tr
            .f("plugins.downloading_catalog", &[("source", &source)]);
        let added = self.tr.t("plugins.catalog_added").to_string();
        self.start_plugin_job(label, move || {
            let result = plugins::ops::add_catalog(&home, &source, None)
                .map(|catalog| {
                    added
                        .replace("{name}", &catalog.name)
                        .replace("{count}", &catalog.entries.len().to_string())
                })
                .map_err(|e| e.to_string());
            PluginJob::CatalogAdded(result)
        });
    }

    /// «Add»: the plugin is downloaded and copied in the background.
    fn add_plugin(&mut self, entry: harness_core::plugins::catalog::Entry, give: bool) {
        if self.roles_unsaved() {
            return;
        }
        let (Some(root), Some(home), Some(plugins)) =
            (self.project.clone(), self.home.clone(), &self.plugins)
        else {
            return;
        };
        let give = give.then(|| plugins.role());
        let name = entry.name.clone();
        let label = self.tr.f("plugins.adding", &[("name", &name)]);
        self.start_plugin_job(label, move || {
            let result = Repo::open(&root)
                .map_err(|e| e.to_string())
                .and_then(|repo| {
                    plugins::ops::add(&repo, &home, &entry, None, false, false)
                        .map_err(|e| e.to_string())
                });
            PluginJob::Added { name, give, result }
        });
    }

    /// Gives the plugin to the role and saves it at once.
    pub(crate) fn give_plugin(&mut self, name: &str, role: Role) {
        let Some(mut settings) = self.roles.as_ref().and_then(|r| r.settings(role)).cloned() else {
            return;
        };
        if !settings.plugins.iter().any(|n| n == name) {
            settings.plugins.push(name.to_string());
        }
        let result = self.save_settings(|text| {
            config::edit::set_role(text, role, &settings).map_err(|e| e.to_string())
        });
        let role = role.as_str();
        self.message = Some(match result {
            Ok(()) => Message::info(
                self.tr
                    .f("plugins.given", &[("name", &name), ("role", &role)]),
            ),
            Err(error) => Message::error(error),
        });
    }

    /// The roles and plugins are read again after harness.toml changed.
    pub(crate) fn reload_plugins(&mut self) {
        if let Some(roles) = &mut self.roles {
            roles.reload();
            if let Some(plugins) = &mut self.plugins {
                plugins.reload(roles);
            }
        }
        if let Some(tasks) = &mut self.tasks {
            tasks.reload();
        }
    }

    /// Reads the catalogs again for the open catalog window.
    pub(crate) fn reload_catalog_views(&mut self) {
        let home = self.home.clone();
        if let Some(plugins) = &mut self.plugins {
            if let Some(view) = &mut plugins.catalog {
                view.reload(home.as_deref());
            }
            if let Some(view) = &mut plugins.catalogs {
                view.reload(home.as_deref());
            }
        }
    }

    /// OK in «Remove the plugin»: its folder and settings go, in one commit.
    pub(crate) fn remove_plugin(&mut self, name: &str) -> Result<(), String> {
        let (Some(root), Some(roles)) = (self.project.clone(), &mut self.roles) else {
            return Err(self.tr.t("tabs.no_open_project").to_string());
        };
        if roles.changed() {
            return Err(self.tr.t("mcp.save_first").to_string());
        }
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        plugins::ops::remove(&repo, name).map_err(|e| e.to_string())?;
        roles.reload();
        if let Some(plugins) = &mut self.plugins {
            plugins.reload(roles);
        }
        if let Some(tasks) = &mut self.tasks {
            tasks.reload();
        }
        self.message = Some(Message::info(
            self.tr.f("plugins.removed", &[("name", &name)]),
        ));
        Ok(())
    }

    /// Writes what the plugin may run by itself; checked like «Save».
    pub(crate) fn allow_plugin(
        &mut self,
        name: &str,
        hooks: bool,
        servers: bool,
    ) -> Result<(), String> {
        self.save_settings(|text| {
            config::edit::set_plugin_allow(text, name, hooks, servers).map_err(|e| e.to_string())
        })?;
        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
            plugins.select_named(name, roles);
        }
        self.message = Some(Message::info(
            self.tr.f("plugins.allow_saved", &[("name", &name)]),
        ));
        Ok(())
    }

    /// The plugin's folder was open in the editor: keep what changed in git.
    pub(crate) fn finish_plugin_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        let tr = &self.tr;
        let saved = self
            .project
            .as_deref()
            .ok_or_else(String::new)
            .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
            .and_then(|repo| {
                repo.commit_paths(&[&job.path], &format!("harness: plugin {}", job.name))
                    .map_err(|e| e.to_string())
            });
        self.message = Some(match (result, saved) {
            (Err(error), _) => Message::error(tr.f("skills.editor_failed", &[("error", &error)])),
            (_, Err(error)) => Message::error(error),
            (_, Ok(true)) => Message::info(tr.f("plugins.edited", &[("name", &job.name)])),
            (_, Ok(false)) => Message::info(tr.t("plugins.unchanged")),
        });
        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
            plugins.reload(roles);
        }
    }
}
