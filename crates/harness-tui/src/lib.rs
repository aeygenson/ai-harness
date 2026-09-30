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
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

mod i18n;
mod projects_tab;
mod tasks;
mod ui;

use i18n::I18n;
use projects_tab::{has_config, ProjectsTab};
use tasks::TasksTab;
use ui::{buttons, expand_home, panel, ButtonId, Form, Hits, ListId, Target};

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
    NewProject,
    OpenFolder,
    /// The folder has no harness settings yet: create them?
    InitFolder(PathBuf),
    /// Take the project off the list (the folder stays).
    Remove(PathBuf),
}

/// Opens the TUI and runs until `q`. It starts with `start` if that folder is
/// a harness project, otherwise with the project opened last.
pub fn run(start: &Path) -> Result<()> {
    let mut app = App::new(projects::harness_home(), start);
    let mut terminal = ratatui::init();
    // Give the mouse back to the terminal even if the program panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    execute!(io::stdout(), EnableMouseCapture)?;
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(io::stdout(), DisableMouseCapture);
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
                _ => {}
            }
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

#[derive(Debug)]
struct App {
    tab: Tab,
    /// `~/.harness`: the project list, the language, more translations.
    home: Option<PathBuf>,
    tr: I18n,
    /// The open project.
    project: Option<PathBuf>,
    tasks: Option<TasksTab>,
    projects: ProjectsTab,
    form: Option<(Purpose, Form)>,
    /// The last result or problem, shown at the bottom.
    message: Option<(String, bool)>,
    hits: Hits,
    last_click: Option<(Instant, Target, u16)>,
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
            projects: ProjectsTab::load(home),
            form: None,
            message: None,
            hits: Hits::default(),
            last_click: None,
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
        if !has_config(root) {
            let text = self
                .tr
                .f("projects.not_a_project", &[("path", &root.display())]);
            self.message = Some((text, true));
            return;
        }
        self.tasks = Some(TasksTab::load(root));
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

    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('L') => self.press(ButtonId::Language),
            KeyCode::Char(c @ '1'..='7') => {
                let index = usize::from(c as u8 - b'1');
                self.tab = TABS[index].0;
            }
            KeyCode::Char('r') | KeyCode::F(5) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.reload();
                }
                self.projects.reload();
            }
            code => match self.tab {
                Tab::Tasks => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_key(code);
                    }
                }
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
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, hit: Option<(Target, u16)>, double: bool) {
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
                Some((list, index)) => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_click(list, index);
                    }
                }
                None => {}
            },
            Target::Field(_) => {}
        }
    }

    /// A button, clicked or chosen with its key.
    fn press(&mut self, id: ButtonId) {
        match id {
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
            ButtonId::NewProject => {
                let tr = &self.tr;
                self.form = Some((
                    Purpose::NewProject,
                    Form::new(
                        tr.t("form.new_title"),
                        tr.t("form.new_text"),
                        tr.t("form.create"),
                    )
                    .field(tr.t("form.new_folder"), "~/code/")
                    .field(tr.t("form.new_name"), ""),
                ));
            }
            ButtonId::OpenFolder => {
                let tr = &self.tr;
                self.form = Some((
                    Purpose::OpenFolder,
                    Form::new(tr.t("form.open_title"), "", tr.t("form.open"))
                        .field(tr.t("form.folder"), "~/code/"),
                ));
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
            ButtonId::Ok | ButtonId::Cancel => {}
        }
    }

    /// OK in the open form.
    fn submit(&mut self) {
        let Some((purpose, form)) = self.form.take() else {
            return;
        };
        let result = match &purpose {
            Purpose::NewProject => self.create_project(&form),
            Purpose::OpenFolder => {
                let path = expand_home(form.value(0));
                if form.value(0).is_empty() {
                    Err(self.tr.t("form.type_folder").to_string())
                } else if !path.is_dir() {
                    Err(self
                        .tr
                        .f("form.no_such_folder", &[("path", &path.display())]))
                } else if has_config(&path) {
                    self.open(&path);
                    Ok(())
                } else {
                    self.form = Some(self.init_form(&path));
                    return;
                }
            }
            Purpose::InitFolder(path) => projects::init(path)
                .map(|_| self.open(path))
                .map_err(|e| e.to_string()),
            Purpose::Remove(path) => {
                let path = path.clone();
                let result = self.projects.update(|list| list.remove(&path));
                if self.project.as_deref() == Some(path.as_path()) {
                    self.project = None;
                    self.tasks = None;
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

    fn create_project(&mut self, form: &Form) -> Result<(), String> {
        if form.value(0).is_empty() || form.value(0) == "~/code/" {
            return Err(self.tr.t("form.no_folder").to_string());
        }
        let path = expand_home(form.value(0));
        if has_config(&path) {
            return Err(self.tr.t("form.exists").to_string());
        }
        let done = projects::init(&path).map_err(|e| e.to_string())?;
        let path = path.canonicalize().unwrap_or(path);
        let name = match form.value(1) {
            "" => name_of(&path),
            name => name.to_string(),
        };
        self.projects.update(|list| list.add(&name, &path))?;
        self.open(&path);
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
        spans.push(Span::styled(
            format!(" {}", self.tr.t("footer.hint")),
            Style::new().fg(Color::DarkGray),
        ));
        frame.render_widget(Line::from(spans), footer);

        if let Some((_, form)) = &self.form {
            form.draw(frame, &mut self.hits, self.tr.t("form.cancel"));
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
