//! The Roles tab: which agent and model each role runs on, and which skills,
//! MCP servers and plugins it gets; plus the agent of `[retro]`.
//!
//! Changes are kept here until «Save». Saving goes through
//! `harness_core::settings::save`: the same checks as before a run, then
//! `harness.toml` is changed with `toml_edit` (comments stay) and committed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use harness_agents::credentials;
use harness_core::config::{Config, RetroConfig, RoleConfig, AGENTS, CONFIG_FILE};
use harness_core::config_edit;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::Role;
use harness_core::plugins::family;
use harness_core::settings;
use harness_core::skills::{split_header, SKILLS_DIR};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::tasks::draw_list;
use crate::ui::{buttons, panel, selected, ButtonId, Hits, ListId, Target};

/// The roles in the list, `None` is `[retro]`.
pub const WHO: [Option<Role>; 5] = [
    Some(Role::Architect),
    Some(Role::Developer),
    Some(Role::Tester),
    Some(Role::Security),
    None,
];

/// A line of the details that can be clicked or chosen with Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Agent(&'static str),
    Model,
    Skill(String),
    Mcp(String),
    Plugin(String),
}

/// What the App has to do after a row was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Ask for the model in a form; the current one is given.
    EditModel(String),
    /// Show this message.
    Say(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Details,
}

/// A skill in `.harness/skills/`: its name and the description from its header.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillFile {
    name: String,
    description: Option<String>,
}

#[derive(Debug)]
pub struct RolesTab {
    root: PathBuf,
    /// `harness.toml` as it is on disk, and what it says.
    text: String,
    saved: Config,
    /// The settings with the changes not saved yet.
    roles: BTreeMap<Role, RoleConfig>,
    retro: Option<RetroConfig>,
    skills: Vec<SkillFile>,
    /// Which agents have a saved login.
    logins: BTreeMap<&'static str, bool>,
    pub selected: usize,
    /// The chosen row of the details.
    pub row: usize,
    focus: Focus,
    pub problem: Option<String>,
}

