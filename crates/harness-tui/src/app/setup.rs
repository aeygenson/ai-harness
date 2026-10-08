//! Creating the `App`, opening a project, and the checks before leaving a tab or
//! the program (work still running, unsaved roles).

use std::path::{Path, PathBuf};

use anyhow::Result;

use harness_core::config;
use harness_core::config::projects::name_of;
use harness_core::git::Repo;
use harness_platform::editor;

use crate::tabs;
use crate::tabs::agents::AgentsTab;
use crate::tabs::mcp::McpTab;
use crate::tabs::plugins::PluginsTab;
use crate::tabs::projects::{has_config, ProjectsTab};
use crate::tabs::retro::RetroTab;
use crate::tabs::roles::RolesTab;
use crate::tabs::skills::SkillsTab;
use crate::tabs::tasks::TasksTab;
use crate::ui;
use crate::ui::i18n::I18n;
use crate::ui::message::Message;
use crate::ui::Hits;
use crate::{ask_agents, sign_in, App, Tab};

impl App {
    pub(crate) fn new(home: Option<PathBuf>, start: &Path) -> Self {
        let tr = I18n::load(home.as_deref());
        let mut app = Self {
            tab: Tab::Projects,
            projects: ProjectsTab::load(home.clone(), &tr),
            tr,
            home,
            project: None,
            tasks: None,
            roles: None,
            skills: None,
            mcp: None,
            plugins: None,
            retro: None,
            edit: None,
            sign_in: None,
            agents: AgentsTab::new(),
            agent_checker: tabs::agents::check,
            agent_check: None,
            tool_checker: harness_agents::install::tools::check,
            tool_check: None,
            installer: tabs::agents::install,
            install_events: None,
            release_checker: tabs::agents::check_release,
            release_check: None,
            updater: tabs::agents::update_harness,
            form: None,
            browser: None,
            native: false,
            start_dir: default_start_dir(),
            message: None,
            hits: Hits::default(),
            last_click: None,
            builder: harness_agents::build::build_team,
            retro_builder: harness_agents::build::retro_agent,
            asker: ask_agents,
            viewer: editor::viewer,
            asking: None,
            checker: harness_agents::mcp::check::list_tools,
            checking: None,
            searcher: harness_agents::mcp::registry::search,
            searching: None,
            signer: sign_in,
            signing: None,
            plugin_job: None,
            official_catalog: tabs::plugins::catalog::OFFICIAL.to_string(),
            splash: crate::app::splash::Splash::default(),
            quit_warned: false,
            quit: false,
        };
        app.splash.at_start = crate::app::splash::shown_at_start(app.home.as_deref());
        let saved = app
            .home
            .as_deref()
            .and_then(|home| ui::i18n::saved_setting(home, "theme"));
        ui::theme::select(saved.as_deref().unwrap_or(ui::theme::default_code()));
        let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        let last = app.projects.list.last.clone();
        if has_config(&start) {
            app.open(&start);
        } else if let Some(last) = last.filter(|p| has_config(p)) {
            app.open(&last);
        }
        app
    }

    /// Opens a project: it becomes the current one and the one opened last.
    pub(crate) fn open(&mut self, root: &Path) {
        if self.busy() {
            return;
        }
        if !has_config(root) {
            let text = self
                .tr
                .f("projects.not_a_project", &[("path", &root.display())]);
            self.message = Some(Message::error(text));
            return;
        }
        self.tasks = Some(TasksTab::load(root, self.home.as_deref()));
        let mut roles = RolesTab::load(root, self.home.as_deref());
        roles.set_ready(self.agents.ready());
        self.plugins = Some(PluginsTab::load(root, &roles));
        self.roles = Some(roles);
        self.skills = Some(SkillsTab::load(root));
        self.retro = Some(RetroTab::load(root));
        self.mcp = Some(McpTab::load(self.home.as_deref()));
        self.project = Some(root.to_path_buf());
        self.tab = Tab::Tasks;
        let saved = self.projects.update(
            |list| {
                if !list.projects.iter().any(|p| p.path == root) {
                    list.add(&name_of(root), root);
                }
                list.last = Some(root.to_path_buf());
            },
            &self.tr,
        );
        self.message = Some(match saved {
            Ok(()) => Message::info(self.tr.f("projects.opened", &[("name", &name_of(root))])),
            Err(error) => Message::error(self.tr.f("projects.not_saved", &[("error", &error)])),
        });
    }

    /// Roles are working: the project stays open until they finish.
    pub(crate) fn busy(&mut self) -> bool {
        let running = self.tasks.as_ref().is_some_and(TasksTab::is_running);
        if running {
            self.message = Some(Message::error(self.tr.t("tasks.busy")));
        } else if self.generating() {
            self.message = Some(Message::error(self.tr.t("retro.busy")));
            return true;
        }
        running
    }

    /// Opens a tab. The Agents tab checks the computer the first time.
    pub(crate) fn show(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Agents && !self.agents.known {
            self.check_agents();
        }
    }

    /// The retrospective's agent is working.
    pub(crate) fn generating(&self) -> bool {
        self.retro.as_ref().is_some_and(RetroTab::is_generating)
    }

    /// Writes a changed harness.toml after the same checks as «Save», and
    /// reads it again everywhere.
    pub(crate) fn save_settings(
        &mut self,
        change: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<(), String> {
        let (Some(root), Some(roles)) = (self.project.clone(), &mut self.roles) else {
            return Err(self.tr.t("tabs.no_open_project").to_string());
        };
        if roles.changed() {
            return Err(self.tr.t("mcp.save_first").to_string());
        }
        let text = change(roles.text())?;
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        config::save::save(&repo, &text).map_err(|e| e.to_string())?;
        roles.reload();
        if let Some(plugins) = &mut self.plugins {
            plugins.reload(roles);
        }
        if let Some(tasks) = &mut self.tasks {
            tasks.reload();
        }
        if let Some(skills) = &mut self.skills {
            skills.reload();
        }
        Ok(())
    }

    /// Plugins change harness.toml: unsaved changes of the roles would be
    /// lost, so they are saved or undone first.
    pub(crate) fn roles_unsaved(&mut self) -> bool {
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        if unsaved {
            self.message = Some(Message::error(self.tr.t("mcp.save_first")));
        }
        unsaved
    }
}

/// `~/code` if it exists, otherwise the home folder.
fn default_start_dir() -> PathBuf {
    let home = harness_platform::home::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let code = home.join("code");
    if code.is_dir() {
        code
    } else {
        home
    }
}
