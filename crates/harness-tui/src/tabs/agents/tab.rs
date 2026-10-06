//! The Agents tab: the catalog of console agents, with the ones installed on
//! this computer marked (see `harness_agents::install::catalog`). It does not need an
//! open project: agents are installed on the computer, not in a project.

use harness_agents::install::catalog::{self, Action, Status, CATALOG};
use harness_agents::install::credentials;
use harness_core::config::AgentKind;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId};

#[derive(Debug)]
pub struct AgentsTab {
    /// One per catalog entry, in the catalog's order.
    pub statuses: Vec<Status>,
    /// The computer was checked at least once.
    pub known: bool,
    /// A check runs in the background.
    pub checking: bool,
    pub selected: usize,
    /// The install or update that runs or ran last, with what it printed.
    pub job: Option<Job>,
}

/// An install or update command, started from the tab.
#[derive(Debug)]
pub struct Job {
    pub name: &'static str,
    pub action: Action,
    pub command: String,
    pub lines: Vec<String>,
    /// `None` while it runs.
    pub done: Option<Result<(), String>>,
}

/// The most lines of a command's output kept.
const KEEP_LINES: usize = 500;

impl Job {
    pub fn running(&self) -> bool {
        self.done.is_none()
    }

    pub fn push(&mut self, line: String) {
        self.lines.push(line);
        if self.lines.len() > KEEP_LINES {
            self.lines.drain(..self.lines.len() - KEEP_LINES);
        }
    }
}

impl AgentsTab {
    pub fn new() -> Self {
        let statuses = CATALOG
            .iter()
            .map(|entry| Status {
                entry: *entry,
                path: None,
                version: None,
                problem: None,
                login: None,
            })
            .collect();
        Self {
            statuses,
            known: false,
            checking: false,
            selected: 0,
            job: None,
        }
    }

