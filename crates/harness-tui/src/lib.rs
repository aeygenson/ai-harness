//! `harness tui`: the harness in a full-screen terminal window, with tabs.
//!
//! ```text
//!  harness-test │ 1 Tasks │ 2 Roles │ 3 Skills │ … │ 7 Projects   [ + New project ] [ EN ]
//! ┌ Tasks ─────────────┐┌ task-001 · round 2 of 5 · done ──────────────────────────┐
//! │> task-001  done    ││  r1 architect  approved → human    Design ready          │
//! ...
//!  click or 1–7 tabs · ↑↓ select · wheel scroll · … · L language · q quit
//! ```
//!
//! Everything works with the mouse (click, double click, wheel) and with the
//! keyboard. The terminal's own text selection works with Shift held down.
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
use harness_core::mcp_tools::{self, Tool, ToolList};
use harness_core::models::{self, ModelList};
use harness_core::projects::{self, name_of};
use harness_core::skills::{self, SKILLS_DIR};
use harness_core::{config_edit, plugin_ops, settings};
use ratatui::crossterm::cursor::Hide;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{enable_raw_mode, Clear, ClearType, EnterAlternateScreen};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::prelude::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame, Terminal};

mod editor;
mod i18n;
mod keys;
mod mcp_tab;
mod picker;
mod plugin_catalog;
mod plugins_tab;
mod projects_tab;
mod roles_tab;
mod runner;
mod skills_tab;
mod tasks;
mod ui;

use i18n::I18n;
use mcp_tab::McpTab;
use picker::{Browser, Native};
use plugins_tab::PluginsTab;
use projects_tab::{has_config, ProjectsTab};
use roles_tab::{Action, RolesTab};
use runner::Builder;
use skills_tab::SkillsTab;
use tasks::{Menu, TasksTab};
use ui::{buttons, panel, ButtonId, Form, Hits, ListId, Target};

/// How often the open project is read again.
const RELOAD_EVERY: Duration = Duration::from_secs(3);
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
}

/// The tabs in order: the key of the label, and the step of the plan that
/// brings each one.
const TABS: [(Tab, &str, u8); 7] = [
    (Tab::Tasks, "tabs.tasks", 5),
    (Tab::Roles, "tabs.roles", 2),
    (Tab::Skills, "tabs.skills", 3),
    (Tab::Mcp, "tabs.mcp", 3),
    (Tab::Plugins, "tabs.plugins", 4),
    (Tab::Retro, "tabs.retro", 6),
    (Tab::Projects, "tabs.projects", 1),
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

/// A skill file, or a plugin folder, waiting to be opened in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EditJob {
    name: String,
    path: PathBuf,
    /// The file is a fresh copy of the built-in skill: if Lisa changes
    /// nothing, it is deleted again.
    copied: bool,
    /// The folder of the plugin `name`, not a skill.
    plugin: bool,
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
    let mut terminal = ratatui::init();
    // Give the mouse back to the terminal even if the program panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));
    // A pasted text arrives as one event, so its line breaks do not send it.
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut loaded = Instant::now();
    while !app.quit {
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
        app.tick();
        if let Some(job) = app.edit.take() {
            let result = edit_outside(terminal, &job.path, &app.tr);
            app.finish_edit(&job, result);
        }
        if loaded.elapsed() >= RELOAD_EVERY {
            if let Some(tasks) = &mut app.tasks {
                tasks.reload();
            }
            loaded = Instant::now();
        }
    }
    Ok(())
}

