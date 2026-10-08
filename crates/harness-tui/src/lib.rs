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

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use harness_core::config::projects;
use harness_core::config::{AgentKind, McpConfig};
use harness_core::git::HARNESS_DIR;
use harness_core::mcp::registry::Entry;
use harness_core::mcp::tools::Tool;
use harness_core::mcp::McpServer;
use harness_core::models::ModelList;
use harness_core::plugins;
use harness_core::skills::SKILLS_DIR;
use harness_core::task::handoff::Role;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;

mod app;
mod tabs;
mod ui;

use crate::ui::message::Message;
use harness_agents::install::tools::ToolStatus;
use tabs::agents::{
    AgentChecker, AgentsTab, Installer, JobEvent, ReleaseChecker, ToolChecker, Updater,
};
use tabs::mcp::McpTab;
use tabs::plugins::PluginsTab;
use tabs::projects::picker::Browser;
use tabs::projects::ProjectsTab;
use tabs::retro::{RetroBuilder, RetroTab};
use tabs::roles::RolesTab;
use tabs::skills::SkillsTab;
use tabs::tasks::runner::Builder;
use tabs::tasks::TasksTab;
use ui::i18n::I18n;
use ui::{Form, Hits, Target};

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
    ApplyUpdate(Box<plugins::ops::Prepared>),
    /// Make a retrospective of the open project.
    GenerateRetro,
    /// Apply these proposals of the retrospective in this folder.
    ApplyProposals(PathBuf, Vec<u32>),
    /// Run this maker's command to install or update the agent named.
    RunAgentCommand {
        name: &'static str,
        action: harness_agents::install::catalog::Action,
        command: String,
    },
    /// Install the newer harness in the place of this one.
    UpdateHarness,
}

/// What a download in the background brought.
#[derive(Debug)]
enum PluginJob {
    CatalogAdded(Result<String, String>),
    CatalogUpdated(String, Result<plugins::ops::CatalogUpdate, String>),
    Added {
        name: String,
        give: Option<Role>,
        result: Result<plugins::ops::Added, String>,
    },
    UpdateReady(String, Result<Option<plugins::ops::Prepared>, String>),
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
    app::terminal::check_size()?;
    // First of all: from here on, closing the window no longer kills the TUI
    // while an agent runs; the app is closed normally instead.
    let stop = app::terminal::watch_stop_signals().context("cannot listen for stop signals")?;
    let mut app = App::new(projects::harness_home(), start);
    app.native = true;
    app.splash.open = app.splash.at_start;
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
    let result = app::terminal::event_loop(&mut terminal, &mut app, &stop);
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Each agent's answer: its model list, or why there is none.
type Answers = Vec<(AgentKind, Result<ModelList, String>)>;

/// Asks the agents with a saved login (in `credentials_dir`) for their models.
type ModelAsker = fn(&Path) -> Answers;

fn ask_agents(credentials_dir: &Path) -> Answers {
    let programs = harness_agents::install::models::Programs::default();
    harness_agents::install::models::ask_all(credentials_dir, &programs)
}

/// Starts an MCP server in the project folder and asks it for its tools.
type McpChecker = fn(&McpServer, &Path) -> Result<Vec<Tool>, String>;

/// The form for a new or changed MCP server.
fn server_form(tr: &I18n, title: &str, text: &str, name: &str, server: &McpConfig) -> Form {
    let (command, variables, sign_in) = tabs::mcp::form_values(server);
    Form::new(title, text, tr.t("mcp.ok"))
        .field(tr.t("mcp.name"), name)
        .field(tr.t("mcp.command_field"), &command)
        .field(tr.t("mcp.variables_field"), &variables)
        .check(tr.t("mcp.sign_in_field"), sign_in)
}

/// Searches the MCP registry.
type McpSearcher = fn(&str) -> Result<Vec<Entry>, String>;

/// Signs in to a web MCP server in the browser: credentials folder, server
/// name, address.
type McpSigner = fn(&Path, &str, &str) -> Result<(), String>;

fn sign_in(credentials_dir: &Path, name: &str, url: &str) -> Result<(), String> {
    use harness_agents::mcp::oauth;
    let tools = oauth::Tools {
        curl: "curl".into(),
        open: &oauth::open_in_browser,
        browser_limit: oauth::BROWSER_LIMIT,
    };
    oauth::login(credentials_dir, name, url, &tools)
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
    agent_check: Option<mpsc::Receiver<Vec<harness_agents::install::catalog::Status>>>,
    /// Checks the other programs the harness needs (Git, Node.js, ...);
    /// tests give a fake one.
    tool_checker: ToolChecker,
    /// The answer of that check, while it runs.
    tool_check: Option<mpsc::Receiver<Vec<ToolStatus>>>,
    /// Runs an agent's install or update command; tests give a fake one.
    installer: Installer,
    /// What that command prints, while it runs.
    install_events: Option<mpsc::Receiver<JobEvent>>,
    /// Asks whether a newer harness is out; tests give a fake one.
    release_checker: ReleaseChecker,
    /// The answer of that check, while it runs.
    release_check: Option<mpsc::Receiver<Result<Option<String>, String>>>,
    /// Installs the newer harness; tests give a fake one.
    updater: Updater,
    form: Option<(Purpose, Form)>,
    /// The folder browser, when the system has no folder dialog.
    browser: Option<(Pick, Browser)>,
    /// Use the system's folder dialog (`kdialog`, `zenity`) when there is one.
    native: bool,
    /// Where choosing a folder starts: `~/code` if it exists.
    start_dir: PathBuf,
    /// The last result or problem, shown at the bottom.
    message: Option<Message>,
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
    /// The start window is open (see `app/splash.rs`).
    splash: app::splash::Splash,
    /// `q` was pressed once with unsaved changes.
    quit_warned: bool,
    quit: bool,
}

/// The project's file of skill `name`.
fn skill_path(root: &Path, name: &str) -> PathBuf {
    root.join(HARNESS_DIR)
        .join(SKILLS_DIR)
        .join(format!("{name}.md"))
}

#[cfg(test)]
mod tests;
