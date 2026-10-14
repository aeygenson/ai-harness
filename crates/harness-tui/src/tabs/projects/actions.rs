//! Opening and creating projects: the folder picker and the new project form.

use std::path::{Path, PathBuf};

use harness_core::config;
use harness_core::config::projects::{self, name_of};
use harness_platform::folder_dialog::{self, Native};

use crate::tabs::projects::has_config;
use crate::tabs::projects::picker::Browser;
use crate::ui::message::Message;
use crate::ui::Form;
use crate::{App, Pick, Purpose, Tab};

impl App {
    /// Chooses a folder: in the system's dialog if there is one, otherwise
    /// in the TUI's own browser.
    pub(crate) fn pick(&mut self, pick: Pick) {
        let title = match pick {
            Pick::NewProject => self.tr.t("picker.new_title"),
            Pick::Open => self.tr.t("picker.open_title"),
        }
        .to_string();
        let native = if self.native {
            folder_dialog::native_folder(&title, &self.start_dir)
        } else {
            Native::Unavailable
        };
        match native {
            Native::Chosen(path) => self.picked(pick, path),
            Native::Cancelled => {}
            Native::Unavailable => {
                self.browser = Some((pick, Browser::new(&title, &self.start_dir)));
            }
        }
    }

    /// A folder was chosen.
    pub(crate) fn picked(&mut self, pick: Pick, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        // The next choice starts next to this one.
        if let Some(parent) = path.parent() {
            self.start_dir = parent.to_path_buf();
        }
        match pick {
            Pick::NewProject if has_config(&path) => {
                self.message = Some(Message::error(self.tr.t("form.exists")));
            }
            Pick::NewProject => {
                let tr = &self.tr;
                let text = tr.f("form.new_text", &[("path", &path.display())]);
                self.form = Some((
                    Purpose::NewProject(path.clone()),
                    Form::new(tr.t("form.new_title"), &text, tr.t("form.create"))
                        .field(tr.t("form.new_name"), &name_of(&path))
                        .check(tr.t("form.new_github"), false)
                        .check(tr.t("form.github_private"), true),
                ));
            }
            Pick::Open if has_config(&path) => self.open(&path),
            Pick::Open => self.form = Some(self.init_form(&path)),
        }
    }

    /// Makes `path` a harness project, adds it under the form's name and opens it.
    pub(crate) fn create_project(&mut self, path: &Path, form: &Form) -> Result<(), String> {
        let path = path.to_path_buf();
        if has_config(&path) {
            return Err(self.tr.t("form.exists").to_string());
        }
        // The repository gets the folder's name: the project's name may have
        // spaces. Checked first, so a project is not made half way.
        let folder = name_of(&path);
        if form.is_checked(1) && !config::is_repo(&format!("owner/{folder}")) {
            return Err(self.tr.f("form.github_bad_name", &[("name", &folder)]));
        }
        let done = projects::init(&path).map_err(|e| e.to_string())?;
        let path = path.canonicalize().unwrap_or(path);
        let name = match form.value(0) {
            "" => name_of(&path),
            name => name.to_string(),
        };
        self.projects
            .update(|list| list.add(&name, &path), &self.tr)?;
        self.open(&path);
        // A new project starts with choosing the agents.
        self.tab = Tab::Roles;
        let git = if done.created_git { "git, " } else { "" };
        let text = self
            .tr
            .f("projects.created", &[("name", &name), ("git", &git)]);
        self.message = Some(Message::info(text));
        if form.is_checked(1) {
            self.connect_github(path.clone(), &folder, form.is_checked(2));
        }
        Ok(())
    }

    /// The form that offers to make an existing folder a harness project.
    pub(crate) fn init_form(&self, path: &Path) -> (Purpose, Form) {
        let tr = &self.tr;
        let text = tr.f("form.init_text", &[("path", &path.display())]);
        (
            Purpose::InitFolder(path.to_path_buf()),
            Form::new(tr.t("form.init_title"), &text, tr.t("form.create")),
        )
    }
}
