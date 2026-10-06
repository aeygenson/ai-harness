//! The Retro tab's work: generating a retrospective and applying the proposals
//! Lisa accepted.

use std::path::Path;

use harness_core::config::projects::name_of;
use harness_core::git::{Repo, HARNESS_DIR};

use crate::tabs::tasks::TasksTab;
use crate::ui::Form;
use crate::{App, EditJob, EditKind, Purpose};

impl App {
    /// What the Retro tab asks for.
    pub(crate) fn retro_action(&mut self, action: super::Action) {
        use super::Action as A;
        match action {
            A::None => {}
            A::Say(key) => self.message = Some((self.tr.t(key).to_string(), true)),
            A::Generate => {
                if self.tasks.as_ref().is_some_and(TasksTab::is_running) {
                    self.message = Some((self.tr.t("retro.tasks_running").to_string(), true));
                    return;
                }
                // Lisa sees which project the retrospective is for.
                let Some(root) = &self.project else {
                    return;
                };
                let tr = &self.tr;
                let text = tr.f(
                    "retro.generate_text",
                    &[("name", &name_of(root)), ("path", &root.display())],
                );
                self.form = Some((
                    Purpose::GenerateRetro,
                    Form::new(tr.t("retro.generate_title"), &text, tr.t("retro.generate")),
                ));
            }
            A::Open(number, path) => {
                self.edit = Some(EditJob {
                    name: number,
                    path,
                    copied: false,
                    kind: EditKind::Retro,
                });
            }
            A::Apply(dir, ids) => {
                if self.roles_unsaved() {
                    return;
                }
                let Some(root) = self.project.clone() else {
                    return;
                };
                let harness_dir = root.join(HARNESS_DIR);
                let config = harness_core::config::Config::load(&harness_dir).ok();
                let found = harness_core::retro::suggest::load(&dir).ok();
                let tr = &self.tr;
                let mut text = tr.t("retro.apply_text").to_string();
                for id in &ids {
                    let Some(proposal) = found.as_ref().and_then(|f| f.proposals.get(*id)) else {
                        continue;
                    };
                    text.push_str(&format!("\n{id}. {}", proposal.summary));
                    use harness_core::retro::proposals::FileChange;
                    let file = match (proposal.file_change(&harness_dir), &proposal.content) {
                        (FileChange::New, Some(_)) => Some("retro.new_skill"),
                        (FileChange::Changed { .. }, Some(_)) => Some("retro.changed_skill"),
                        _ => None,
                    };
                    if let Some(key) = file {
                        text.push_str(&format!("\n   {}", tr.f(key, &[("name", &proposal.skill)])));
                    }
                    let roles: Vec<&str> = config
                        .as_ref()
                        .map(|c| proposal.missing_roles(c))
                        .unwrap_or_default()
                        .iter()
                        .map(|given| given.role.as_str())
                        .collect();
                    if !roles.is_empty() {
                        text.push_str(&format!(
                            "\n   {}",
                            tr.f(
                                "retro.given_to",
                                &[("name", &proposal.skill), ("roles", &roles.join(", "))]
                            )
                        ));
                    }
                }
                self.form = Some((
                    Purpose::ApplyProposals(dir, ids),
                    Form::new(tr.t("retro.apply_title"), &text, tr.t("retro.apply_ok")),
                ));
            }
        }
    }

    /// OK in «Make a retrospective»: the agent starts in the background.
    pub(crate) fn generate_retro(&mut self) {
        // The roles may have started while the question was open.
        if self.tasks.as_ref().is_some_and(TasksTab::is_running) {
            self.message = Some((self.tr.t("retro.tasks_running").to_string(), true));
            return;
        }
        let language = self.tr.t("retro.language").to_string();
        if let Some(retro) = &mut self.retro {
            if retro.is_generating() {
                return;
            }
            retro.generate(self.retro_builder, &language);
            self.message = Some((self.tr.t("retro.started").to_string(), false));
        }
    }

    /// OK in «Apply proposals»: skills and harness.toml change, one commit.
    pub(crate) fn apply_proposals(&mut self, dir: &Path, ids: &[u32]) -> Result<(), String> {
        let root = self.project.clone().ok_or_else(String::new)?;
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        let applied =
            harness_core::retro::ops::apply(&repo, dir, ids).map_err(|e| e.to_string())?;
        let list: Vec<String> = applied.iter().map(u32::to_string).collect();
        self.message = Some((
            self.tr.f("retro.applied", &[("ids", &list.join(", "))]),
            false,
        ));
        if let Some(retro) = &mut self.retro {
            retro.chosen.clear();
            retro.reload();
        }
        self.reload_skills(None);
        Ok(())
    }

    /// The retrospective was open in the editor: keep what changed in git.
    pub(crate) fn finish_retro_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        let tr = &self.tr;
        let saved = self
            .project
            .as_deref()
            .ok_or_else(String::new)
            .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
            .and_then(|repo| {
                repo.commit_paths(&[&job.path], &format!("harness: retro {} edited", job.name))
                    .map_err(|e| e.to_string())
            });
        self.message = Some(match (result, saved) {
            (Err(error), _) => (tr.f("skills.editor_failed", &[("error", &error)]), true),
            (_, Err(error)) => (error, true),
            (_, Ok(true)) => (tr.f("retro.edited", &[("number", &job.name)]), false),
            (_, Ok(false)) => (tr.t("retro.unchanged").to_string(), false),
        });
        if let Some(retro) = &mut self.retro {
            retro.reload();
        }
    }
}