impl RolesTab {
    pub fn load(root: &Path) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            text: String::new(),
            saved: Config::parse("").unwrap_or_else(|_| unreachable!("an empty config is valid")),
            roles: BTreeMap::new(),
            retro: None,
            skills: Vec::new(),
            logins: BTreeMap::new(),
            selected: 0,
            row: 0,
            focus: Focus::List,
            problem: None,
        };
        tab.reload();
        tab
    }

    /// Reads the settings again; changes not saved are dropped.
    pub fn reload(&mut self) {
        self.problem = None;
        let harness_dir = self.root.join(HARNESS_DIR);
        let path = harness_dir.join(CONFIG_FILE);
        match fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|text| {
                let config = Config::parse(&text).map_err(|e| e.to_string())?;
                Ok((text, config))
            }) {
            Ok((text, config)) => {
                self.roles = config.roles.clone();
                self.retro = config.retro.clone();
                self.text = text;
                self.saved = config;
            }
            Err(problem) => self.problem = Some(problem),
        }
        self.skills = read_skills(&harness_dir.join(SKILLS_DIR));
        let dir = credentials::default_dir();
        self.logins = AGENTS
            .iter()
            .map(|&agent| {
                let saved = dir
                    .as_deref()
                    .is_some_and(|dir| credentials::has_login(dir, agent));
                (agent, saved)
            })
            .collect();
        self.row = self.row.min(self.rows().len().saturating_sub(1));
    }

    /// Are the details focused (so Esc goes back to the list)?
    pub fn in_details(&self) -> bool {
        self.focus == Focus::Details
    }

    /// Are there changes not saved yet?
    pub fn changed(&self) -> bool {
        self.roles != self.saved.roles || self.retro != self.saved.retro
    }

    fn who(&self) -> Option<Role> {
        WHO[self.selected.min(WHO.len() - 1)]
    }

    /// The agent and model of the selected role or of `[retro]`.
    fn agent(&self) -> (Option<&str>, Option<&str>) {
        match self.who() {
            Some(role) => self.roles.get(&role).map_or((None, None), |r| {
                (Some(r.agent.as_str()), r.model.as_deref())
            }),
            None => self.retro.as_ref().map_or((None, None), |r| {
                (Some(r.agent.as_str()), r.model.as_deref())
            }),
        }
    }

    /// The rows of the details, in order.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = AGENTS.iter().map(|a| Row::Agent(a)).collect();
        rows.push(Row::Model);
        let Some(role) = self.who() else {
            return rows;
        };
        let settings = self.roles.get(&role);
        let listed = |list: fn(&RoleConfig) -> &Vec<String>| -> Vec<String> {
            settings.map(|s| list(s).clone()).unwrap_or_default()
        };
        let mut skills: Vec<String> = self.skills.iter().map(|s| s.name.clone()).collect();
        for name in listed(|s| &s.skills)
            .into_iter()
            .chain(listed(|s| &s.always_skills))
        {
            if !skills.contains(&name) {
                skills.push(name);
            }
        }
        rows.extend(skills.into_iter().map(Row::Skill));
        let mut servers: Vec<String> = self.saved.mcp.keys().cloned().collect();
        for name in listed(|s| &s.mcp) {
            if !servers.contains(&name) {
                servers.push(name);
            }
        }
        rows.extend(servers.into_iter().map(Row::Mcp));
        let agent = settings.map_or("", |s| family(&s.agent));
        let mut plugins: Vec<String> = self
            .saved
            .plugins
            .iter()
            .filter(|(_, p)| p.agent == agent)
            .map(|(name, _)| name.clone())
            .collect();
        for name in listed(|s| &s.plugins) {
            if !plugins.contains(&name) {
                plugins.push(name);
            }
        }
        rows.extend(plugins.into_iter().map(Row::Plugin));
        rows
    }

    pub fn on_key(&mut self, key: KeyCode, tr: &I18n) -> Action {
        let rows = self.rows().len();
        match (key, self.focus) {
            (KeyCode::Tab | KeyCode::Right, Focus::List) => self.focus = Focus::Details,
            (KeyCode::Tab | KeyCode::Left | KeyCode::Esc, Focus::Details) => {
                self.focus = Focus::List;
            }
            (KeyCode::Up | KeyCode::Char('k'), Focus::List) => {
                self.select(self.selected.saturating_sub(1))
            }
            (KeyCode::Down | KeyCode::Char('j'), Focus::List) => self.select(self.selected + 1),
            (KeyCode::Up | KeyCode::Char('k'), Focus::Details) => {
                self.row = self.row.saturating_sub(1)
            }
            (KeyCode::Down | KeyCode::Char('j'), Focus::Details) => {
                self.row = (self.row + 1).min(rows.saturating_sub(1));
            }
            (KeyCode::Enter | KeyCode::Char(' '), Focus::List) => self.focus = Focus::Details,
            (KeyCode::Enter | KeyCode::Char(' '), Focus::Details) => {
                return self.activate(self.row, tr)
            }
            _ => {}
        }
        Action::None
    }

    pub fn select(&mut self, index: usize) {
        let index = index.min(WHO.len() - 1);
        if index != self.selected {
            self.selected = index;
            self.row = 0;
        }
        self.focus = Focus::List;
    }

    /// The wheel over the details moves the chosen row.
    pub fn on_wheel(&mut self, down: bool) {
        let last = self.rows().len().saturating_sub(1);
        self.focus = Focus::Details;
        self.row = if down {
            (self.row + 1).min(last)
        } else {
            self.row.saturating_sub(1)
        };
    }

    /// A click or Enter on row `index` of the details.
    pub fn activate(&mut self, index: usize, tr: &I18n) -> Action {
        let rows = self.rows();
        let Some(row) = rows.get(index).cloned() else {
            return Action::None;
        };
        self.row = index;
        self.focus = Focus::Details;
        let who = self.who();
        match row {
            Row::Agent(agent) => self.set_agent(agent, tr),
            Row::Model => Action::EditModel(self.agent().1.unwrap_or_default().to_string()),
            Row::Skill(name) => {
                if let Some(settings) = who.and_then(|role| self.roles.get_mut(&role)) {
                    // Not used -> read when needed -> always in the prompt -> not used.
                    let on_demand = settings.skills.contains(&name);
                    let always = settings.always_skills.contains(&name);
                    settings.skills.retain(|s| *s != name);
                    settings.always_skills.retain(|s| *s != name);
                    match (on_demand, always) {
                        (false, false) => settings.skills.push(name),
                        (true, false) => settings.always_skills.push(name),
                        _ => {}
                    }
                }
                Action::None
            }
            Row::Mcp(name) => {
                if let Some(settings) = who.and_then(|role| self.roles.get_mut(&role)) {
                    toggle(&mut settings.mcp, name);
                }
                Action::None
            }
            Row::Plugin(name) => {
                if let Some(settings) = who.and_then(|role| self.roles.get_mut(&role)) {
                    toggle(&mut settings.plugins, name);
                }
                Action::None
            }
        }
    }

    fn set_agent(&mut self, agent: &str, tr: &I18n) -> Action {
        match self.who() {
            Some(role) => {
                let settings = self.roles.entry(role).or_insert_with(|| RoleConfig {
                    agent: agent.to_string(),
                    model: None,
                    skills: Vec::new(),
                    always_skills: Vec::new(),
                    mcp: Vec::new(),
                    plugins: Vec::new(),
                });
                if settings.agent == agent {
                    return Action::None;
                }
                // A model name belongs to one agent.
                settings.model = None;
                settings.agent = agent.to_string();
                // Plugins are made for one kind of agent.
                let plugins = &self.saved.plugins;
                let (keep, drop): (Vec<String>, Vec<String>) = settings
                    .plugins
                    .drain(..)
                    .partition(|name| plugins.get(name).is_some_and(|p| p.agent == family(agent)));
                settings.plugins = keep;
                if !drop.is_empty() {
                    return Action::Say(tr.f(
                        "roles.plugins_dropped",
                        &[("plugins", &drop.join(", ")), ("agent", &agent)],
                    ));
                }
            }
            None => {
                let model = match &self.retro {
                    Some(retro) if retro.agent == agent => return Action::None,
                    _ => None,
                };
                self.retro = Some(RetroConfig {
                    agent: agent.to_string(),
                    model,
                });
            }
        }
        Action::None
    }

    /// The model from the form; empty means the agent's own default.
    pub fn set_model(&mut self, model: &str) {
        let model = Some(model.trim().to_string()).filter(|m| !m.is_empty());
        match self.who() {
            Some(role) => {
                if let Some(settings) = self.roles.get_mut(&role) {
                    settings.model = model;
                }
            }
            None => {
                if let Some(retro) = &mut self.retro {
                    retro.model = model;
                }
            }
        }
    }

    pub fn undo(&mut self) {
        self.roles = self.saved.roles.clone();
        self.retro = self.saved.retro.clone();
    }

    /// Checks, writes and commits the changes. The error is shown as it is.
    pub fn save(&mut self) -> Result<(), String> {
        let mut text = self.text.clone();
        for (role, settings) in &self.roles {
            if self.saved.roles.get(role) != Some(settings) {
                text = config_edit::set_role(&text, *role, settings).map_err(|e| e.to_string())?;
            }
        }
        if self.retro != self.saved.retro {
            if let Some(retro) = &self.retro {
                text = config_edit::set_retro(&text, retro).map_err(|e| e.to_string())?;
            }
        }
        let repo = Repo::open(&self.root).map_err(|e| e.to_string())?;
        settings::save(&repo, &text).map_err(|e| e.to_string())?;
        let (selected, row) = (self.selected, self.row);
        self.reload();
        (self.selected, self.row) = (selected, row);
        Ok(())
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [bar, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let changed = self.changed();
        buttons(
            frame,
            bar,
            hits,
            &[
                (tr.t("roles.save"), ButtonId::Save, changed),
                (tr.t("roles.undo"), ButtonId::Undo, changed),
            ],
        );
        if changed {
            let note = tr.t("roles.unsaved");
            let width = u16::try_from(note.chars().count()).unwrap_or(0);
            let x = bar.right().saturating_sub(width);
            frame.render_widget(
                Span::styled(note.to_string(), Style::new().fg(Color::Yellow)),
                Rect::new(x.max(bar.x), bar.y, width.min(bar.width), 1),
            );
        }
        let [left, right] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(0)]).areas(main);

        let items: Vec<ListItem> = WHO
            .iter()
            .map(|who| {
                let (name, agent) = match who {
                    Some(role) => (
                        role_key(*role),
                        self.roles.get(role).map(|r| r.agent.as_str()),
                    ),
                    None => ("retro", self.retro.as_ref().map(|r| r.agent.as_str())),
                };
                let dirty = match who {
                    Some(role) => self.roles.get(role) != self.saved.roles.get(role),
                    None => self.retro != self.saved.retro,
                };
                let mark = if dirty { "*" } else { " " };
                ListItem::new(format!("{name:<10}{mark}{}", agent.unwrap_or("—")))
            })
            .collect();
        draw_list(
            frame,
            hits,
            left,
            ListId::Roles,
            tr.t("roles.title"),
            items,
            self.selected,
            self.focus == Focus::List,
        );

        let name = self.who().map_or("retro", role_key);
        let block = panel(&format!(" {name} "), self.focus == Focus::Details);
        let inner = block.inner(right);
        frame.render_widget(block, right);

        let lines = self.detail_lines(tr);
        // Keep the chosen row on the screen.
        let chosen_line = lines
            .iter()
            .position(|(row, _)| *row == Some(self.row))
            .unwrap_or(0);
        let height = usize::from(inner.height);
        let first = (chosen_line + 1).saturating_sub(height);
        for (y, (row, line)) in lines.into_iter().skip(first).take(height).enumerate() {
            let rect = Rect::new(
                inner.x,
                inner.y + u16::try_from(y).unwrap_or(0),
                inner.width,
                1,
            );
            let line = match row {
                Some(index) if index == self.row && self.focus == Focus::Details => {
                    line.style(selected())
                }
                _ => line,
            };
            frame.render_widget(Paragraph::new(line), rect);
            if let Some(index) = row {
                hits.add(rect, Target::Row(index));
            }
        }
    }

    /// The lines of the details; clickable ones carry their row index.
    fn detail_lines(&self, tr: &I18n) -> Vec<(Option<usize>, Line<'static>)> {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let dim = Style::new().fg(Color::DarkGray);
        let mut lines = Vec::new();
        if self.problem.is_some() {
            return lines;
        }
        let (agent, model) = self.agent();
        let who = self.who();
        let settings = who.and_then(|role| self.roles.get(&role));
        let heading = |key: &str| (None, Line::styled(tr.t(key).to_string(), bold));

        lines.push(heading(if who.is_some() {
            "roles.agent"
        } else {
            "roles.retro_agent"
        }));
        // Each group gets its heading before its first row.
        let (mut skills_seen, mut mcp_seen, mut plugins_seen) = (false, false, false);
        for (index, row) in self.rows().into_iter().enumerate() {
            let line = match row {
                Row::Agent(name) => {
                    let mark = if agent == Some(name) { "(•)" } else { "( )" };
                    let login = if self.logins.get(name).copied().unwrap_or(false) {
                        Span::styled(
                            tr.t("roles.login_saved").to_string(),
                            Style::new().fg(Color::Green),
                        )
                    } else {
                        Span::styled(
                            tr.f(
                                "roles.no_login",
                                &[("command", &credentials::login_command(name))],
                            ),
                            Style::new().fg(Color::Yellow),
                        )
                    };
                    Line::from(vec![Span::raw(format!("  {mark} {name:<16}")), login])
                }
                Row::Model => {
                    lines.push((None, Line::default()));
                    let shown = model
                        .map_or_else(|| tr.t("roles.default_model").to_string(), str::to_string);
                    Line::from(vec![
                        Span::styled(format!("{:<10}", tr.t("roles.model")), bold),
                        Span::raw(format!("[ {shown} ]  ")),
                        Span::styled(tr.t("roles.model_hint").to_string(), dim),
                    ])
                }
                Row::Skill(name) => {
                    if !skills_seen {
                        skills_seen = true;
                        lines.push((None, Line::default()));
                        lines.push(heading("roles.skills"));
                        lines.push((
                            None,
                            Line::styled(tr.t("roles.skills_hint").to_string(), dim),
                        ));
                    }
                    let settings = settings.cloned().unwrap_or_else(empty_role);
                    let mark = if settings.always_skills.contains(&name) {
                        "[■]"
                    } else if settings.skills.contains(&name) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    let about = match self.skills.iter().find(|s| s.name == name) {
                        None => Span::styled(
                            tr.t("roles.no_file").to_string(),
                            Style::new().fg(Color::Red),
                        ),
                        Some(SkillFile {
                            description: None, ..
                        }) => Span::styled(
                            tr.t("roles.no_description").to_string(),
                            Style::new().fg(Color::Red),
                        ),
                        Some(SkillFile {
                            description: Some(d),
                            ..
                        }) => Span::styled(d.clone(), dim),
                    };
                    Line::from(vec![Span::raw(format!("  {mark} {name:<24} ")), about])
                }
                Row::Mcp(name) => {
                    if !mcp_seen {
                        mcp_seen = true;
                        lines.push((None, Line::default()));
                        lines.push(heading("roles.mcp"));
                    }
                    let on = settings.is_some_and(|s| s.mcp.contains(&name));
                    let about = match self.saved.mcp.get(&name) {
                        Some(server) => Span::styled(server.command.clone(), dim),
                        None => Span::styled(
                            tr.t("roles.not_described").to_string(),
                            Style::new().fg(Color::Red),
                        ),
                    };
                    Line::from(vec![
                        Span::raw(format!("  {} {name:<24} ", check(on))),
                        about,
                    ])
                }
                Row::Plugin(name) => {
                    if !plugins_seen {
                        plugins_seen = true;
                        let title = tr.f("roles.plugins", &[("agent", &agent.map_or("", family))]);
                        lines.push((None, Line::default()));
                        lines.push((None, Line::styled(title, bold)));
                    }
                    let on = settings.is_some_and(|s| s.plugins.contains(&name));
                    let about = match self.saved.plugins.get(&name) {
                        Some(plugin) => {
                            Span::styled(plugin.source.clone().unwrap_or_default(), dim)
                        }
                        None => Span::styled(
                            tr.t("roles.not_described").to_string(),
                            Style::new().fg(Color::Red),
                        ),
                    };
                    Line::from(vec![
                        Span::raw(format!("  {} {name:<24} ", check(on))),
                        about,
                    ])
                }
            };
            lines.push((Some(index), line));
        }
        if who.is_some() {
            for (present, key) in [
                (skills_seen, "roles.no_skills"),
                (mcp_seen, "roles.no_mcp"),
                (plugins_seen, "roles.no_plugins"),
            ] {
                if !present {
                    lines.push((None, Line::default()));
                    lines.push((None, Line::styled(tr.t(key).to_string(), dim)));
                }
            }
        } else {
            lines.push((None, Line::default()));
            lines.push((
                None,
                Line::styled(tr.t("roles.retro_hint").to_string(), dim),
            ));
        }
        lines
    }
}

fn toggle(list: &mut Vec<String>, name: String) {
    if list.contains(&name) {
        list.retain(|n| *n != name);
    } else {
        list.push(name);
    }
}

fn check(on: bool) -> &'static str {
    if on {
        "[x]"
    } else {
        "[ ]"
    }
}

fn empty_role() -> RoleConfig {
    RoleConfig {
        agent: String::new(),
        model: None,
        skills: Vec::new(),
        always_skills: Vec::new(),
        mcp: Vec::new(),
        plugins: Vec::new(),
    }
}

pub fn role_key(role: Role) -> &'static str {
    match role {
        Role::Architect => "architect",
        Role::Developer => "developer",
        Role::Tester => "tester",
        Role::Security => "security",
        Role::Human => "human",
    }
}

/// The skill files of the project, sorted by name.
fn read_skills(dir: &Path) -> Vec<SkillFile> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut skills: Vec<SkillFile> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let description = fs::read_to_string(&path)
                .ok()
                .and_then(|text| split_header(&text))
                .map(|(description, _)| description);
            Some(SkillFile { name, description })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}
