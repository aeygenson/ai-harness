//! The Roles tab: which agent, model and effort level each role runs on, and
//! which skills, MCP servers and plugins it gets; plus the agent of `[retro]`.
//!
//! The models and levels come from what the agents themselves said last
//! (`harness_core::models`, «Refresh models»); a model can also be typed in.
//!
//! Changes are kept here until «Save». Saving goes through
//! `harness_core::config::save::save`: the same checks as before a run, then
//! `harness.toml` is changed with `toml_edit` (comments stay) and committed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use harness_core::config;
use harness_core::config::{
    AgentKind, Config, McpConfig, PluginConfig, RetroConfig, RoleConfig, CONFIG_FILE,
};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::models::{self, ModelList};
use harness_core::skills;
use harness_core::task::handoff::Role;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
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
    Agent(AgentKind),
    /// A model from the agent's list; `None` is the agent's own default.
    Model(Option<String>),
    /// A model typed in by hand.
    OtherModel,
    /// The effort level: a click moves to the next one.
    Effort,
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

/// A skill a role can choose: its name and the description from its header.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillFile {
    name: String,
    description: Option<String>,
}

#[derive(Debug)]
pub struct RolesTab {
    root: PathBuf,
    /// `~/.harness`, where the agents' model lists are kept.
    home: Option<PathBuf>,
    /// The model list of each agent, as it answered last.
    models: BTreeMap<AgentKind, ModelList>,
    /// `harness.toml` as it is on disk, and what it says.
    text: String,
    saved: Config,
    /// The settings with the changes not saved yet.
    roles: BTreeMap<Role, RoleConfig>,
    retro: Option<RetroConfig>,
    skills: Vec<SkillFile>,
    /// The agents installed with a saved login, from the Agents tab's check;
    /// `None` until it has answered, then all agents are offered.
    ready: Option<BTreeSet<AgentKind>>,
    pub selected: usize,
    /// The chosen row of the details.
    pub row: usize,
    focus: Focus,
    pub problem: Option<String>,
    /// The agents are being asked for their models.
    pub refreshing: bool,
}

