//! Plugin downloads that run in the background: adding from a catalog and
//! updating, and what happens when such a job finishes.

use std::sync::mpsc;

use harness_core::git::Repo;
use harness_core::plugins;
use harness_core::task::handoff::Role;

use crate::ui::message::Message;
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
        self.message = Some(Message::info(label));
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
            Ok(()) => Message::info(self.tr.f("plugins.updated", &[("name", &prepared.name)])),
            Err(error) => {
                plugins::ops::discard(prepared);
                Message::error(self.tr.f(
                    "plugins.update_failed",
                    &[("name", &prepared.name), ("error", &error)],
                ))
            }
        });
        self.reload_plugins();
    }

    /// A download in the background finished.
    pub(crate) fn plugin_job_done(&mut self, done: PluginJob) {
        match done {
            PluginJob::CatalogAdded(result) => {
                self.message = Some(match result {
                    Ok(text) => Message::info(text),
                    Err(error) => {
                        Message::error(self.tr.f("plugins.catalog_failed", &[("error", &error)]))
                    }
                });
                self.reload_catalog_views();
            }
            PluginJob::CatalogUpdated(name, result) => {
                let tr = &self.tr;
                self.message = Some(match result {
                    Ok(plugins::ops::CatalogUpdate::Local) => {
                        Message::info(tr.f("plugins.catalog_local", &[("name", &name)]))
                    }
                    Ok(plugins::ops::CatalogUpdate::Same(_)) => {
                        Message::info(tr.f("plugins.catalog_same", &[("name", &name)]))
                    }
                    Ok(plugins::ops::CatalogUpdate::Updated(_)) => {
                        Message::info(tr.f("plugins.catalog_updated", &[("name", &name)]))
                    }
                    Err(error) => Message::error(error),
                });
                self.reload_catalog_views();
            }
            PluginJob::Added { name, give, result } => self.plugin_added(name, give, result),
            PluginJob::UpdateReady(name, result) => match result {
                Ok(None) => {
                    let text = self.tr.f("plugins.up_to_date", &[("name", &name)]);
                    self.message = Some(Message::info(text));
                }
                Ok(Some(prepared)) => self.offer_plugin_update(prepared),
                Err(error) => {
                    let text = self.tr.f(
                        "plugins.update_failed",
                        &[("name", &name), ("error", &error)],
                    );
                    self.message = Some(Message::error(text));
                }
            },
        }
    }

    /// A plugin was copied into the project: select it, then ask to allow what
    /// it brings, or give it to `give` at once.
    fn plugin_added(
        &mut self,
        name: String,
        give: Option<Role>,
        result: Result<plugins::ops::Added, String>,
    ) {
        let added = match result {
            Ok(added) => added,
            Err(error) => {
                let text = self
                    .tr
                    .f("plugins.add_failed", &[("name", &name), ("error", &error)]);
                self.message = Some(Message::error(text));
                return;
            }
        };
        self.reload_plugins();
        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
            plugins.catalog = None;
            plugins.catalogs = None;
            plugins.select_named(&name, roles);
        }
        self.message = Some(Message::info(
            self.tr.f("plugins.added", &[("name", &name)]),
        ));
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

    /// A newer version of a plugin is ready: show what changes and ask to apply it.
    fn offer_plugin_update(&mut self, prepared: plugins::ops::Prepared) {
        let tr = &self.tr;
        let mut text = tr.f("plugins.update_text", &[("name", &prepared.name)]);
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
            text.push_str(&tr.f("plugins.more_files", &[("count", &(lines.len() - 12))]));
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
}
