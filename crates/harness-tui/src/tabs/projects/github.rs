//! Putting a project on GitHub from the Projects tab: «Connect GitHub» for a
//! project on the list, or the box in the new project form. The work (`gh`
//! and `git push`) runs in the background; see `harness_agents::github::connect`.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use harness_core::config::projects::name_of;
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;

use crate::app::background::look;
use crate::tabs::projects::has_config;
use crate::tabs::tasks::TasksTab;
use crate::ui::message::Message;
use crate::ui::Form;
use crate::{App, Purpose};

/// Puts the project in a folder on GitHub as a repository (`name` or
/// `owner/name`), private or not; returns `owner/name`. Tests give a fake one.
pub type GitHubConnector = fn(&Path, &str, bool) -> Result<String, String>;

/// The answer of a connection running in the background.
pub type GitHubJob = mpsc::Receiver<Result<String, String>>;

impl App {
    /// «Connect GitHub»: asks for the repository's name for the selected project.
    pub(crate) fn ask_to_connect_github(&mut self) {
        let Some(project) = self.projects.current() else {
            return;
        };
        if !has_config(&project.path) {
            let text = self.tr.f(
                "projects.not_a_project",
                &[("path", &project.path.display())],
            );
            self.message = Some(Message::error(text));
            return;
        }
        // A project already on GitHub (an earlier try that did not finish)
        // offers its own repository.
        let repo = Config::load(&project.path.join(HARNESS_DIR))
            .ok()
            .and_then(|config| config.github)
            .map_or_else(|| name_of(&project.path), |github| github.repo);
        let tr = &self.tr;
        let text = tr.f("form.github_text", &[("name", &project.name)]);
        let form = Form::new(tr.t("form.github_title"), &text, tr.t("form.github_ok"))
            .field(tr.t("form.github_name"), &repo)
            .check(tr.t("form.github_private"), true);
        self.form = Some((Purpose::ConnectGitHub(project.path.clone()), form));
    }

    /// The project is being put on GitHub: says so and answers `true`.
    /// Meanwhile nothing else may change `harness.toml` or the branches
    /// (saving settings, starting a task, quitting).
    pub(crate) fn connecting(&mut self) -> bool {
        if self.github_job.is_none() {
            return false;
        }
        self.message = Some(Message::error(self.tr.t("projects.github_wait")));
        true
    }

    /// Starts putting the project in `root` on GitHub in the background.
    pub(crate) fn connect_github(&mut self, root: PathBuf, name: &str, private: bool) {
        if self.connecting() {
            return;
        }
        // A running task commits and switches branches in the same folder.
        if self.tasks.as_ref().is_some_and(TasksTab::is_running) {
            self.message = Some(Message::error(self.tr.t("projects.github_task_running")));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let connector = self.github_connector;
        let name = name.to_string();
        self.message = Some(Message::info(
            self.tr.f("projects.github_working", &[("name", &name)]),
        ));
        std::thread::spawn(move || {
            let _ = tx.send(connector(&root, &name, private));
        });
        self.github_job = Some(rx);
    }

    /// The connection has finished: say how it went, and read the open
    /// project again (its `harness.toml` now has `[github]`).
    pub(crate) fn take_github_job(&mut self) {
        let stopped = Err(self.tr.t("projects.github_stopped").to_string());
        let Some(answer) = look(self.github_job.as_ref()).answer_or(stopped) else {
            return;
        };
        self.github_job = None;
        // Even a failed try may have committed `[github]`.
        self.reload_all();
        self.message = Some(match answer {
            Ok(repo) => Message::info(self.tr.f("projects.github_done", &[("repo", &repo)])),
            Err(error) => Message::error(self.tr.f(
                "projects.github_failed",
                &[("error", &harness_core::text::safe_line(&error, 400))],
            )),
        });
    }
}