impl RolesTab {
    pub fn load(root: &Path, home: Option<&Path>) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            home: home.map(Path::to_path_buf),
            models: BTreeMap::new(),
            text: String::new(),
            saved: Config::parse("").unwrap_or_else(|_| unreachable!("an empty config is valid")),
            roles: BTreeMap::new(),
            retro: None,
            skills: Vec::new(),
            ready: None,
            selected: 0,
            row: 0,
            focus: Focus::List,
            problem: None,
            refreshing: false,
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
        self.skills = read_skills(&harness_dir);
        self.reload_models();
    }

    /// What the Agents tab found: only these agents are offered for a role.
    pub fn set_ready(&mut self, ready: Option<BTreeSet<AgentKind>>) {
        self.ready = ready;
        self.row = self.row.min(self.rows().len().saturating_sub(1));
    }

    /// The agent can work: installed with a login, or not checked yet.
    fn is_ready(&self, agent: AgentKind) -> bool {
        self.ready
            .as_ref()
            .is_none_or(|ready| ready.contains(&agent))
    }

    /// Reads the saved model lists again (after «Refresh models»).
    pub fn reload_models(&mut self) {
        self.models = match &self.home {
            Some(home) => AgentKind::ALL
                .into_iter()
                .filter_map(|agent| models::load(home, agent).map(|l| (agent, l)))
                .collect(),
            None => BTreeMap::new(),
        };
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
    fn agent(&self) -> (Option<AgentKind>, Option<&str>) {
        let (agent, model, _) = self.choice();
        (agent, model)
    }

    /// The agent, model and effort of the selected role or of `[retro]`.
    fn choice(&self) -> (Option<AgentKind>, Option<&str>, Option<&str>) {
        match self.who() {
            Some(role) => self.roles.get(&role).map_or((None, None, None), |r| {
                (Some(r.agent), r.model.as_deref(), r.effort.as_deref())
            }),
            None => self.retro.as_ref().map_or((None, None, None), |r| {
                (Some(r.agent), r.model.as_deref(), r.effort.as_deref())
            }),
        }
    }

    /// The model list of the selected role's agent.
    fn model_list(&self) -> Option<&ModelList> {
        self.agent().0.and_then(|agent| self.models.get(&agent))
    }

    /// The levels the chosen model takes (for the agent's default model,
    /// those of the model the agent calls its default).
    fn effort_levels(&self) -> Vec<String> {
        let Some(list) = self.model_list() else {
            return Vec::new();
        };
        let model = match self.agent().1 {
            Some(id) => list.find(id),
            None => list.models.iter().find(|m| m.default),
        };
        model.map(|m| m.efforts.clone()).unwrap_or_default()
    }

    /// Sets the model and effort of the selected role or of `[retro]`.
    fn set_choice(&mut self, model: Option<String>, effort: Option<String>) {
        match self.who() {
            Some(role) => {
                if let Some(settings) = self.roles.get_mut(&role) {
                    (settings.model, settings.effort) = (model, effort);
                }
            }
            None => {
                if let Some(retro) = &mut self.retro {
                    (retro.model, retro.effort) = (model, effort);
                }
            }
        }
    }

    /// What a role gets when it moves to `agent`: the model and level of
    /// another role on that agent, otherwise the agent's own default.
    fn first_choice(&self, agent: AgentKind) -> (Option<String>, Option<String>) {
        let who = self.who();
        let other = self
            .roles
            .iter()
            .filter(|(role, _)| Some(**role) != who)
            .map(|(_, r)| (r.agent, &r.model, &r.effort))
            .chain(
                self.retro
                    .iter()
                    .filter(|_| who.is_some())
                    .map(|r| (r.agent, &r.model, &r.effort)),
            )
            .find(|(a, model, _)| *a == agent && model.is_some());
        match other {
            Some((_, model, effort)) => (model.clone(), effort.clone()),
            None => self
                .models
                .get(&agent)
                .map_or((None, None), ModelList::default_choice),
        }
    }

    /// The rows of the details, in order.
    pub fn rows(&self) -> Vec<Row> {
        // Only agents ready to work, and the one chosen now even if it is not.
        let chosen = self.choice().0;
        let mut rows: Vec<Row> = AgentKind::ALL
            .into_iter()
            .filter(|&a| self.is_ready(a) || chosen == Some(a))
            .map(Row::Agent)
            .collect();
        rows.push(Row::Model(None));
        if let Some(list) = self.model_list() {
            rows.extend(list.models.iter().map(|m| Row::Model(Some(m.id.clone()))));
        }
        rows.push(Row::OtherModel);
        if !self.effort_levels().is_empty() || self.choice().2.is_some() {
            rows.push(Row::Effort);
        }
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
        let agent = settings.map(|s| s.agent);
        let mut plugins: Vec<String> = self
            .saved
            .plugins
            .iter()
            .filter(|(_, p)| Some(p.agent) == agent)
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
            Row::Model(model) => {
                let effort = match (&model, self.model_list()) {
                    (None, _) => None,
                    (Some(id), Some(list)) => {
                        list.find(id).and_then(|m| m.effort_for(self.choice().2))
                    }
                    (Some(_), None) => self.choice().2.map(str::to_string),
                };
                self.set_choice(model, effort);
                Action::None
            }
            Row::OtherModel => {
                let typed = self
                    .agent()
                    .1
                    .filter(|m| self.model_list().is_none_or(|l| l.find(m).is_none()));
                Action::EditModel(typed.unwrap_or_default().to_string())
            }
            Row::Effort => {
                // The agent's default -> each level -> the agent's default.
                let mut levels: Vec<Option<String>> = vec![None];
                levels.extend(self.effort_levels().into_iter().map(Some));
                let (_, model, effort) = self.choice();
                let at = levels
                    .iter()
                    .position(|l| l.as_deref() == effort)
                    .map_or(0, |i| (i + 1) % levels.len());
                let model = model.map(str::to_string);
                self.set_choice(model, levels[at].clone());
                Action::None
            }
            Row::Skill(name) => {
                if let Some(role) = who {
                    self.cycle_skill(role, &name);
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

    fn set_agent(&mut self, agent: AgentKind, tr: &I18n) -> Action {
        match self.who() {
            Some(role) => {
                let first = self.first_choice(agent);
                let settings = self.roles.entry(role).or_insert_with(|| RoleConfig {
                    agent,
                    model: None,
                    effort: None,
                    skills: Vec::new(),
                    always_skills: Vec::new(),
                    mcp: Vec::new(),
                    plugins: Vec::new(),
                });
                if settings.agent == agent {
                    return Action::None;
                }
                // A model belongs to one agent: take the new agent's.
                (settings.model, settings.effort) = first;
                settings.agent = agent;
                // Plugins are made for one kind of agent.
                let plugins = &self.saved.plugins;
                let (keep, drop): (Vec<String>, Vec<String>) = settings
                    .plugins
                    .drain(..)
                    .partition(|name| plugins.get(name).is_some_and(|p| p.agent == agent));
                settings.plugins = keep;
                if !drop.is_empty() {
                    return Action::Say(tr.f(
                        "roles.plugins_dropped",
                        &[("plugins", &drop.join(", ")), ("agent", &agent)],
                    ));
                }
            }
            None => {
                if self.retro.as_ref().is_some_and(|r| r.agent == agent) {
                    return Action::None;
                }
                let (model, effort) = self.first_choice(agent);
                self.retro = Some(RetroConfig {
                    agent,
                    model,
                    effort,
                });
            }
        }
        Action::None
    }

    /// The model from the form; empty means the agent's own default. The
    /// level stays if the model is in the list and takes it.
    pub fn set_model(&mut self, model: &str) {
        let model = Some(model.trim().to_string()).filter(|m| !m.is_empty());
        let effort = self.choice().2.map(str::to_string);
        let effort = match (&model, self.model_list()) {
            (None, _) => None,
            (Some(id), Some(list)) => match list.find(id) {
                Some(found) => found.effort_for(effort.as_deref()),
                None => effort,
            },
            (Some(_), None) => effort,
        };
        self.set_choice(model, effort);
    }

    /// `harness.toml` as it is on disk.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The MCP servers `harness.toml` describes.
    pub fn servers(&self) -> &BTreeMap<String, McpConfig> {
        &self.saved.mcp
    }

    /// A role's settings with the changes not saved yet.
    pub fn settings(&self, role: Role) -> Option<&RoleConfig> {
        self.roles.get(&role)
    }

    /// The next mark of skill `name` for `role`: not used -> read when
    /// needed -> always in the prompt -> not used; saved with the rest of
    /// the changes.
    pub fn cycle_skill(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            let on_demand = settings.skills.iter().any(|s| s == name);
            let always = settings.always_skills.iter().any(|s| s == name);
            settings.skills.retain(|s| s != name);
            settings.always_skills.retain(|s| s != name);
            match (on_demand, always) {
                (false, false) => settings.skills.push(name.to_string()),
                (true, false) => settings.always_skills.push(name.to_string()),
                _ => {}
            }
        }
    }

    /// Gives `role` the MCP server `name`, or takes it away; saved with
    /// the rest of the changes.
    pub fn toggle_mcp(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            toggle(&mut settings.mcp, name.to_string());
        }
    }

    /// The plugins `harness.toml` describes.
    pub fn plugins(&self) -> &BTreeMap<String, PluginConfig> {
        &self.saved.plugins
    }

    /// Gives `role` the plugin `name`, or takes it away; saved with the
    /// rest of the changes.
    pub fn toggle_plugin(&mut self, role: Role, name: &str) {
        if let Some(settings) = self.roles.get_mut(&role) {
            toggle(&mut settings.plugins, name.to_string());
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
                text = config::edit::set_role(&text, *role, settings).map_err(|e| e.to_string())?;
            }
        }
        if self.retro != self.saved.retro {
            if let Some(retro) = &self.retro {
                text = config::edit::set_retro(&text, retro).map_err(|e| e.to_string())?;
            }
        }
        let repo = Repo::open(&self.root).map_err(|e| e.to_string())?;
        config::save::save(&repo, &text).map_err(|e| e.to_string())?;
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
                (
                    tr.t("roles.refresh"),
                    ButtonId::RefreshModels,
                    !self.refreshing,
                ),
            ],
        );
        if changed {
            let note = tr.t("roles.unsaved");
            let width = u16::try_from(note.chars().count()).unwrap_or(0);
            let x = bar.right().saturating_sub(width);
            frame.render_widget(
                Span::styled(note.to_string(), theme::warn()),
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
                        role.as_str(),
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

        let name = self.who().map_or("retro", Role::as_str);
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
        let dim = theme::dim();
        let mut lines = Vec::new();
        if self.problem.is_some() {
            return lines;
        }
        let (agent, model, effort) = self.choice();
        let list = self.model_list();
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
                    let mut spans = vec![Span::raw(format!("  {mark} {name:<16}"))];
                    if !self.is_ready(name) {
                        spans.push(Span::styled(
                            tr.t("roles.not_ready").to_string(),
                            theme::bad(),
                        ));
                    }
                    Line::from(spans)
                }
                Row::Model(None) => {
                    lines.push((None, Line::default()));
                    lines.push(heading("roles.model"));
                    let mark = if model.is_none() { "(•)" } else { "( )" };
                    Line::from(format!("  {mark} {}", tr.t("roles.default_model")))
                }
                Row::Model(Some(id)) => {
                    let mark = if model == Some(id.as_str()) {
                        "(•)"
                    } else {
                        "( )"
                    };
                    let about = list
                        .and_then(|l| l.find(&id))
                        .map(|m| {
                            let default = if m.default {
                                format!("{} ", tr.t("roles.agents_default"))
                            } else {
                                String::new()
                            };
                            format!("{default}{}", m.name.clone().unwrap_or_default())
                        })
                        .unwrap_or_default();
                    Line::from(vec![
                        Span::raw(format!("  {mark} {id:<24} ")),
                        Span::styled(about, dim),
                    ])
                }
                Row::OtherModel => {
                    let typed = model.filter(|m| list.is_none_or(|l| l.find(m).is_none()));
                    let line = match typed {
                        Some(m) => Line::from(vec![
                            Span::raw(format!(
                                "  (•) {} ",
                                tr.f("roles.model_typed", &[("model", &m)])
                            )),
                            Span::styled(tr.t("roles.model_hint").to_string(), dim),
                        ]),
                        None => Line::from(format!("  ( ) {}", tr.t("roles.model_other"))),
                    };
                    if list.is_none() {
                        lines.push((Some(index), line));
                        let hint = tr.f(
                            "roles.no_models",
                            &[("agent", &agent.map_or("", AgentKind::as_str))],
                        );
                        lines.push((None, Line::styled(hint, dim)));
                        continue;
                    }
                    line
                }
                Row::Effort => {
                    lines.push((None, Line::default()));
                    let mut spans =
                        vec![Span::styled(format!("{:<10}", tr.t("roles.effort")), bold)];
                    let mut levels: Vec<Option<String>> = vec![None];
                    levels.extend(self.effort_levels().into_iter().map(Some));
                    if effort.is_some() && !levels.iter().any(|l| l.as_deref() == effort) {
                        levels.push(effort.map(str::to_string));
                    }
                    for (i, level) in levels.iter().enumerate() {
                        let name = level
                            .clone()
                            .unwrap_or_else(|| tr.t("roles.default_effort").to_string());
                        if i > 0 {
                            spans.push(Span::styled(" · ", dim));
                        }
                        if level.as_deref() == effort {
                            spans.push(Span::styled(format!("[{name}]"), bold));
                        } else {
                            spans.push(Span::styled(name, dim));
                        }
                    }
                    spans.push(Span::styled(
                        format!("   {}", tr.t("roles.effort_hint")),
                        dim,
                    ));
                    Line::from(spans)
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
                    let mark = if settings.is_some_and(|s| s.always_skills.contains(&name)) {
                        "[■]"
                    } else if settings.is_some_and(|s| s.skills.contains(&name)) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    let about = match self.skills.iter().find(|s| s.name == name) {
                        None => Span::styled(tr.t("roles.no_file").to_string(), theme::bad()),
                        Some(SkillFile {
                            description: None, ..
                        }) => Span::styled(tr.t("roles.no_description").to_string(), theme::bad()),
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
                        None => Span::styled(tr.t("roles.not_described").to_string(), theme::bad()),
                    };
                    Line::from(vec![
                        Span::raw(format!("  {} {name:<24} ", check(on))),
                        about,
                    ])
                }
                Row::Plugin(name) => {
                    if !plugins_seen {
                        plugins_seen = true;
                        let title = tr.f(
                            "roles.plugins",
                            &[("agent", &agent.map_or("", AgentKind::as_str))],
                        );
                        lines.push((None, Line::default()));
                        lines.push((None, Line::styled(title, bold)));
                    }
                    let on = settings.is_some_and(|s| s.plugins.contains(&name));
                    let about = match self.saved.plugins.get(&name) {
                        Some(plugin) => {
                            Span::styled(plugin.source.clone().unwrap_or_default(), dim)
                        }
                        None => Span::styled(tr.t("roles.not_described").to_string(), theme::bad()),
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

/// The skills a role can choose: built-in optional ones and the project's
/// own, sorted by name. The base of each role is not chosen here.
fn read_skills(harness_dir: &Path) -> Vec<SkillFile> {
    skills::library(harness_dir)
        .into_iter()
        .filter(|s| !skills::is_base(&s.name))
        .map(|s| SkillFile {
            name: s.name,
            description: s.description,
        })
        .collect()
}