    /// The answer of a check.
    pub fn checked(&mut self, statuses: Vec<Status>) {
        self.checking = false;
        if !statuses.is_empty() {
            self.statuses = statuses;
            self.known = true;
        }
        self.selected = self.selected.min(self.statuses.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&Status> {
        self.statuses.get(self.selected)
    }

    /// What «Install»/«Update» (or, with `remove`, «Remove») would do for
    /// the selected agent, with the command: only once the computer was
    /// checked, while nothing else runs, and when there is a command for
    /// this system.
    pub fn next_step(&self, remove: bool) -> Option<(&Status, Action, String)> {
        if !self.known || self.checking || self.job.as_ref().is_some_and(Job::running) {
            return None;
        }
        let status = self.current()?;
        if remove {
            return Some((status, Action::Remove, status.removal()?));
        }
        let (action, command) = status.action()?;
        Some((status, action, command?))
    }

    /// The agent «Sign in» would sign in to: one the harness runs and that
    /// is installed, once the computer was checked and while nothing runs.
    pub fn sign_in_target(&self) -> Option<&Status> {
        if !self.known || self.checking || self.job.as_ref().is_some_and(Job::running) {
            return None;
        }
        self.current()
            .filter(|status| status.entry.runs() && status.installed())
    }

    /// Looks again which agents have a saved login (after «Sign in»).
    pub fn reload_logins(&mut self, credentials_dir: &std::path::Path) {
        for status in &mut self.statuses {
            if let Ok(agent) = status.entry.id.parse::<AgentKind>() {
                status.login = Some(credentials::has_login(credentials_dir, agent));
            }
        }
    }

    /// The agents ready for a role: installed, with a saved login.
    pub fn ready(&self) -> Option<std::collections::BTreeSet<AgentKind>> {
        self.known.then(|| {
            self.statuses
                .iter()
                .filter(|s| s.installed() && s.login == Some(true))
                .filter_map(|s| s.entry.id.parse::<AgentKind>().ok())
                .collect()
        })
    }

    pub fn select(&mut self, index: usize) {
        if index < self.statuses.len() {
            self.selected = index;
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.statuses.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn on_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            _ => {}
        }
    }

    /// The mark before an agent: installed, too old, not installed, not known yet.
    fn mark(&self, status: &Status) -> (&'static str, ratatui::style::Style) {
        if !self.known {
            ("…", theme::dim())
        } else if status.old() {
            ("!", theme::warn())
        } else if status.installed() {
            ("✓", theme::ok())
        } else {
            ("○", theme::dim())
        }
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [bar, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let check = if self.checking {
            tr.t("agents.checking_button")
        } else {
            tr.t("agents.check")
        };
        let step = self.next_step(false);
        let removable = self.next_step(true).is_some();
        let run = match self.current().and_then(Status::action) {
            Some((Action::Update, _)) => tr.t("agents.run_update"),
            _ => tr.t("agents.run_install"),
        };
        let running = self.job.as_ref().is_some_and(Job::running);
        buttons(
            frame,
            bar,
            hits,
            &[
                (check, ButtonId::AgentsCheck, !self.checking && !running),
                (run, ButtonId::AgentRun, step.is_some()),
                (tr.t("agents.run_remove"), ButtonId::AgentRemove, removable),
                (
                    tr.t("agents.run_sign_in"),
                    ButtonId::AgentSignIn,
                    self.sign_in_target().is_some(),
                ),
            ],
        );
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
                .areas(main);

        let items: Vec<ListItem> = self
            .statuses
            .iter()
            .map(|status| {
                let (mark, style) = self.mark(status);
                let mut spans = vec![
                    Span::styled(format!("{mark} "), style),
                    Span::raw(status.entry.name.to_string()),
                ];
                if !status.entry.runs() {
                    spans.push(Span::styled(
                        format!("  {}", tr.t("agents.catalog_only")),
                        theme::bad(),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        draw_list(
            frame,
            hits,
            left,
            ListId::Agents,
            tr.t("agents.title"),
            items,
            self.selected,
            true,
        );

        let (right, log) = match &self.job {
            Some(_) => {
                let [top, bottom] =
                    Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                        .areas(right);
                (top, Some(bottom))
            }
            None => (right, None),
        };
        let text = match self.current() {
            Some(status) => self.details(status, tr),
            None => vec![],
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(panel(tr.t("agents.about"), false))
                .wrap(Wrap { trim: false }),
            right,
        );
        if let (Some(job), Some(area)) = (&self.job, log) {
            draw_job(frame, area, job, tr);
        }
    }

    fn details(&self, status: &Status, tr: &I18n) -> Vec<Line<'static>> {
        let entry = &status.entry;
        let mut lines = vec![
            Line::styled(
                format!("{} · {}", entry.name, entry.vendor),
                theme::accent(),
            ),
            Line::default(),
        ];
        // Said first and loud: roles cannot use it, whatever is installed.
        if !entry.runs() {
            let loud = theme::bad().add_modifier(Modifier::BOLD);
            lines.push(Line::styled(tr.t("agents.not_run_title").to_string(), loud));
            lines.push(Line::styled(
                tr.t("agents.not_run").to_string(),
                theme::bad(),
            ));
            lines.push(Line::styled(
                tr.t("agents.not_run_ask").to_string(),
                theme::bad(),
            ));
            lines.push(Line::default());
        }
        let state = if !self.known {
            Line::styled(tr.t("agents.checking").to_string(), theme::dim())
        } else if status.host_only() {
            let host = entry.inside.unwrap_or_default();
            Line::styled(tr.f("agents.host_only", &[("host", &host)]), theme::warn())
        } else if let Some(path) = &status.path {
            let version = status.version.as_deref().unwrap_or("?");
            let text = tr.f(
                "agents.installed",
                &[("path", &path.display()), ("version", &version)],
            );
            Line::styled(text, theme::ok())
        } else {
            Line::styled(tr.t("agents.missing").to_string(), theme::dim())
        };
        lines.push(state);
        if let (true, Some(min)) = (status.old(), entry.min_version) {
            let text = tr.f("agents.old", &[("min", &min)]);
            lines.push(Line::styled(text, theme::warn()));
        }
        if let Some(problem) = &status.problem {
            lines.push(Line::styled(problem.clone(), theme::warn()));
        }
        if let Some(host) = entry.inside {
            lines.push(Line::from(tr.f("agents.inside", &[("host", &host)])));
        }
        if entry.runs() {
            lines.push(Line::from(tr.f("agents.runs", &[("id", &entry.id)])));
        }
        match status.login {
            Some(true) if self.known => {
                lines.push(Line::styled(tr.t("agents.login").to_string(), theme::ok()));
            }
            Some(false) if self.known => {
                lines.push(Line::styled(
                    tr.t("agents.no_login").to_string(),
                    theme::warn(),
                ));
            }
            _ => {}
        }
        lines.push(Line::default());
        lines.push(Line::from(tr.f("agents.plan", &[("plan", &entry.plan)])));
        let about = format!("agents.about_{}", entry.id.replace('+', "_"));
        let about = tr.t(&about);
        if !about.starts_with("agents.") {
            lines.push(Line::default());
            lines.extend(about.lines().map(|l| Line::from(l.to_string())));
        }
        lines.push(Line::default());
        match status.action() {
            Some((action, Some(command))) => {
                let key = match action {
                    Action::Install => "agents.install",
                    Action::Update | Action::Remove => "agents.update",
                };
                lines.push(Line::from(tr.t(key).to_string()));
                lines.push(Line::styled(format!("  {command}"), theme::accent()));
            }
            Some((_, None)) => match (
                status.needs_sudo(Action::Update),
                status.needs_sudo(Action::Remove),
            ) {
                (Some(update), Some(remove)) => {
                    lines.push(Line::styled(
                        tr.t("agents.system_npm").to_string(),
                        theme::warn(),
                    ));
                    lines.push(Line::styled(format!("  {update}"), theme::accent()));
                    lines.push(Line::styled(format!("  {remove}"), theme::accent()));
                }
                _ => lines.push(Line::from(
                    tr.f("agents.see_site", &[("site", &entry.site)]),
                )),
            },
            None => {}
        }
        lines.push(Line::from(tr.f("agents.site", &[("site", &entry.site)])));
        lines
    }
}

/// The output of the install or update: the last lines that fit, and how it ended.
fn draw_job(frame: &mut Frame, area: Rect, job: &Job, tr: &I18n) {
    let key = match (job.action, &job.done) {
        (_, None) => "agents.job_running",
        (Action::Install, Some(Ok(()))) => "agents.job_installed",
        (Action::Update, Some(Ok(()))) => "agents.job_updated",
        (Action::Remove, Some(Ok(()))) => "agents.job_removed",
        (_, Some(Err(_))) => "agents.job_failed",
    };
    let title = format!(" {} ", tr.f(key, &[("name", &job.name)]));
    let inner_height = usize::from(area.height.saturating_sub(2));
    let mut lines: Vec<Line> = vec![Line::styled(format!("$ {}", job.command), theme::accent())];
    if let Some(Err(error)) = &job.done {
        lines.push(Line::styled(error.clone(), theme::bad()));
    }
    let room = inner_height.saturating_sub(lines.len());
    let start = job.lines.len().saturating_sub(room);
    lines.extend(job.lines[start..].iter().map(|l| Line::from(l.clone())));
    let focused = job.running();
    frame.render_widget(Paragraph::new(lines).block(panel(&title, focused)), area);
}

/// Checks the computer: which agents are installed, their versions, logins.
pub type AgentChecker = fn(Option<&std::path::Path>) -> Vec<Status>;

pub fn check(credentials_dir: Option<&std::path::Path>) -> Vec<Status> {
    catalog::check_all(credentials_dir)
}

/// What a running install or update sends: a line it printed, or its end.
#[derive(Debug)]
pub enum JobEvent {
    Line(String),
    Done(Result<(), String>),
}

/// Runs an install or update command; tests give a fake one.
pub type Installer = fn(&str, &std::sync::mpsc::Sender<JobEvent>);

pub fn install(command: &str, tx: &std::sync::mpsc::Sender<JobEvent>) {
    let result = catalog::run_command(command, |line| {
        let _ = tx.send(JobEvent::Line(line));
    });
    let _ = tx.send(JobEvent::Done(result));
}