/// Gives the terminal to the editor and takes it back when the editor is closed.
fn edit_outside(terminal: &mut DefaultTerminal, file: &Path, tr: &I18n) -> Result<(), String> {
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    println!("{}", tr.f("skills.editing", &[("path", &file.display())]));
    let result = editor::run(file);
    let _ = enable_raw_mode();
    let _ = execute!(
        io::stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All),
        Hide,
        EnableMouseCapture,
        EnableBracketedPaste
    );
    // A new terminal draws everything again. (`Terminal::clear` would ask the
    // terminal where its cursor is, and not every terminal answers.)
    if let Ok(fresh) = Terminal::new(CrosstermBackend::new(io::stdout())) {
        *terminal = fresh;
    }
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
    /// A skill to open in the editor after this event.
    edit: Option<EditJob>,
    projects: ProjectsTab,
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
    asker: ModelAsker,
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
            edit: None,
            projects: ProjectsTab::load(home),
            form: None,
            browser: None,
            native: false,
            start_dir: default_start_dir(),
            message: None,
            hits: Hits::default(),
            last_click: None,
            builder: harness_agents::build::build_team,
            asker: ask_agents,
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
        let roles = RolesTab::load(root, self.home.as_deref());
        self.plugins = Some(PluginsTab::load(root, &roles));
        self.roles = Some(roles);
        self.skills = Some(SkillsTab::load(root));
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
        }
        running
    }

    /// Takes what the background work sent.
    fn tick(&mut self) {
        let job = self.plugin_job.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(done) => Some(Some(done)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        });
        if let Some(done) = job {
            self.plugin_job = None;
            if let Some(plugins) = &mut self.plugins {
                plugins.busy = None;
            }
            match done {
                Some(done) => self.plugin_job_done(done),
                None => self.message = Some(("the download stopped".into(), true)),
            }
        }
        let answers = self.asking.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(answers) => Some(Some(answers)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        });
        if let Some(answers) = answers {
            self.asking = None;
            self.models_answered(answers.unwrap_or_default());
        }
        let checked = self
            .checking
            .as_ref()
            .and_then(|(_, _, rx)| match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("the check stopped".into())),
            });
        if let Some(answer) = checked {
            if let Some((name, server, _)) = self.checking.take() {
                self.mcp_checked(&name, &server, answer);
            }
        }
        let found = self.searching.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("the search stopped".into())),
        });
        if let Some(answer) = found {
            self.searching = None;
            if let Some(catalog) = self.mcp.as_mut().and_then(|m| m.catalog.as_mut()) {
                catalog.found(answer);
            }
        }
        let signed = self
            .signing
            .as_ref()
            .and_then(|(_, rx)| match rx.try_recv() {
                Ok(answer) => Some(answer),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("the sign-in stopped".into())),
            });
        if let Some(answer) = signed {
            if let Some((name, _)) = self.signing.take() {
                if let Some(mcp) = &mut self.mcp {
                    mcp.signing = None;
                }
                self.message = Some(match answer {
                    Ok(()) => (self.tr.f("mcp.signed_in_as", &[("name", &name)]), false),
                    Err(error) => (error, true),
                });
            }
        }
        if let Some(tasks) = &mut self.tasks {
            if let Some(message) = tasks.tick(&self.tr) {
                self.message = Some(message);
            }
        }
    }

    fn on_paste(&mut self, text: &str) {
        if let Some((_, form)) = &mut self.form {
            text.chars()
                .filter(|c| !c.is_control())
                .for_each(|c| form.type_char(c));
        } else if let Some(name) = self.browser.as_mut().and_then(|(_, b)| b.naming.as_mut()) {
            name.extend(text.chars().filter(|c| !c.is_control()));
        } else if self.tab == Tab::Tasks && self.browser.is_none() {
            if let Some(tasks) = &mut self.tasks {
                tasks.paste(text);
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        let quit_warned = std::mem::take(&mut self.quit_warned);
        if self.browser.is_some() {
            self.browser_key(key.code);
            return;
        }
        if let Some((_, form)) = &mut self.form {
            match key.code {
                KeyCode::Esc => self.close_form(),
                KeyCode::Enter => self.submit(),
                KeyCode::Tab | KeyCode::Down => form.next_field(),
                KeyCode::Backspace => form.backspace(),
                KeyCode::Char(c) => form.type_char(c),
                _ => {}
            }
            return;
        }
        // Writing the message: every key is text, except these.
        if self.tab == Tab::Tasks {
            if let Some(tasks) = self.tasks.as_mut().filter(|t| t.typing()) {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match (key.code, keys::latin(key.code)) {
                    // Enter is a new line; these send.
                    (_, KeyCode::Char('s')) if control => self.press(ButtonId::Send),
                    (KeyCode::Enter, _) if control || alt => self.press(ButtonId::Send),
                    // Other Ctrl and Alt keys are not text.
                    _ if control || alt => {}
                    (code, _) => tasks.on_key(code),
                }
                return;
            }
        }
        // Hot keys work with a Russian keyboard layout too: «й» is q.
        let code = keys::latin(key.code);
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        let running = self.tasks.as_ref().is_some_and(TasksTab::is_running);
        // A message written but not sent is not thrown away by one key.
        let unsent = self
            .tasks
            .as_ref()
            .is_some_and(|t| !t.input.trim().is_empty());
        match code {
            KeyCode::Esc if self.tasks.as_ref().is_some_and(|t| t.menu.is_some()) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.menu = None;
                }
            }
            KeyCode::Char('q') | KeyCode::Esc if running => {
                self.message = Some((self.tr.t("tasks.quit_running").to_string(), true));
            }
            KeyCode::Esc
                if self.tab == Tab::Roles
                    && self.roles.as_ref().is_some_and(RolesTab::in_details) =>
            {
                if let Some(roles) = &mut self.roles {
                    roles.on_key(KeyCode::Esc, &self.tr);
                }
            }
            KeyCode::Esc
                if self.tab == Tab::Mcp && self.mcp.as_ref().is_some_and(McpTab::in_catalog) =>
            {
                if let Some(mcp) = &mut self.mcp {
                    mcp.catalog = None;
                }
            }
            KeyCode::Esc
                if self.tab == Tab::Plugins
                    && self.plugins.as_ref().is_some_and(PluginsTab::in_catalog) =>
            {
                if let Some(plugins) = &mut self.plugins {
                    plugins.catalog_key(KeyCode::Esc);
                }
            }
            KeyCode::Char('q') | KeyCode::Esc if unsaved && !quit_warned => {
                self.quit_warned = true;
                self.message = Some((self.tr.t("roles.unsaved_quit").to_string(), true));
            }
            KeyCode::Char('q') | KeyCode::Esc if unsent && !quit_warned => {
                self.quit_warned = true;
                self.message = Some((self.tr.t("tasks.unsent_quit").to_string(), true));
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('L') | KeyCode::F(2) => self.press(ButtonId::Language),
            KeyCode::Char(c @ '1'..='7') => {
                let index = usize::from(c as u8 - b'1');
                self.tab = TABS[index].0;
            }
            KeyCode::Char('r') | KeyCode::F(5) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.reload();
                }
                // Unsaved changes are not thrown away by a reload.
                if let Some(roles) = self.roles.as_mut().filter(|r| !r.changed()) {
                    roles.reload();
                }
                if let Some(skills) = &mut self.skills {
                    skills.reload();
                }
                if let Some(mcp) = &mut self.mcp {
                    mcp.reload();
                }
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                    plugins.reload(roles);
                }
                self.reload_catalog_views();
                self.projects.reload();
            }
            code => match self.tab {
                Tab::Tasks => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_key(code);
                    }
                }
                Tab::Roles => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    code => {
                        if let Some(roles) = &mut self.roles {
                            let action = roles.on_key(code, &self.tr);
                            self.act(action);
                        }
                    }
                },
                Tab::Skills => {
                    if let Some(skills) = &mut self.skills {
                        let action = skills.on_key(code);
                        self.skill_action(action);
                    }
                }
                Tab::Mcp if self.mcp.as_ref().is_some_and(McpTab::in_catalog) => {
                    if let Some(mcp) = &mut self.mcp {
                        let action = mcp.catalog_key(code);
                        self.mcp_action(action);
                    }
                }
                Tab::Mcp => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    KeyCode::Char(' ') | KeyCode::Enter => self.press(ButtonId::McpToggle),
                    KeyCode::Char('c') => self.press(ButtonId::McpCheck),
                    code => {
                        if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                            let action = mcp.on_key(code, roles);
                            self.mcp_action(action);
                        }
                    }
                },
                Tab::Plugins if self.plugins.as_ref().is_some_and(PluginsTab::in_catalog) => {
                    if let Some(plugins) = &mut self.plugins {
                        let action = plugins.catalog_key(code);
                        self.plugin_action(action);
                    }
                }
                Tab::Plugins => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    KeyCode::Char(' ') | KeyCode::Enter => self.press(ButtonId::PluginToggle),
                    code => {
                        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                            let action = plugins.on_key(code, roles);
                            self.plugin_action(action);
                        }
                    }
                },
                Tab::Projects => match code {
                    KeyCode::Enter => self.press(ButtonId::UseProject),
                    KeyCode::Char('n') => self.press(ButtonId::NewProject),
                    KeyCode::Char('o') => self.press(ButtonId::OpenFolder),
                    KeyCode::Delete => self.press(ButtonId::RemoveProject),
                    code => self.projects.on_key(code),
                },
                _ => {}
            },
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let hit = self.hits.at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let now = Instant::now();
                let double = matches!(
                    (self.last_click, hit),
                    (Some((at, target, row)), Some((t, r)))
                        if target == t && row == r && now - at < DOUBLE_CLICK
                );
                self.last_click = hit.map(|(target, row)| (now, target, row));
                self.click(hit, double);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let down = mouse.kind == MouseEventKind::ScrollDown;
                if let Some((_, browser)) = &mut self.browser {
                    browser.move_by(if down { 1 } else { -1 });
                    return;
                }
                if self.form.is_some() {
                    return;
                }
                match (self.tab, hit.and_then(|(t, r)| Hits::row(t, r))) {
                    (Tab::Tasks, Some((list, _))) => {
                        if let Some(tasks) = &mut self.tasks {
                            tasks.on_click_list(list);
                            tasks.on_key(if down { KeyCode::Down } else { KeyCode::Up });
                        }
                    }
                    (Tab::Tasks, None) => {
                        if let Some(tasks) = &mut self.tasks {
                            tasks.on_wheel(down);
                        }
                    }
                    (Tab::Projects, _) => {
                        self.projects
                            .on_key(if down { KeyCode::Down } else { KeyCode::Up });
                    }
                    (Tab::Roles, Some((ListId::Roles, _))) => {
                        if let Some(roles) = &mut self.roles {
                            let next = if down {
                                roles.selected + 1
                            } else {
                                roles.selected.saturating_sub(1)
                            };
                            roles.select(next);
                        }
                    }
                    (Tab::Roles, _) => {
                        if let Some(roles) = &mut self.roles {
                            roles.on_wheel(down);
                        }
                    }
                    (Tab::Skills, Some((ListId::Skills, _))) => {
                        if let Some(skills) = &mut self.skills {
                            skills.move_by(if down { 1 } else { -1 });
                        }
                    }
                    (Tab::Skills, _) => {
                        if let Some(skills) = &mut self.skills {
                            skills.on_wheel(down);
                        }
                    }
                    (Tab::Mcp, _) => {
                        if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                            if mcp.in_catalog() {
                                mcp.move_found(if down { 1 } else { -1 });
                            } else {
                                mcp.move_by(if down { 1 } else { -1 }, roles);
                            }
                        }
                    }
                    (Tab::Plugins, _) => {
                        let delta = if down { 1 } else { -1 };
                        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                            if let Some(view) = &mut plugins.catalogs {
                                view.move_by(delta);
                            } else if let Some(view) = &mut plugins.catalog {
                                view.move_by(delta);
                            } else {
                                plugins.move_by(delta, roles);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, hit: Option<(Target, u16)>, double: bool) {
        // An open list of the message box closes with any click; a click on
        // it chooses.
        if let Some((tasks, menu)) = self
            .tasks
            .as_mut()
            .and_then(|t| t.menu.take().map(|menu| (t, menu)))
        {
            if let Some((
                target @ Target::List {
                    list: ListId::Choices,
                    ..
                },
                row,
            )) = hit
            {
                if let Some((_, index)) = Hits::row(target, row) {
                    if !tasks.choose(menu, index) {
                        self.message =
                            Some((self.tr.t("tasks.choice_unavailable").to_string(), true));
                    }
                }
            }
            if !matches!(
                hit,
                Some((Target::Button(ButtonId::Input | ButtonId::Send), _))
            ) {
                return;
            }
        }
        if let Some((_, browser)) = &mut self.browser {
            match hit {
                Some((Target::Button(id), _)) => self.press(id),
                Some((target @ Target::List { .. }, row)) => {
                    if let Some((_, index)) = Hits::row(target, row) {
                        browser.mark(index);
                        if double {
                            browser.open_selected();
                        }
                    }
                }
                Some((Target::Field(_), _)) => {}
                // A click outside the window closes it.
                _ => self.browser = None,
            }
            return;
        }
        if let Some((_, form)) = &mut self.form {
            match hit {
                Some((Target::Button(ButtonId::Ok), _)) => self.submit(),
                Some((Target::Button(ButtonId::Cancel), _)) => self.close_form(),
                Some((Target::Field(i), _)) if i < form.fields.len() => form.focus = i,
                Some((Target::Field(_), _)) => {}
                // A click outside the window closes it.
                _ => self.close_form(),
            }
            return;
        }
        let Some((target, row)) = hit else {
            return;
        };
        match target {
            Target::Tab(index) => self.tab = TABS[index].0,
            Target::Button(id) => self.press(id),
            Target::List { .. } => match Hits::row(target, row) {
                Some((ListId::Projects, index)) => {
                    if index < self.projects.list.projects.len() {
                        self.projects.selected = index;
                        if double {
                            self.press(ButtonId::UseProject);
                        }
                    }
                }
                Some((ListId::Roles, index)) => {
                    if let Some(roles) = &mut self.roles {
                        roles.select(index);
                    }
                }
                Some((ListId::Skills, index)) => {
                    if let Some(skills) = &mut self.skills {
                        skills.select(index);
                    }
                    if double {
                        self.press(ButtonId::SkillEdit);
                    }
                }
                Some((ListId::Mcp, index)) => {
                    if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                        mcp.select(index, roles);
                    }
                    if double {
                        self.press(ButtonId::McpToggle);
                    }
                }
                Some((ListId::Plugins, index)) => {
                    if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                        plugins.select(index, roles);
                    }
                    if double {
                        self.press(ButtonId::PluginToggle);
                    }
                }
                Some((ListId::PluginCatalog, index)) => {
                    if let Some(view) = self.plugins.as_mut().and_then(|p| p.catalog.as_mut()) {
                        view.select(index);
                    }
                    if double {
                        self.press(ButtonId::PluginAdd);
                    }
                }
                Some((ListId::PluginCatalogs, index)) => {
                    if let Some(view) = self.plugins.as_mut().and_then(|p| p.catalogs.as_mut()) {
                        view.select(index);
                    }
                }
                Some((ListId::McpCatalog, index)) => {
                    if let Some(mcp) = &mut self.mcp {
                        mcp.select_found(index);
                    }
                    if double {
                        self.press(ButtonId::McpUse);
                    }
                }
                Some((ListId::RoleFilter, index)) => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.toggle_filter(index);
                    }
                }
                Some((list, index)) => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_click(list, index);
                    }
                }
                None => {}
            },
            Target::Row(index) => {
                if let Some(roles) = &mut self.roles {
                    let action = roles.activate(index, &self.tr);
                    self.act(action);
                }
            }
            Target::Field(_) => {}
        }
    }

    /// What the Roles tab asks for after a click or a key.
    fn act(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Say(text) => self.message = Some((text, false)),
            Action::EditModel(model) => {
                let tr = &self.tr;
                self.form = Some((
                    Purpose::Model,
                    Form::new(
                        tr.t("roles.model_title"),
                        tr.t("roles.model_text"),
                        tr.t("roles.model_ok"),
                    )
                    .field(tr.t("roles.model_field"), &model),
                ));
            }
        }
    }

    /// «Check»: the server starts in the background and is asked for its
    /// tools, with its secrets as a run would give them.
    fn check_mcp(&mut self) {
        let (Some(root), Some(mcp), Some(roles)) = (&self.project, &mut self.mcp, &self.roles)
        else {
            return;
        };
        if self.checking.is_some() {
            return;
        }
        let Some(name) = mcp.current(roles) else {
            return;
        };
        let Some(config) = roles.servers().get(&name).cloned() else {
            return;
        };
        let secrets = self.home.as_ref().map(|h| h.join("credentials"));
        let server = harness_core::mcp::server(&name, &config, |secret| {
            harness_agents::mcp_oauth::mcp_secret(secrets.as_deref()?, secret)
        });
        // A web server is reached through this same program.
        let server = server.map(|server| match std::env::current_exe() {
            Ok(harness) => harness_agents::launcher::with_bridge(vec![server.clone()], &harness)
                .pop()
                .unwrap_or(server),
            Err(_) => server,
        });
        let server = match server {
            Ok(server) => server,
            Err(error) => {
                self.message = Some((error.to_string(), true));
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let (checker, root) = (self.checker, root.clone());
        std::thread::spawn(move || {
            let _ = tx.send(checker(&server, &root));
        });
        mcp.checking = Some(name.clone());
        self.message = Some((self.tr.f("mcp.checking", &[("name", &name)]), false));
        self.checking = Some((name, config, rx));
    }

    /// The server answered: keep its tools and say how many.
    fn mcp_checked(&mut self, name: &str, server: &McpConfig, answer: Result<Vec<Tool>, String>) {
        if let Some(mcp) = &mut self.mcp {
            mcp.checking = None;
        }
        self.message = Some(match answer {
            Ok(tools) => {
                let count = tools.len();
                let list = ToolList {
                    server: name.to_string(),
                    fetched: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs()),
                    tools,
                };
                match &self.home {
                    Some(home) => match mcp_tools::save(home, server, &list) {
                        Ok(()) => (
                            self.tr
                                .f("mcp.checked", &[("name", &name), ("count", &count)]),
                            false,
                        ),
                        Err(error) => (error.to_string(), true),
                    },
                    None => ("HOME is not set".into(), true),
                }
            }
            Err(error) => (error, true),
        });
    }

    /// Signs in to the web server `name` in the background: the browser
    /// opens, and the answer comes in `tick`.
    fn start_sign_in(&mut self, name: &str) {
        let url = self
            .roles
            .as_ref()
            .and_then(|r| r.servers().get(name))
            .and_then(|s| s.url.clone());
        let (Some(url), Some(home), Some(mcp)) = (url, &self.home, &mut self.mcp) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (signer, dir, server) = (self.signer, home.join("credentials"), name.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(signer(&dir, &server, url.trim()));
        });
        mcp.signing = Some(name.to_string());
        self.message = Some((self.tr.f("mcp.sign_in_started", &[("name", &name)]), false));
        self.signing = Some((name.to_string(), rx));
    }

    /// Asks the registry in the background; the catalog shows the answer.
    fn search_registry(&mut self, query: &str) {
        let Some(mcp) = &mut self.mcp else {
            return;
        };
        query.clone_into(&mut mcp.last_query);
        let Some(catalog) = &mut mcp.catalog else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (searcher, query) = (self.searcher, query.to_string());
        catalog.query.clone_from(&query);
        catalog.searching = true;
        catalog.error = None;
        std::thread::spawn(move || {
            let _ = tx.send(searcher(&query));
        });
        self.searching = Some(rx);
    }

    /// What the MCP tab asks for.
    fn mcp_action(&mut self, action: mcp_tab::Action) {
        use mcp_tab::Action as A;
        let tr = &self.tr;
        let servers = self.roles.as_ref().map(RolesTab::servers);
        self.form = match action {
            A::None => return,
            A::SignIn(name) => {
                self.start_sign_in(&name);
                return;
            }
            A::Unusable(why) => {
                self.message = Some((tr.f("mcp.cannot_use", &[("why", &why)]), true));
                return;
            }
            A::Search(query) => Some((
                Purpose::McpSearch,
                Form::new(
                    tr.t("mcp.search_title"),
                    tr.t("mcp.search_text"),
                    tr.t("mcp.search"),
                )
                .field(tr.t("mcp.search_field"), &query),
            )),
            A::Use(offer) => {
                // A name harness.toml does not use yet.
                let taken = |name: &str| servers.is_some_and(|s| s.contains_key(name));
                let name = std::iter::once(offer.name.clone())
                    .chain((2..).map(|n| format!("{}-{n}", offer.name)))
                    .find(|n| !taken(n))
                    .unwrap_or_default();
                let text = format!("{}\n{}", tr.t("mcp.from_catalog"), tr.t("mcp.form_text"));
                Some((
                    Purpose::McpServer(None),
                    server_form(tr, tr.t("mcp.new_title"), &text, &name, &offer.server),
                ))
            }
            A::New => Some((
                Purpose::McpServer(None),
                server_form(
                    tr,
                    tr.t("mcp.new_title"),
                    tr.t("mcp.form_text"),
                    "",
                    &McpConfig::default(),
                ),
            )),
            A::Edit(name) => {
                let server = servers.and_then(|s| s.get(&name));
                let old = server.map(|_| name.clone());
                let title = tr.f("mcp.edit_title", &[("name", &name)]);
                let empty = McpConfig::default();
                Some((
                    Purpose::McpServer(old),
                    server_form(
                        tr,
                        &title,
                        tr.t("mcp.form_text"),
                        &name,
                        server.unwrap_or(&empty),
                    ),
                ))
            }
            A::Remove(name) => {
                let text = tr.f("mcp.remove_text", &[("name", &name)]);
                Some((
                    Purpose::RemoveMcp(name),
                    Form::new(tr.t("mcp.remove_title"), &text, tr.t("mcp.remove")),
                ))
            }
            A::Secret(name) => Some((
                Purpose::Secret,
                Form::new(
                    tr.t("mcp.secret_title"),
                    tr.t("mcp.secret_text"),
                    tr.t("mcp.ok"),
                )
                .field(tr.t("mcp.secret_name"), &name)
                .secret(tr.t("mcp.secret_value")),
            )),
        };
        // The value is typed into the second field.
        if let Some((Purpose::Secret, form)) = &mut self.form {
            form.focus = 1;
        }
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

    /// OK in the server form.
    fn save_mcp(&mut self, old: Option<String>, form: &Form) -> Result<(), String> {
        let name = form.value(0).to_string();
        let server = mcp_tab::server_from(form.value(1), form.value(2), form.value(3))?;
        harness_core::mcp::check_server(&name, &server).map_err(|e| e.to_string())?;
        self.save_settings(|text| {
            config_edit::set_mcp(text, old.as_deref(), &name, &server).map_err(|e| e.to_string())
        })?;
        if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
            // A server from the catalog is now in the list.
            mcp.catalog = None;
            mcp.select_named(&name, roles);
        }
        self.message = Some((self.tr.f("mcp.saved", &[("name", &name)]), false));
        Ok(())
    }

    fn remove_mcp(&mut self, name: &str) -> Result<(), String> {
        self.save_settings(|text| config_edit::remove_mcp(text, name).map_err(|e| e.to_string()))?;
        self.message = Some((self.tr.f("mcp.removed", &[("name", &name)]), false));
        Ok(())
    }

    /// OK in the secret form: the value goes to a private file, nowhere else.
    fn save_secret(&mut self, form: &Form) -> Result<(), String> {
        let name = form.value(0).to_string();
        if !harness_core::mcp::is_simple_name(&name) {
            return Err(self.tr.f("mcp.bad_secret_name", &[("name", &name)]));
        }
        let value = form.value(1);
        if value.is_empty() {
            return Err(self.tr.t("mcp.empty_secret").to_string());
        }
        let Some(home) = &self.home else {
            return Err("HOME is not set".into());
        };
        credentials::save_secret(
            &home.join("credentials"),
            &name,
            &credentials::Secret::new(value),
        )
        .map_err(|e| e.to_string())?;
        if let Some(mcp) = &mut self.mcp {
            mcp.reload();
        }
        self.message = Some((self.tr.f("mcp.secret_saved_as", &[("name", &name)]), false));
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

    /// Runs `work` in the background; its result comes in `tick`.
    fn start_plugin_job(
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

    /// Plugins change harness.toml: unsaved changes of the roles would be
    /// lost, so they are saved or undone first.
    fn roles_unsaved(&mut self) -> bool {
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        if unsaved {
            self.message = Some((self.tr.t("mcp.save_first").to_string(), true));
        }
        unsaved
    }

    /// «From catalog»; with no catalog at all, the official one is added
    /// by itself.
    fn open_plugin_catalog(&mut self) {
        let agent = match (&self.plugins, &self.roles) {
            (Some(plugins), Some(roles)) => roles
                .settings(plugins.role())
                .map(|s| harness_core::plugins::family(&s.agent).to_string())
                .unwrap_or_default(),
            _ => return,
        };
        let view = plugin_catalog::CatalogView::load(self.home.as_deref(), &agent);
        let none = self
            .home
            .as_deref()
            .is_some_and(|home| plugin_ops::catalogs(home).is_ok_and(|c| c.is_empty()));
        if let Some(plugins) = &mut self.plugins {
            plugins.catalog = Some(view);
        }
        if none {
            self.start_catalog_add(self.official_catalog.clone());
        }
    }

    fn start_catalog_add(&mut self, source: String) {
        let Some(home) = self.home.clone() else {
            return;
        };
        let label = self
            .tr
            .f("plugins.downloading_catalog", &[("source", &source)]);
        let added = self.tr.t("plugins.catalog_added").to_string();
        self.start_plugin_job(label, move || {
            let result = plugin_ops::add_catalog(&home, &source, None)
                .map(|catalog| {
                    added
                        .replace("{name}", &catalog.name)
                        .replace("{count}", &catalog.entries.len().to_string())
                })
                .map_err(|e| e.to_string());
            PluginJob::CatalogAdded(result)
        });
    }

    /// «Add»: the plugin is downloaded and copied in the background.
    fn add_plugin(&mut self, entry: harness_core::catalog::Entry, give: bool) {
        if self.roles_unsaved() {
            return;
        }
        let (Some(root), Some(home), Some(plugins)) =
            (self.project.clone(), self.home.clone(), &self.plugins)
        else {
            return;
        };
        let give = give.then(|| plugins.role());
        let name = entry.name.clone();
        let label = self.tr.f("plugins.adding", &[("name", &name)]);
        self.start_plugin_job(label, move || {
            let result = Repo::open(&root)
                .map_err(|e| e.to_string())
                .and_then(|repo| {
                    plugin_ops::add(&repo, &home, &entry, None, false, false)
                        .map_err(|e| e.to_string())
                });
            PluginJob::Added { name, give, result }
        });
    }

    /// «Update»: the newest version is downloaded in the background, then
    /// shown before it is taken.
    fn prepare_plugin_update(&mut self, name: String) {
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
                    plugin_ops::prepare_update(&repo, &home, &name).map_err(|e| e.to_string())
                });
            PluginJob::UpdateReady(name, result)
        });
    }

    fn apply_plugin_update(&mut self, prepared: &plugin_ops::Prepared) {
        let result = self
            .project
            .as_deref()
            .ok_or_else(String::new)
            .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
            .and_then(|repo| plugin_ops::apply_update(&repo, prepared).map_err(|e| e.to_string()));
        self.message = Some(match result {
            Ok(()) => (
                self.tr.f("plugins.updated", &[("name", &prepared.name)]),
                false,
            ),
            Err(error) => {
                plugin_ops::discard(prepared);
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

    /// The roles and plugins are read again after harness.toml changed.
    fn reload_plugins(&mut self) {
        if let Some(roles) = &mut self.roles {
            roles.reload();
            if let Some(plugins) = &mut self.plugins {
                plugins.reload(roles);
            }
        }
        if let Some(tasks) = &mut self.tasks {
            tasks.reload();
        }
    }

    fn reload_catalog_views(&mut self) {
        let home = self.home.clone();
        if let Some(plugins) = &mut self.plugins {
            if let Some(view) = &mut plugins.catalog {
                view.reload(home.as_deref());
            }
            if let Some(view) = &mut plugins.catalogs {
                view.reload(home.as_deref());
            }
        }
    }

    /// Gives the plugin to the role and saves it at once.
    fn give_plugin(&mut self, name: &str, role: Role) {
        let Some(mut settings) = self.roles.as_ref().and_then(|r| r.settings(role)).cloned() else {
            return;
        };
        if !settings.plugins.iter().any(|n| n == name) {
            settings.plugins.push(name.to_string());
        }
        let result = self.save_settings(|text| {
            config_edit::set_role(text, role, &settings).map_err(|e| e.to_string())
        });
        let role = skills_tab::role_name(role);
        self.message = Some(match result {
            Ok(()) => (
                self.tr
                    .f("plugins.given", &[("name", &name), ("role", &role)]),
                false,
            ),
            Err(error) => (error, true),
        });
    }

    /// A download in the background finished.
    fn plugin_job_done(&mut self, done: PluginJob) {
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
                    Ok(plugin_ops::CatalogUpdate::Local) => {
                        (tr.f("plugins.catalog_local", &[("name", &name)]), false)
                    }
                    Ok(plugin_ops::CatalogUpdate::Same(_)) => {
                        (tr.f("plugins.catalog_same", &[("name", &name)]), false)
                    }
                    Ok(plugin_ops::CatalogUpdate::Updated(_)) => {
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

    /// What the Plugins tab asks for.
    fn plugin_action(&mut self, action: plugins_tab::Action) {
        use plugins_tab::Action as A;
        let tr = &self.tr;
        match action {
            A::None => {}
            A::Remove(name) => {
                let text = tr.f("plugins.remove_text", &[("name", &name)]);
                self.form = Some((
                    Purpose::RemovePlugin(name),
                    Form::new(tr.t("plugins.remove_title"), &text, tr.t("plugins.remove")),
                ));
            }
            A::Allow {
                name,
                hooks,
                servers,
            } => {
                let old = self.roles.as_ref().and_then(|r| r.plugins().get(&name));
                let more =
                    old.is_none_or(|p| (hooks && !p.allow_hooks) || (servers && !p.allow_mcp));
                if !more {
                    // Forbidding needs no question; it fails while a role
                    // still has the plugin.
                    if self.allow_plugin(&name, hooks, servers).is_err() {
                        let text = self.tr.f("plugins.forbid_used", &[("name", &name)]);
                        self.message = Some((text, true));
                    }
                    return;
                }
                let key = if old.is_some_and(|p| hooks && !p.allow_hooks) {
                    "plugins.allow_hooks_text"
                } else {
                    "plugins.allow_servers_text"
                };
                let text = tr.f(key, &[("name", &name)]);
                self.form = Some((
                    Purpose::AllowPlugin {
                        name,
                        hooks,
                        servers,
                        give: None,
                    },
                    Form::new(tr.t("plugins.allow_title"), &text, tr.t("plugins.allow")),
                ));
            }
            A::OpenCatalog => self.open_plugin_catalog(),
            A::Search(query) => {
                self.form = Some((
                    Purpose::PluginSearch,
                    Form::new(
                        tr.t("plugins.search_title"),
                        tr.t("plugins.search_text"),
                        tr.t("plugins.search"),
                    )
                    .field(tr.t("plugins.search_field"), &query),
                ));
            }
            A::Unusable(why) => {
                self.message = Some((tr.f("plugins.cannot_add", &[("why", &why)]), true));
            }
            A::Add { entry, give } => self.add_plugin(entry, give),
            A::Update(name) => self.prepare_plugin_update(name),
            A::OpenCatalogs => {
                let view = plugin_catalog::CatalogsView::load(self.home.as_deref());
                if let Some(plugins) = &mut self.plugins {
                    plugins.catalogs = Some(view);
                }
            }
            A::AddCatalog(offered) => {
                self.form = Some((
                    Purpose::AddCatalog,
                    Form::new(
                        tr.t("plugins.add_catalog_title"),
                        tr.t("plugins.add_catalog_text"),
                        tr.t("plugins.add"),
                    )
                    .field(tr.t("plugins.catalog_field"), &offered),
                ));
            }
            A::UpdateCatalog(name) => {
                let Some(home) = self.home.clone() else {
                    return;
                };
                let label = tr.f("plugins.updating_catalog", &[("name", &name)]);
                self.start_plugin_job(label, move || {
                    let result =
                        plugin_ops::update_catalog(&home, &name).map_err(|e| e.to_string());
                    PluginJob::CatalogUpdated(name, result)
                });
            }
            A::RemoveCatalog(name) => {
                let text = tr.f("plugins.remove_catalog_text", &[("name", &name)]);
                self.form = Some((
                    Purpose::RemoveCatalog(name),
                    Form::new(
                        tr.t("plugins.remove_catalog_title"),
                        &text,
                        tr.t("plugins.remove"),
                    ),
                ));
            }
            A::Open(name) => {
                let path = self.plugins.as_ref().and_then(|tab| {
                    let plugin = self.roles.as_ref()?.plugins().get(&name)?;
                    Some(tab.folder(&name, plugin))
                });
                match path {
                    Some(path) if path.is_dir() => {
                        self.edit = Some(EditJob {
                            name,
                            path,
                            copied: false,
                            plugin: true,
                        });
                    }
                    Some(path) => {
                        let text = tr.f("plugins.no_folder", &[("path", &path.display())]);
                        self.message = Some((text, true));
                    }
                    None => {}
                }
            }
        }
    }

    /// OK in «Remove the plugin»: its folder and settings go, in one commit.
    fn remove_plugin(&mut self, name: &str) -> Result<(), String> {
        let (Some(root), Some(roles)) = (self.project.clone(), &mut self.roles) else {
            return Err(self.tr.t("tabs.no_open_project").to_string());
        };
        if roles.changed() {
            return Err(self.tr.t("mcp.save_first").to_string());
        }
        let repo = Repo::open(&root).map_err(|e| e.to_string())?;
        plugin_ops::remove(&repo, name).map_err(|e| e.to_string())?;
        roles.reload();
        if let Some(plugins) = &mut self.plugins {
            plugins.reload(roles);
        }
        if let Some(tasks) = &mut self.tasks {
            tasks.reload();
        }
        self.message = Some((self.tr.f("plugins.removed", &[("name", &name)]), false));
        Ok(())
    }

    /// Writes what the plugin may run by itself; checked like «Save».
    fn allow_plugin(&mut self, name: &str, hooks: bool, servers: bool) -> Result<(), String> {
        self.save_settings(|text| {
            config_edit::set_plugin_allow(text, name, hooks, servers).map_err(|e| e.to_string())
        })?;
        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
            plugins.select_named(name, roles);
        }
        self.message = Some((self.tr.f("plugins.allow_saved", &[("name", &name)]), false));
        Ok(())
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
                    plugin: false,
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

    /// The editor was closed: keep the change in git, or drop a copy of a
    /// built-in skill that was not changed.
    fn finish_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        if job.plugin {
            self.finish_plugin_edit(job, result);
            return;
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

    /// The plugin's folder was open in the editor: keep what changed in git.
    fn finish_plugin_edit(&mut self, job: &EditJob, result: Result<(), String>) {
        let tr = &self.tr;
        let saved = self
            .project
            .as_deref()
            .ok_or_else(String::new)
            .and_then(|root| Repo::open(root).map_err(|e| e.to_string()))
            .and_then(|repo| {
                repo.commit_paths(&[&job.path], &format!("harness: plugin {}", job.name))
                    .map_err(|e| e.to_string())
            });
        self.message = Some(match (result, saved) {
            (Err(error), _) => (tr.f("skills.editor_failed", &[("error", &error)]), true),
            (_, Err(error)) => (error, true),
            (_, Ok(true)) => (tr.f("plugins.edited", &[("name", &job.name)]), false),
            (_, Ok(false)) => (tr.t("plugins.unchanged").to_string(), false),
        });
        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
            plugins.reload(roles);
        }
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
            plugin: false,
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

    /// A button, clicked or chosen with its key.
    fn press(&mut self, id: ButtonId) {
        match id {
            ButtonId::RefreshModels => self.ask_for_models(),
            ButtonId::McpRole(index) => {
                if let Some(mcp) = &mut self.mcp {
                    mcp.choose_role(index);
                }
            }
            ButtonId::McpCheck => self.check_mcp(),
            ButtonId::McpCatalog => {
                if let Some(mcp) = &mut self.mcp {
                    let action = mcp.open_catalog();
                    self.mcp_action(action);
                }
            }
            ButtonId::McpSearch | ButtonId::McpUse | ButtonId::McpBack => {
                if let Some(mcp) = &mut self.mcp {
                    let action = mcp.catalog_press(id);
                    self.mcp_action(action);
                }
            }
            ButtonId::McpNew
            | ButtonId::McpEdit
            | ButtonId::McpRemove
            | ButtonId::McpSecret
            | ButtonId::McpSignIn => {
                if let (Some(mcp), Some(roles)) = (&self.mcp, &self.roles) {
                    let action = mcp.press(id, roles);
                    self.mcp_action(action);
                }
            }
            ButtonId::McpToggle => {
                if let (Some(mcp), Some(roles)) = (&self.mcp, &mut self.roles) {
                    if let Err(key) = mcp.toggle(roles) {
                        self.message = Some((self.tr.t(key).to_string(), true));
                    }
                }
            }
            ButtonId::PluginRole(index) => {
                if let Some(plugins) = &mut self.plugins {
                    plugins.choose_role(index);
                }
            }
            ButtonId::PluginToggle => {
                if let (Some(plugins), Some(roles)) = (&self.plugins, &mut self.roles) {
                    if let Err(key) = plugins.toggle(roles) {
                        self.message = Some((self.tr.t(key).to_string(), true));
                    }
                }
            }
            ButtonId::PluginCatalog => self.plugin_action(plugins_tab::Action::OpenCatalog),
            ButtonId::PluginFilter(_)
            | ButtonId::PluginSearch
            | ButtonId::PluginAdd
            | ButtonId::PluginAddGive
            | ButtonId::PluginCatalogs
            | ButtonId::PluginBack
            | ButtonId::CatalogAdd
            | ButtonId::CatalogUpdate
            | ButtonId::CatalogRemove => {
                if let Some(plugins) = &mut self.plugins {
                    let action = plugins.catalog_press(id);
                    self.plugin_action(action);
                }
            }
            ButtonId::PluginHooks
            | ButtonId::PluginServers
            | ButtonId::PluginUpdate
            | ButtonId::PluginRemove
            | ButtonId::PluginOpen => {
                if let (Some(plugins), Some(roles)) = (&self.plugins, &self.roles) {
                    let action = plugins.press(id, roles);
                    self.plugin_action(action);
                }
            }
            ButtonId::SkillRole(_)
            | ButtonId::SkillEdit
            | ButtonId::SkillNew
            | ButtonId::SkillRestore => {
                if let Some(skills) = &mut self.skills {
                    let action = skills.press(id);
                    self.skill_action(action);
                }
            }
            ButtonId::Input => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.focus_input();
                }
            }
            ButtonId::To | ButtonId::Model | ButtonId::Level => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.toggle_menu(match id {
                        ButtonId::Model => Menu::Model,
                        ButtonId::Level => Menu::Level,
                        _ => Menu::To,
                    });
                }
            }
            ButtonId::Send => {
                if let Some(tasks) = &mut self.tasks {
                    self.message = Some(match tasks.send(self.builder, &self.tr) {
                        Ok(text) => (text, false),
                        Err(error) => (error, true),
                    });
                }
            }
            ButtonId::UseProject => {
                if let Some(project) = self.projects.current() {
                    let path = project.path.clone();
                    if has_config(&path) {
                        self.open(&path);
                    } else if path.is_dir() {
                        self.form = Some(self.init_form(&path));
                    } else {
                        let text = self.tr.f("projects.gone", &[("path", &path.display())]);
                        self.message = Some((text, true));
                    }
                }
            }
            ButtonId::NewProject => self.pick(Pick::NewProject),
            ButtonId::OpenFolder => self.pick(Pick::Open),
            ButtonId::Choose => {
                if let Some((pick, browser)) = self.browser.take() {
                    self.picked(pick, browser.chosen());
                }
            }
            ButtonId::Up => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.up();
                }
            }
            ButtonId::NewFolder => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.naming = Some(String::new());
                }
            }
            ButtonId::ToggleHidden => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.toggle_hidden();
                }
            }
            ButtonId::Language => {
                self.tr.next();
                // The last message was in the old language.
                self.message = None;
                if let Some(home) = &self.home {
                    if let Err(error) = self.tr.save(home) {
                        self.message = Some((error, true));
                    }
                }
            }
            ButtonId::RemoveProject => {
                if self.project.as_deref() == self.projects.current().map(|p| p.path.as_path())
                    && self.busy()
                {
                    return;
                }
                if let Some(project) = self.projects.current() {
                    let tr = &self.tr;
                    let text = tr.f(
                        "form.remove_text",
                        &[("name", &project.name), ("path", &project.path.display())],
                    );
                    self.form = Some((
                        Purpose::Remove(project.path.clone()),
                        Form::new(tr.t("form.remove_title"), &text, tr.t("form.remove")),
                    ));
                }
            }
            ButtonId::Save => {
                if let Some(roles) = &mut self.roles {
                    self.message = Some(match roles.save() {
                        Ok(()) => (self.tr.t("roles.saved").to_string(), false),
                        Err(error) => (error, true),
                    });
                    // The Tasks and Skills tabs show the agents and skills too.
                    if let Some(tasks) = &mut self.tasks {
                        tasks.reload();
                    }
                    if let Some(skills) = &mut self.skills {
                        skills.reload();
                    }
                }
            }
            ButtonId::Undo => {
                if let Some(roles) = &mut self.roles {
                    roles.undo();
                    self.message = None;
                }
            }
            ButtonId::Cancel => self.browser = None,
            ButtonId::Ok => {}
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
            picker::native_folder(&title, &self.start_dir)
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

    /// Keys while the folder browser is open.
    fn browser_key(&mut self, code: KeyCode) {
        let Some((_, browser)) = &mut self.browser else {
            return;
        };
        if let Some(name) = &mut browser.naming {
            match code {
                KeyCode::Esc => browser.naming = None,
                KeyCode::Enter => browser.create(&self.tr),
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c) => name.push(c),
                _ => {}
            }
            return;
        }
        match keys::latin(code) {
            KeyCode::Esc => self.browser = None,
            KeyCode::Up | KeyCode::Char('k') => browser.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => browser.move_by(1),
            KeyCode::PageUp => browser.move_by(-10),
            KeyCode::PageDown => browser.move_by(10),
            KeyCode::Enter | KeyCode::Right => browser.open_selected(),
            KeyCode::Backspace | KeyCode::Left => browser.up(),
            KeyCode::Char('n') => browser.naming = Some(String::new()),
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Char('c') => self.press(ButtonId::Choose),
            _ => {}
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
            Tab::Skills => match &self.skills {
                Some(skills) => skills.draw(frame, main, &mut self.hits, &self.tr),
                None => placeholder(
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
            Tab::Projects => {
                self.projects.draw(
                    frame,
                    main,
                    &mut self.hits,
                    self.project.as_deref(),
                    &self.tr,
                );
            }
            tab => {
                let (_, label, step) = TABS
                    .iter()
                    .find(|(t, _, _)| *t == tab)
                    .copied()
                    .unwrap_or(TABS[0]);
                placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t(label)),
                    &self.tr.f("tabs.coming", &[("step", &step)]),
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
            let color = if *error { Color::Red } else { Color::Green };
            spans.push(Span::styled(format!(" {text}  "), Style::new().fg(color)));
        } else if let Some(problem) = problem {
            spans.push(Span::styled(
                format!(" {problem}  "),
                Style::new().fg(Color::Red),
            ));
        }
        let typing = self.tab == Tab::Tasks && self.tasks.as_ref().is_some_and(TasksTab::typing);
        let hint = if typing {
            "tasks.typing_hint"
        } else {
            "footer.hint"
        };
        spans.push(Span::styled(
            format!(" {}", self.tr.t(hint)),
            Style::new().fg(Color::DarkGray),
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
        put(
            frame,
            format!(" {name} "),
            Style::new().add_modifier(Modifier::BOLD),
        );
        for (index, (tab, label, _)) in TABS.iter().enumerate() {
            put(frame, "│".into(), Style::new().fg(Color::DarkGray));
            let style = if *tab == self.tab {
                Style::new().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::new()
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
        let items = [
            (self.tr.t("tabs.new_project"), ButtonId::NewProject),
            (self.tr.label(), ButtonId::Language),
        ];
        let width: u16 = items
            .iter()
            .map(|(label, _)| u16::try_from(label.chars().count()).unwrap_or(0) + 5)
            .sum();
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
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
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
