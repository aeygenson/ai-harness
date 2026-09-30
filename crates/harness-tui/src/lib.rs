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

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use harness_core::projects::{self, name_of};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

mod i18n;
mod keys;
mod picker;
mod projects_tab;
mod roles_tab;
mod runner;
mod tasks;
mod ui;

use i18n::I18n;
use picker::{Browser, Native};
use projects_tab::{has_config, ProjectsTab};
use roles_tab::{Action, RolesTab};
use runner::Builder;
use tasks::TasksTab;
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
        if loaded.elapsed() >= RELOAD_EVERY {
            if let Some(tasks) = &mut app.tasks {
                tasks.reload();
            }
            loaded = Instant::now();
        }
    }
    Ok(())
}

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
            projects: ProjectsTab::load(home),
            form: None,
            browser: None,
            native: false,
            start_dir: default_start_dir(),
            message: None,
            hits: Hits::default(),
            last_click: None,
            builder: harness_agents::build::build_team,
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
        self.tasks = Some(TasksTab::load(root));
        self.roles = Some(RolesTab::load(root));
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
                KeyCode::Esc => self.form = None,
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
                match key.code {
                    KeyCode::Enter => self.press(ButtonId::Send),
                    code => tasks.on_key(code),
                }
                return;
            }
        }
        // Hot keys work with a Russian keyboard layout too: «й» is q.
        let code = keys::latin(key.code);
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        let running = self.tasks.as_ref().is_some_and(TasksTab::is_running);
        match code {
            KeyCode::Esc if self.tasks.as_ref().is_some_and(|t| t.menu) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.menu = false;
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
            KeyCode::Char('q') | KeyCode::Esc if unsaved && !quit_warned => {
                self.quit_warned = true;
                self.message = Some((self.tr.t("roles.unsaved_quit").to_string(), true));
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
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, hit: Option<(Target, u16)>, double: bool) {
        // The open «To» list closes with any click; a click on it chooses.
        if let Some(tasks) = self.tasks.as_mut().filter(|t| t.menu) {
            tasks.menu = false;
            if let Some((
                target @ Target::List {
                    list: ListId::Choices,
                    ..
                },
                row,
            )) = hit
            {
                if let Some((_, index)) = Hits::row(target, row) {
                    tasks.choose(index);
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
                Some((Target::Button(ButtonId::Cancel), _)) => self.form = None,
                Some((Target::Field(i), _)) if i < form.fields.len() => form.focus = i,
                Some((Target::Field(_), _)) => {}
                // A click outside the window closes it.
                _ => self.form = None,
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

    /// A button, clicked or chosen with its key.
    fn press(&mut self, id: ButtonId) {
        match id {
            ButtonId::Input => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.focus_input();
                }
            }
            ButtonId::To => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.menu = !tasks.menu;
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
                    // The Tasks tab shows the agents too.
                    if let Some(tasks) = &mut self.tasks {
                        tasks.reload();
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
            Purpose::Remove(path) => {
                let path = path.clone();
                let result = self.projects.update(|list| list.remove(&path));
                if self.project.as_deref() == Some(path.as_path()) {
                    self.project = None;
                    self.tasks = None;
                    self.roles = None;
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
