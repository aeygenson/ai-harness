//! `harness tui`: the harness in a full-screen terminal window, with tabs.
//!
//! ```text
//!  harness-test │ 1 Tasks │ 2 Roles │ 3 Skills │ … │ 7 Projects   [ + New project ] [ EN ]
//! ┌ Tasks ─────────────┐┌ task-001 · round 2 of 5 · done ──────────────────────────┐
//! │> task-001  done    ││  r1 architect  approved → human    Design ready          │
//! ...
//!  click or 1–8 tabs · ↑↓ select · wheel scroll · … · L language · q quit
//! ```
//!
//! Everything works with the mouse (click, double click, wheel) and with the
//! keyboard. The terminal's own text selection works with Shift held down
//! (on a Mac: Option in iTerm2, fn in Terminal).
//! Every change goes through the same core functions as the CLI. The texts
//! come from translation files (see `i18n`); English is the default.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use harness_agents::credentials;
use harness_core::config::McpConfig;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::Role;
use harness_core::mcp::McpServer;
use harness_core::mcp_registry::Entry;
use harness_core::mcp_tools::Tool;
use harness_core::models::{self, ModelList};
use harness_core::projects::{self, name_of};
use harness_core::skills::{self, SKILLS_DIR};
use harness_core::{plugin_ops, settings};
use harness_platform::editor;
use harness_platform::folder_dialog::{self, Native};
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

mod agents_tab;
mod background;
mod i18n;
mod keyboard;
mod keys;
mod mcp;
mod mcp_tab;
mod mouse;
mod picker;
mod plugin_catalog;
mod plugin_jobs;
mod plugins;
mod plugins_tab;
mod press;
mod projects_tab;
mod retro_tab;
mod roles_tab;
mod runner;
mod skills_tab;
mod tasks;
mod terminal;
mod theme;
mod ui;

use agents_tab::{AgentChecker, AgentsTab, Installer, JobEvent};
use i18n::I18n;
use mcp_tab::McpTab;
use picker::Browser;
use plugins_tab::PluginsTab;
use projects_tab::{has_config, ProjectsTab};
use retro_tab::{RetroBuilder, RetroTab};
use roles_tab::RolesTab;
use runner::Builder;
use skills_tab::SkillsTab;
use tasks::TasksTab;
use ui::{buttons, panel, ButtonId, Form, Hits, Target};

/// Two clicks on the same thing within this time are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Tasks,
    Roles,
    Skills,
    Mcp,
    Plugins,
    Retro,
    Projects,
    Agents,
}

/// The tabs in order, with the key of the label.
const TABS: [(Tab, &str); 8] = [
    (Tab::Tasks, "tabs.tasks"),
    (Tab::Roles, "tabs.roles"),
    (Tab::Skills, "tabs.skills"),
    (Tab::Mcp, "tabs.mcp"),
    (Tab::Plugins, "tabs.plugins"),
    (Tab::Retro, "tabs.retro"),
    (Tab::Projects, "tabs.projects"),
    (Tab::Agents, "tabs.agents"),
];

/// What the open form is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Purpose {
    /// Create a project in this folder: the form asks for its name.
    NewProject(PathBuf),
    /// The folder has no harness settings yet: create them?
    InitFolder(PathBuf),
    /// Take the project off the list (the folder stays).
    Remove(PathBuf),
    /// The model of the role selected on the Roles tab.
    Model,
    /// A new skill: its name and description.
    NewSkill,
    /// Delete the project's copy of a built-in skill.
    RestoreSkill(String),
    /// A new MCP server (`None`) or a change to this one.
    McpServer(Option<String>),
    /// Remove this MCP server.
    RemoveMcp(String),
    /// Save a secret for MCP servers: its name and value.
    Secret,
    /// What to search in the MCP registry.
    McpSearch,
    /// Remove this plugin from the project.
    RemovePlugin(String),
    /// Let this plugin run its hooks, its own servers; then give it to
    /// the role, if it was just added for it.
    AllowPlugin {
        name: String,
        hooks: bool,
        servers: bool,
        give: Option<Role>,
    },
    /// What to look for in the plugin catalogs.
    PluginSearch,
    /// A catalog to add: `owner/repo`, a git address or a folder.
    AddCatalog,
    /// Take this catalog off the list.
    RemoveCatalog(String),
    /// Take the new version of a plugin, after seeing what changes.
    ApplyUpdate(Box<plugin_ops::Prepared>),
    /// Make a retrospective of the open project.
    GenerateRetro,
    /// Apply these proposals of the retrospective in this folder.
    ApplyProposals(PathBuf, Vec<u32>),
    /// Run this maker's command to install or update the agent named.
    RunAgentCommand {
        name: &'static str,
        action: harness_agents::catalog::Action,
        command: String,
    },
}

