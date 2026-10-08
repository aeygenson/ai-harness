//! The Agents tab: the catalog of console agents, with the ones installed on
//! this computer marked (see `harness_agents::install::catalog`). It does not need an
//! open project: agents are installed on the computer, not in a project.

use ratatui::crossterm::event::KeyCode;

use harness_agents::install::catalog::{self, Action, Status, CATALOG};
use harness_agents::install::credentials;
use harness_agents::install::tools::ToolStatus;
use harness_core::config::AgentKind;

use super::release::HarnessRelease;

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
    /// The other programs the harness needs; empty until they were checked.
    pub tools: Vec<ToolStatus>,
    /// The harness's own version and whether a newer one is out.
    pub release: HarnessRelease,
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
    /// The job updates the harness itself, not an agent.
    pub harness: bool,
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
            tools: Vec::new(),
            release: HarnessRelease::of_this_program(),
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

    /// The version «Update Harness» would install: only while nothing else runs.
    pub fn harness_update(&self) -> Option<&str> {
        if self.job.as_ref().is_some_and(Job::running) {
            return None;
        }
        self.release.available()
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
                .filter(|s| s.ready())
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
}

/// Checks the computer: which agents are installed, their versions, logins.
pub type AgentChecker = fn(Option<&std::path::Path>) -> Vec<Status>;

pub fn check(credentials_dir: Option<&std::path::Path>) -> Vec<Status> {
    catalog::check_all(credentials_dir)
}

/// Checks the other programs the harness needs (Git, Node.js, ...).
pub type ToolChecker = fn() -> Vec<ToolStatus>;

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
