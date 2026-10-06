//! Plugin downloads that run in the background: adding from a catalog and
//! updating, and what happens when such a job finishes.

use std::sync::mpsc;

use harness_core::git::Repo;
use harness_core::plugins;

use crate::ui::Form;
use crate::{App, PluginJob, Purpose};

impl App {
    /// Runs `work` in the background; its result comes in `tick`.
    pub(crate) fn start_plugin_job(
        &mut self,
        label: String,
        work: impl FnOnce() -> PluginJob + Send + 'static,
    ) {
        if self.plugin_job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.plugin_job = Some(rx);
        if let Some(plugins) = &mut self.plugins {
            plugins.busy = Some(label.clone());
        }
        self.message = Some((label, false));
    }

    /// «Update»: the newest version is downloaded in the background, then
    /// shown before it is taken.
    pub(crate) fn prepare_plugin_update(&mut self, name: String) {
        if self.roles_unsaved() {
            return;
        }
        let (Some(root), Some(home)) = (self.project.clone(), self.home.clone()) else {
            return;
        };
        let label = self.tr.f("plugins.checking_update", &[("name", &name)]);
        self.start_plugin_job(label, move || {
            let result = Repo::open(&root)
                .map_err(|e| e.to_string())
                .and_then(|repo| {
                    plugins::ops::prepare_update(&repo, &home, &name).map_err(|e| e.to_string())
                });
            PluginJob::UpdateReady(name, result)
        });
    }

    /// Applies a prepared plugin update; if that fails, the download is thrown away.
    pub(crate) fn apply_plugin_update(&mut self, prepared: &plugins::ops::Prepared) {
        let result = self
            .project
            .as_deref()
            .ok_or_else(String::new)
            .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
            .and_then(|repo| {
                plugins::ops::apply_update(&repo, prepared).map_err(|e| e.to_string())
            });
        self.message = Some(match result {
            Ok(()) => (
                self.tr.f("plugins.updated", &[("name", &prepared.name)]),
                false,
            ),
            Err(error) => {
                plugins::ops::discard(prepared);
                (
                    self.tr.f(
                        "plugins.update_failed",
                        &[("name", &prepared.name), ("error", &error)],
                    ),
                    true,
                )
            }
        });
        self.reload_plugins();
    }

    /// A download in the background finished.
    pub(crate) fn plugin_job_done(&mut self, done: PluginJob) {
        match done {
            PluginJob::CatalogAdded(result) => {
                self.message = Some(match result {
                    Ok(text) => (text, false),
                    Err(error) => (
                        self.tr.f("plugins.catalog_failed", &[("error", &error)]),
                        true,
                    ),
                });
                self.reload_catalog_views();
            }
            PluginJob::CatalogUpdated(name, result) => {
                let tr = &self.tr;
                self.message = Some(match result {
                    Ok(plugins::ops::CatalogUpdate::Local) => {
                        (tr.f("plugins.catalog_local", &[("name", &name)]), false)
                    }
                    Ok(plugins::ops::CatalogUpdate::Same(_)) => {
                        (tr.f("plugins.catalog_same", &[("name", &name)]), false)
                    }
                    Ok(plugins::ops::CatalogUpdate::Updated(_)) => {
                        (tr.f("plugins.catalog_updated", &[("name", &name)]), false)
                    }
                    Err(error) => (error, true),
                });
                self.reload_catalog_views();
            }
            PluginJob::Added { name, give, result } => {
                let added = match result {
                    Ok(added) => added,
                    Err(error) => {
                        let text = self
                            .tr
                            .f("plugins.add_failed", &[("name", &name), ("error", &error)]);
                        self.message = Some((text, true));
                        return;
                    }
                };
                self.reload_plugins();
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                    plugins.catalog = None;
                    plugins.catalogs = None;
                    plugins.select_named(&name, roles);
                }
                self.message = Some((self.tr.f("plugins.added", &[("name", &name)]), false));
                let contents = added.contents;
                if contents.hooks || contents.servers {
                    let tr = &self.tr;
                    let key = if contents.hooks {
                        "plugins.allow_hooks_text"
                    } else {
                        "plugins.allow_servers_text"
                    };
                    let text = format!(
                        "{}\n{}",
                        tr.f("plugins.added", &[("name", &name)]),
                        tr.f(key, &[("name", &name)])
                    );
                    self.form = Some((
                        Purpose::AllowPlugin {
                            name,
                            hooks: contents.hooks,
                            servers: contents.servers,
                            give,
                        },
                        Form::new(tr.t("plugins.allow_title"), &text, tr.t("plugins.allow")),
                    ));
                } else if let Some(role) = give {
                    self.give_plugin(&name, role);
                }
            }
            PluginJob::UpdateReady(name, result) => match result {
                Ok(None) => {
                    let text = self.tr.f("plugins.up_to_date", &[("name", &name)]);
                    self.message = Some((text, false));
                }
                Ok(Some(prepared)) => {
                    let tr = &self.tr;
                    let mut text = tr.f("plugins.update_text", &[("name", &name)]);
                    let changes = &prepared.changes;
                    let lines: Vec<String> = [
                        ("+", &changes.added),
                        ("~", &changes.changed),
                        ("-", &changes.removed),
                    ]
                    .iter()
                    .flat_map(|(sign, files)| files.iter().map(move |f| format!("{sign} {f}")))
                    .collect();
                    for line in lines.iter().take(12) {
                        text.push('\n');
                        text.push_str(line);
                    }
                    if lines.len() > 12 {
                        text.push('\n');
                        text.push_str(
                            &tr.f("plugins.more_files", &[("count", &(lines.len() - 12))]),
                        );
                    }
                    if prepared.contents.hooks || prepared.contents.servers {
                        text.push('\n');
                        text.push_str(tr.t("plugins.update_runs"));
                    }
                    self.form = Some((
                        Purpose::ApplyUpdate(Box::new(prepared)),
                        Form::new(tr.t("plugins.update_title"), &text, tr.t("plugins.update")),
                    ));
                }
                Err(error) => {
                    let text = self.tr.f(
                        "plugins.update_failed",
                        &[("name", &name), ("error", &error)],
                    );
                    self.message = Some((text, true));
                }
            },
        }
    }
}
