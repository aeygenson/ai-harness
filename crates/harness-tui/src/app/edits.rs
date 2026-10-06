//! Editing files outside the TUI: which file to open, and what to do once the
//! editor is closed.

use std::fs;

use harness_core::git::Repo;
use harness_core::skills::{self};
use harness_platform::editor;

use crate::ui::message::Message;
use crate::{App, EditJob, EditKind};

impl App {
    /// A file chosen in «Files» of the Tasks tab: Zed opens it and the TUI
    /// goes on; without Zed the editor gets the terminal.
    pub(crate) fn open_task_file(&mut self) {
        let Some(path) = self.tasks.as_mut().and_then(|t| t.open.take()) else {
            return;
        };
        let shown = self
            .project
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(harness_platform::path::slashed)
            .unwrap_or_else(|| path.display().to_string());
        if !path.is_file() {
            self.message = Some(Message::error(
                self.tr.f("tasks.file_missing", &[("path", &shown)]),
            ));
            return;
        }
        match (self.viewer)(&path) {
            Some(command) => {
                self.message = Some(match editor::view(command) {
                    Ok(()) => Message::info(self.tr.f("tasks.file_opened", &[("path", &shown)])),
                    Err(error) => {
                        Message::error(self.tr.f("skills.editor_failed", &[("error", &error)]))
                    }
                });
            }
            None => {
                self.edit = Some(EditJob {
                    name: shown,
                    path,
                    copied: false,
                    kind: EditKind::View,
                });
            }
        }
    }

    /// The editor was closed: keep the change in git, or drop a copy of a
    /// built-in skill that was not changed.
    pub(crate) fn finish_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        match job.kind {
            EditKind::Skill => {}
            EditKind::Plugin => return self.finish_plugin_edit(job, result),
            EditKind::Retro => return self.finish_retro_edit(job, result),
            EditKind::View => {
                self.message = result.err().map(|error| {
                    Message::error(self.tr.f("skills.editor_failed", &[("error", &error)]))
                });
                return;
            }
        }
        let tr = &self.tr;
        let mut message = result
            .err()
            .map(|error| Message::error(tr.f("skills.editor_failed", &[("error", &error)])));
        let text = fs::read_to_string(&job.path).unwrap_or_default();
        let unchanged_copy =
            job.copied && skills::copy_of_built_in(&job.name).as_deref() == Some(text.as_str());
        if unchanged_copy {
            let _ = fs::remove_file(&job.path);
            message.get_or_insert(Message::info(tr.t("skills.unchanged")));
        } else if job.path.is_file() {
            let saved = self
                .project
                .as_deref()
                .ok_or_else(String::new)
                .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
                .and_then(|repo| {
                    repo.commit_paths(&[&job.path], &format!("harness: skill {}", job.name))
                        .map_err(|e| e.to_string())
                });
            let next = match saved {
                Err(error) => Message::error(error),
                Ok(_) if skills::split_header(&text).is_none() => {
                    Message::error(tr.f("skills.broken", &[("name", &job.name)]))
                }
                Ok(true) => Message::info(tr.f("skills.saved", &[("name", &job.name)])),
                Ok(false) => Message::info(tr.t("skills.unchanged")),
            };
            message.get_or_insert(next);
        }
        self.message = message;
        self.reload_skills(Some(&job.name));
    }
}