/// What a download in the background brought.
#[derive(Debug)]
enum PluginJob {
    CatalogAdded(Result<String, String>),
    CatalogUpdated(String, Result<plugin_ops::CatalogUpdate, String>),
    Added {
        name: String,
        give: Option<Role>,
        result: Result<plugin_ops::Added, String>,
    },
    UpdateReady(String, Result<Option<plugin_ops::Prepared>, String>),
}

/// A skill file, a plugin folder or a retrospective, waiting to be opened in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EditJob {
    name: String,
    path: PathBuf,
    /// The file is a fresh copy of the built-in skill: if Lisa changes
    /// nothing, it is deleted again.
    copied: bool,
    /// What is edited: a skill, a plugin folder, a retrospective.
    kind: EditKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Skill,
    /// The folder of the plugin `name`.
    Plugin,
    /// The text of the retrospective `name` (its number).
    Retro,
    /// A file of a task step, only looked at: without Zed, in `$EDITOR`.
    View,
}

/// What a folder is being chosen for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    NewProject,
    Open,
}

/// Opens the TUI and runs until `q`. It starts with `start` if that folder is
/// a harness project, otherwise with the project opened last.
pub fn run(start: &Path) -> Result<()> {
    let mut app = App::new(projects::harness_home(), start);
    app.native = true;
    // Which agents are installed: asked once at the start, in the background.
    app.check_agents();
    let mut terminal = ratatui::init();
    // Give the mouse back to the terminal even if the program panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));
    // A pasted text arrives as one event, so its line breaks do not send it.
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = terminal::event_loop(&mut terminal, &mut app);
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Each agent's answer: its model list, or why there is none.
type Answers = Vec<(String, Result<ModelList, String>)>;

/// Asks the agents with a saved login (in `credentials_dir`) for their models.
type ModelAsker = fn(&Path) -> Answers;

fn ask_agents(credentials_dir: &Path) -> Answers {
    harness_agents::models::ask_all(credentials_dir, &Default::default())
}

/// Starts an MCP server in the project folder and asks it for its tools.
type McpChecker = fn(&McpServer, &Path) -> Result<Vec<Tool>, String>;

/// The form for a new or changed MCP server.
fn server_form(tr: &I18n, title: &str, text: &str, name: &str, server: &McpConfig) -> Form {
    let [command, variables, sign_in] = mcp_tab::form_values(server);
    Form::new(title, text, tr.t("mcp.ok"))
        .field(tr.t("mcp.name"), name)
        .field(tr.t("mcp.command_field"), &command)
        .field(tr.t("mcp.variables_field"), &variables)
        .field(tr.t("mcp.sign_in_field"), &sign_in)
}

/// Searches the MCP registry.
type McpSearcher = fn(&str) -> Result<Vec<Entry>, String>;

/// Signs in to a web MCP server in the browser: credentials folder, server
/// name, address.
type McpSigner = fn(&Path, &str, &str) -> Result<(), String>;

fn sign_in(credentials_dir: &Path, name: &str, url: &str) -> Result<(), String> {
    use harness_agents::mcp_oauth;
    let tools = mcp_oauth::Tools {
        curl: "curl".into(),
        open: &mcp_oauth::open_in_browser,
        browser_limit: mcp_oauth::BROWSER_LIMIT,
    };
    mcp_oauth::login(credentials_dir, name, url, &tools)
}

/// A server being checked: its name, settings and the coming answer.
type Checking = (String, McpConfig, mpsc::Receiver<Result<Vec<Tool>, String>>);

#[derive(Debug)]
struct App {
    tab: Tab,
    /// `~/.harness`: the project list, the language, more translations.
    home: Option<PathBuf>,
    tr: I18n,
    /// The open project.
    project: Option<PathBuf>,
    tasks: Option<TasksTab>,
    roles: Option<RolesTab>,
    skills: Option<SkillsTab>,
    mcp: Option<McpTab>,
    plugins: Option<PluginsTab>,
    retro: Option<RetroTab>,
    /// A skill to open in the editor after this event.
    edit: Option<EditJob>,
    /// An agent to sign in to after this event (the name `harness login` takes),
    /// with the name shown.
    sign_in: Option<(&'static str, &'static str)>,
    projects: ProjectsTab,
    agents: AgentsTab,
    /// Checks which agents are installed; tests give a fake one.
    agent_checker: AgentChecker,
    /// The answer of that check, while it runs.
    agent_check: Option<mpsc::Receiver<Vec<harness_agents::catalog::Status>>>,
    /// Runs an agent's install or update command; tests give a fake one.
    installer: Installer,
    /// What that command prints, while it runs.
    install_events: Option<mpsc::Receiver<JobEvent>>,
    form: Option<(Purpose, Form)>,
    /// The folder browser, when the system has no folder dialog.
    browser: Option<(Pick, Browser)>,
    /// Use the system's folder dialog (`kdialog`, `zenity`) when there is one.
    native: bool,
    /// Where choosing a folder starts: `~/code` if it exists.
    start_dir: PathBuf,
    /// The last result or problem, shown at the bottom.
    message: Option<(String, bool)>,
    hits: Hits,
    last_click: Option<(Instant, Target, u16)>,
    /// Builds the agents that run the roles.
    builder: Builder,
    /// Builds the agent of the retrospective.
    retro_builder: RetroBuilder,
    asker: ModelAsker,
    /// The command that opens a file in Zed without waiting; `None` without Zed.
    viewer: fn(&Path) -> Option<std::process::Command>,
    /// The answers of «Refresh models», while the agents are being asked.
    asking: Option<mpsc::Receiver<Answers>>,
    checker: McpChecker,
    /// «Check» on the MCP tab, while the server is asked.
    checking: Option<Checking>,
    searcher: McpSearcher,
    /// The registry's answer, while it is asked.
    searching: Option<mpsc::Receiver<Result<Vec<Entry>, String>>>,
    signer: McpSigner,
    /// The end of a sign-in in the browser: the server and how it went.
    signing: Option<(String, mpsc::Receiver<Result<(), String>>)>,
    /// A plugin or catalog download, while it runs.
    plugin_job: Option<mpsc::Receiver<PluginJob>>,
    /// The catalog added by itself when there is none.
    official_catalog: String,
    /// `q` was pressed once with unsaved changes.
    quit_warned: bool,
    quit: bool,
}

impl App {
    fn new(home: Option<PathBuf>, start: &Path) -> Self {
        let mut app = Self {
            tab: Tab::Projects,
            tr: I18n::load(home.as_deref()),
            home: home.clone(),
            project: None,
            tasks: None,
            roles: None,
            skills: None,
            mcp: None,
            plugins: None,
            retro: None,
            edit: None,
            sign_in: None,
            projects: ProjectsTab::load(home),
            agents: AgentsTab::new(),
            agent_checker: agents_tab::check,
            agent_check: None,
            installer: agents_tab::install,
            install_events: None,
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
            checker: harness_agents::mcp_check::list_tools,
            checking: None,
            searcher: harness_agents::mcp_registry::search,
            searching: None,
            signer: sign_in,
            signing: None,
            plugin_job: None,
            official_catalog: plugin_catalog::OFFICIAL.to_string(),
            quit_warned: false,
            quit: false,
        };
        let saved = app
            .home
            .as_deref()
            .and_then(|home| i18n::saved_setting(home, "theme"));
        theme::select(saved.as_deref().unwrap_or(theme::default_code()));
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
    fn open(&mut self, root: &Path) {
        if self.busy() {
            return;
        }
        if !has_config(root) {
            let text = self
                .tr
                .f("projects.not_a_project", &[("path", &root.display())]);
            self.message = Some((text, true));
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
        let saved = self.projects.update(|list| {
            if !list.projects.iter().any(|p| p.path == root) {
                list.add(&name_of(root), root);
            }
            list.last = Some(root.to_path_buf());
        });
        self.message = Some(match saved {
            Ok(()) => (
                self.tr.f("projects.opened", &[("name", &name_of(root))]),
                false,
            ),
            Err(error) => (self.tr.f("projects.not_saved", &[("error", &error)]), true),
        });
    }

    /// Roles are working: the project stays open until they finish.
    fn busy(&mut self) -> bool {
        let running = self.tasks.as_ref().is_some_and(TasksTab::is_running);
        if running {
            self.message = Some((self.tr.t("tasks.busy").to_string(), true));
        } else if self.generating() {
            self.message = Some((self.tr.t("retro.busy").to_string(), true));
            return true;
        }
        running
    }

    /// Back from `harness login`: say how it went and look at the logins again.
    fn finish_sign_in(&mut self, name: &str, result: Result<(), String>) {
        self.message = Some(match result {
            Ok(()) => (self.tr.f("agents.signed_in", &[("name", &name)]), false),
            Err(error) => (
                self.tr.f(
                    "agents.sign_in_failed",
                    &[("name", &name), ("error", &error)],
                ),
                true,
            ),
        });
        if let Some(dir) = self.home.as_ref().map(|h| h.join("credentials")) {
            self.agents.reload_logins(&dir);
        }
        if let Some(roles) = &mut self.roles {
            roles.set_ready(self.agents.ready());
        }
    }

    /// «Install», «Update» or «Remove»: first the command is shown to be confirmed.
    fn ask_to_run_agent_command(&mut self, remove: bool) {
        use harness_agents::catalog::Action;
        let Some((status, action, command)) = self.agents.next_step(remove) else {
            return;
        };
        let name = status.entry.name;
        let (title, ok, text) = match action {
            Action::Install => (
                "agents.confirm_install",
                "agents.run_install",
                "agents.confirm_text",
            ),
            Action::Update => (
                "agents.confirm_update",
                "agents.run_update",
                "agents.confirm_text",
            ),
            Action::Remove => (
                "agents.confirm_remove",
                "agents.run_remove",
                "agents.confirm_remove_text",
            ),
        };
        let tr = &self.tr;
        let mut text = tr.f(text, &[("command", &command)]);
        // Claude Code may also be what runs Claude's own sessions here.
        if action == Action::Remove && status.entry.id == "claude" {
            text = format!("{text}\n\n{}", tr.t("agents.remove_claude_note"));
        }
        if action == Action::Install && !status.entry.runs() {
            text = format!("{}\n\n{text}", tr.t("agents.confirm_not_run"));
        }
        let form = Form::new(&tr.f(title, &[("name", &name)]), &text, tr.t(ok));
        self.form = Some((
            Purpose::RunAgentCommand {
                name,
                action,
                command,
            },
            form,
        ));
    }

    /// Opens a tab. The Agents tab checks the computer the first time.
    fn show(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Agents && !self.agents.known {
            self.check_agents();
        }
    }

    /// The retrospective's agent is working.
    fn generating(&self) -> bool {
        self.retro.as_ref().is_some_and(RetroTab::is_generating)
    }

    /// Writes a changed harness.toml after the same checks as «Save», and
    /// reads it again everywhere.
    fn save_settings(
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
        settings::save(&repo, &text).map_err(|e| e.to_string())?;
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

    /// Closes the open form without its OK; a new plugin version that was
    /// waiting for it is dropped.
    fn close_form(&mut self) {
        if let Some((Purpose::ApplyUpdate(prepared), _)) = self.form.take() {
            plugin_ops::discard(&prepared);
            self.message = Some((self.tr.t("plugins.update_dropped").to_string(), false));
        }
    }

    /// Plugins change harness.toml: unsaved changes of the roles would be
    /// lost, so they are saved or undone first.
    fn roles_unsaved(&mut self) -> bool {
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        if unsaved {
            self.message = Some((self.tr.t("mcp.save_first").to_string(), true));
        }
        unsaved
    }

    /// What the Skills tab asks for.
    fn skill_action(&mut self, action: skills_tab::Action) {
        use skills_tab::Action as A;
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

    /// A file chosen in «Files» of the Tasks tab: Zed opens it and the TUI
    /// goes on; without Zed the editor gets the terminal.
    fn open_task_file(&mut self) {
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
            self.message = Some((self.tr.f("tasks.file_missing", &[("path", &shown)]), true));
            return;
        }
        match (self.viewer)(&path) {
            Some(command) => {
                self.message = Some(match editor::view(command) {
                    Ok(()) => (self.tr.f("tasks.file_opened", &[("path", &shown)]), false),
                    Err(error) => (
                        self.tr.f("skills.editor_failed", &[("error", &error)]),
                        true,
                    ),
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
    fn finish_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        match job.kind {
            EditKind::Skill => {}
            EditKind::Plugin => return self.finish_plugin_edit(job, result),
            EditKind::Retro => return self.finish_retro_edit(job, result),
            EditKind::View => {
                self.message = result.err().map(|error| {
                    (
                        self.tr.f("skills.editor_failed", &[("error", &error)]),
                        true,
                    )
                });
                return;
            }
        }
        let tr = &self.tr;
        let mut message = result
            .err()
            .map(|error| (tr.f("skills.editor_failed", &[("error", &error)]), true));
        let text = fs::read_to_string(&job.path).unwrap_or_default();
        let unchanged_copy =
            job.copied && skills::copy_of_built_in(&job.name).as_deref() == Some(text.as_str());
        if unchanged_copy {
            let _ = fs::remove_file(&job.path);
            message.get_or_insert((tr.t("skills.unchanged").to_string(), false));
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
                Err(error) => (error, true),
                Ok(_) if skills::split_header(&text).is_none() => {
                    (tr.f("skills.broken", &[("name", &job.name)]), true)
                }
                Ok(true) => (tr.f("skills.saved", &[("name", &job.name)]), false),
                Ok(false) => (tr.t("skills.unchanged").to_string(), false),
            };
            message.get_or_insert(next);
        }
        self.message = message;
        self.reload_skills(Some(&job.name));
    }

    /// The retrospective was open in the editor: keep what changed in git.
    fn finish_retro_edit(&mut self, job: &EditJob, result: Result<(), String>) {
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

    /// What the Retro tab asks for.
    fn retro_action(&mut self, action: retro_tab::Action) {
        use retro_tab::Action as A;
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
                let found = harness_core::suggest::load(&dir).ok();
                let tr = &self.tr;
                let mut text = tr.t("retro.apply_text").to_string();
                for id in &ids {
                    let Some(proposal) = found.as_ref().and_then(|f| f.proposals.get(*id)) else {
                        continue;
                    };
                    text.push_str(&format!("\n{id}. {}", proposal.summary));
                    use harness_core::proposals::FileChange;
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
    fn generate_retro(&mut self) {
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
    fn apply_proposals(&mut self, dir: &Path, ids: &[u32]) -> Result<(), String> {
        let root = self.project.clone().ok_or_else(String::new)?;
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        let applied = harness_core::retro_ops::apply(&repo, dir, ids).map_err(|e| e.to_string())?;
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

    /// The skills changed: both tabs that show them read them again.
    fn reload_skills(&mut self, select: Option<&str>) {
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
    fn create_skill(&mut self, form: &Form) -> Result<(), String> {
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
    fn restore_skill(&mut self, name: &str) -> Result<(), String> {
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

    /// «Refresh models»: the agents are asked in the background.
    fn ask_for_models(&mut self) {
        if self.asking.is_some() {
            return;
        }
        let Some(dir) = credentials::default_dir() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let asker = self.asker;
        std::thread::spawn(move || {
            let _ = tx.send(asker(&dir));
        });
        self.asking = Some(rx);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = true;
        }
        self.message = Some((self.tr.t("roles.refreshing").to_string(), false));
    }

    /// The agents answered: keep the lists and say what came back.
    fn models_answered(&mut self, answers: Answers) {
        let mut got = Vec::new();
        let mut failed = Vec::new();
        for (agent, answer) in answers {
            let saved = answer.and_then(|list| {
                let count = list.models.len();
                match &self.home {
                    Some(home) => models::save(home, &list).map_err(|e| e.to_string()),
                    None => Err("HOME is not set".into()),
                }
                .map(|()| count)
            });
            match saved {
                Ok(count) => got.push(format!("{agent} {count}")),
                Err(error) => failed.push(format!("{agent}: {error}")),
            }
        }
        let text = match (got.is_empty(), failed.is_empty()) {
            (true, true) => (self.tr.t("roles.no_logins").to_string(), true),
            (_, true) => (
                self.tr.f("roles.refreshed", &[("lists", &got.join(", "))]),
                false,
            ),
            _ => {
                let mut text = failed.join("; ");
                if !got.is_empty() {
                    text = format!(
                        "{}; {text}",
                        self.tr.f("roles.refreshed", &[("lists", &got.join(", "))])
                    );
                }
                (text, true)
            }
        };
        self.message = Some(text);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = false;
            roles.reload_models();
        }
    }

    /// OK in the open form.
    fn submit(&mut self) {
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

    /// Chooses a folder: in the system's dialog if there is one, otherwise
    /// in the TUI's own browser.
    fn pick(&mut self, pick: Pick) {
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
    fn picked(&mut self, pick: Pick, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        // The next choice starts next to this one.
        if let Some(parent) = path.parent() {
            self.start_dir = parent.to_path_buf();
        }
        match pick {
            Pick::NewProject if has_config(&path) => {
                self.message = Some((self.tr.t("form.exists").to_string(), true));
            }
            Pick::NewProject => {
                let tr = &self.tr;
                let text = tr.f("form.new_text", &[("path", &path.display())]);
                self.form = Some((
                    Purpose::NewProject(path.clone()),
                    Form::new(tr.t("form.new_title"), &text, tr.t("form.create"))
                        .field(tr.t("form.new_name"), &name_of(&path)),
                ));
            }
            Pick::Open if has_config(&path) => self.open(&path),
            Pick::Open => self.form = Some(self.init_form(&path)),
        }
    }

    fn create_project(&mut self, path: &Path, form: &Form) -> Result<(), String> {
        let path = path.to_path_buf();
        if has_config(&path) {
            return Err(self.tr.t("form.exists").to_string());
        }
        let done = projects::init(&path).map_err(|e| e.to_string())?;
        let path = path.canonicalize().unwrap_or(path);
        let name = match form.value(0) {
            "" => name_of(&path),
            name => name.to_string(),
        };
        self.projects.update(|list| list.add(&name, &path))?;
        self.open(&path);
        // A new project starts with choosing the agents.
        self.tab = Tab::Roles;
        let git = if done.created_git { "git, " } else { "" };
        let text = self
            .tr
            .f("projects.created", &[("name", &name), ("git", &git)]);
        self.message = Some((text, false));
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.hits.clear();
        frame.render_widget(
            ratatui::widgets::Block::new().style(theme::base()),
            frame.area(),
        );
        let [top, main, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_tabs(frame, top);

        match self.tab {
            Tab::Tasks => match &self.tasks {
                Some(tasks) => tasks.draw(frame, main, &mut self.hits, &self.tr),
                None => placeholder(
                    frame,
                    main,
                    self.tr.t("tasks.title"),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Roles => match &self.roles {
                Some(roles) => roles.draw(frame, main, &mut self.hits, &self.tr),
                None => placeholder(
                    frame,
                    main,
                    self.tr.t("roles.title"),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Skills => match (&self.skills, &self.roles) {
                (Some(skills), Some(roles)) => {
                    skills.draw(frame, main, &mut self.hits, &self.tr, roles);
                }
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.skills")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Mcp => match (&self.mcp, &self.roles) {
                (Some(mcp), Some(roles)) => mcp.draw(frame, main, &mut self.hits, &self.tr, roles),
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.mcp")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Plugins => match (&self.plugins, &self.roles) {
                (Some(plugins), Some(roles)) => {
                    plugins.draw(frame, main, &mut self.hits, &self.tr, roles);
                }
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.plugins")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Retro => match &self.retro {
                Some(retro) => retro.draw(frame, main, &mut self.hits, &self.tr),
                None => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.retro")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Agents => self.agents.draw(frame, main, &mut self.hits, &self.tr),
            Tab::Projects => {
                self.projects.draw(
                    frame,
                    main,
                    &mut self.hits,
                    self.project.as_deref(),
                    &self.tr,
                );
            }
        }

        // The latest result or problem first, so a narrow window still shows it.
        let mut spans = Vec::new();
        let problem = self
            .tasks
            .as_ref()
            .and_then(|t| t.problem.clone())
            .or_else(|| self.skills.as_ref().and_then(|s| s.problem.clone()))
            .or_else(|| self.projects.problem.clone())
            .or_else(|| self.tr.problems.first().cloned());
        if let Some((text, error)) = &self.message {
            let (mark, style) = if *error {
                ("✗", theme::bad())
            } else {
                ("✓", theme::ok())
            };
            spans.push(Span::styled(
                format!(" {mark} {text}  "),
                style.add_modifier(Modifier::BOLD),
            ));
        } else if let Some(problem) = problem {
            spans.push(Span::styled(
                format!(" ✗ {problem}  "),
                theme::bad().add_modifier(Modifier::BOLD),
            ));
        }
        let typing = self.tab == Tab::Tasks && self.tasks.as_ref().is_some_and(TasksTab::typing);
        let hint = if typing {
            "tasks.typing_hint"
        } else {
            "footer.hint"
        };
        // The key that lets the terminal select text while the TUI has the mouse.
        let copy = harness_platform::terminal::select_text_key();
        spans.push(Span::styled(
            format!(" {}", self.tr.f(hint, &[("copy", &copy)])),
            theme::dim(),
        ));
        frame.render_widget(Line::from(spans), footer);

        if let Some((_, form)) = &self.form {
            form.draw(frame, &mut self.hits, self.tr.t("form.cancel"));
        }
        if let Some((_, browser)) = &self.browser {
            browser.draw(frame, &mut self.hits, &self.tr);
        }
    }

    fn draw_tabs(&mut self, frame: &mut Frame, area: Rect) {
        let name = self
            .project
            .as_deref()
            .map_or_else(|| self.tr.t("tabs.no_project").to_string(), name_of);
        let mut x = area.x;
        let mut put = |frame: &mut Frame, text: String, style: Style| -> Rect {
            let width = u16::try_from(text.chars().count()).unwrap_or(0);
            let rect = Rect::new(x, area.y, width.min(area.right().saturating_sub(x)), 1);
            frame.render_widget(Span::styled(text, style), rect);
            x = x.saturating_add(width);
            rect
        };
        put(frame, format!(" ◆ {name} "), theme::primary());
        put(frame, " ".into(), Style::new());
        for (index, (tab, label)) in TABS.iter().enumerate() {
            if index > 0 {
                put(frame, "│".into(), theme::dim());
            }
            let style = if *tab == self.tab {
                theme::selected().patch(theme::accent())
            } else {
                theme::dim()
            };
            let rect = put(
                frame,
                format!(" {} {} ", index + 1, self.tr.t(label)),
                style,
            );
            self.hits.add(rect, Target::Tab(index));
        }
        // On the right: always at hand, whichever tab is open.
        let right = Rect::new(x, area.y, area.right().saturating_sub(x), 1);
        let theme_label = format!("◐ {}", self.tr.t(theme::current().name));
        let mut items = [
            (self.tr.t("tabs.new_project"), ButtonId::NewProject),
            (self.tr.label(), ButtonId::Language),
            (theme_label.as_str(), ButtonId::Theme),
        ];
        let width = |items: &[(&str, ButtonId)]| -> u16 {
            items
                .iter()
                .map(|(label, _)| ui::button_width(label) + 1)
                .sum()
        };
        // In a narrow window the theme button shows only its sign.
        if width(&items) >= right.width {
            items[2].0 = "◐";
        }
        let width = width(&items);
        let start = right.right().saturating_sub(width);
        if start > right.x {
            let area = Rect::new(start, area.y, width, 1);
            let items: Vec<_> = items.iter().map(|(l, id)| (*l, *id, true)).collect();
            buttons(frame, area, &mut self.hits, &items);
        }
    }

    fn init_form(&self, path: &Path) -> (Purpose, Form) {
        let tr = &self.tr;
        let text = tr.f("form.init_text", &[("path", &path.display())]);
        (
            Purpose::InitFolder(path.to_path_buf()),
            Form::new(tr.t("form.init_title"), &text, tr.t("form.create")),
        )
    }
}

/// The project's file of skill `name`.
fn skill_path(root: &Path, name: &str) -> PathBuf {
    root.join(HARNESS_DIR)
        .join(SKILLS_DIR)
        .join(format!("{name}.md"))
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

fn placeholder(frame: &mut Frame, area: Rect, title: &str, text: &str) {
    frame.render_widget(
        Paragraph::new(text.to_string())
            .block(panel(title, false))
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests;
