//! The Skills tab's work: creating, restoring and reloading the project's skills.

use std::fs;

use harness_core::git::Repo;
use harness_core::skills::{self};

use crate::ui::Form;
use crate::{skill_path, App, EditJob, EditKind, Purpose};

impl App {
    /// What the Skills tab asks for.
    pub(crate) fn skill_action(&mut self, action: super::Action) {
        use super::Action as A;
        let Some(root) = self.project.clone() else {
            return;
        };
        let tr = &self.tr;
        match action {
            A::None => {}
            A::Cycle(role, name) => {
                if let Some(roles) = &mut self.roles {
                    roles.cycle_skill(role, &name);
                }
            }
            A::Edit(name) => {
                let path = skill_path(&root, &name);
                let mut copied = false;
                if !path.is_file() {
                    let Some(copy) = skills::copy_of_built_in(&name) else {
                        return;
                    };
                    let written = path
                        .parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::write(&path, copy));
                    if let Err(error) = written {
                        self.message = Some((format!("{}: {error}", path.display()), true));
                        return;
                    }
                    copied = true;
                }
                self.edit = Some(EditJob {
                    name,
                    path,
                    copied,
                    kind: EditKind::Skill,
                });
            }
            A::New => {
                self.form = Some((
                    Purpose::NewSkill,
                    Form::new(
                        tr.t("skills.new_title"),
                        tr.t("skills.new_text"),
                        tr.t("skills.create"),
                    )
                    .field(tr.t("skills.new_name"), "")
                    .field(tr.t("skills.new_description"), ""),
                ));
            }
            A::Restore(name) => {
                let text = tr.f("skills.restore_text", &[("name", &name)]);
                self.form = Some((
                    Purpose::RestoreSkill(name),
                    Form::new(tr.t("skills.restore_title"), &text, tr.t("skills.restore")),
                ));
            }
        }
    }

    /// The skills changed: both tabs that show them read them again.
    pub(crate) fn reload_skills(&mut self, select: Option<&str>) {
        if let Some(skills) = &mut self.skills {
            skills.reload();
            if select.is_some() {
                skills.select_named(select);
            }
        }
        if let Some(roles) = self.roles.as_mut().filter(|r| !r.changed()) {
            roles.reload();
        }
    }

    /// OK in the «New skill» form: write the file and open it.
    pub(crate) fn create_skill(&mut self, form: &Form) -> Result<(), String> {
        let root = self.project.clone().ok_or_else(String::new)?;
        let (name, description) = (form.value(0), form.value(1));
        skills::check_name(name).map_err(|e| e.to_string())?;
        if self.skills.as_ref().is_some_and(|s| s.exists(name)) {
            return Err(self.tr.f("skills.exists", &[("name", &name)]));
        }
        if description.is_empty() {
            return Err(self.tr.t("skills.need_description").to_string());
        }
        let path = skill_path(&root, name);
        let text = format!("---\ndescription: {description}\n---\n# {name}\n\n");
        path.parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, text))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.reload_skills(Some(name));
        self.edit = Some(EditJob {
            name: name.to_string(),
            path,
            copied: false,
            kind: EditKind::Skill,
        });
        Ok(())
    }

    /// OK in the «Restore built-in» form: delete the project's copy.
    pub(crate) fn restore_skill(&mut self, name: &str) -> Result<(), String> {
        let root = self.project.clone().ok_or_else(String::new)?;
        let path = skill_path(&root, name);
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        let tracked = repo.is_tracked(&path);
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if tracked {
            repo.commit_paths(
                &[&path],
                &format!("harness: skill {name} is built-in again"),
            )
            .map_err(|e| e.to_string())?;
        }
        self.message = Some((self.tr.f("skills.restored", &[("name", &name)]), false));
        self.reload_skills(Some(name));
        Ok(())
    }
}
