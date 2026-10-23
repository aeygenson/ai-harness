//! Keys, clicks and the mouse wheel on the Tasks tab.

use ratatui::crossterm::event::KeyCode;

use super::tab::{Focus, TasksTab, Zoom};
use crate::ui::ListId;

impl TasksTab {
    /// Keys go into the message.
    pub fn typing(&self) -> bool {
        self.focus == Focus::Input
    }

    pub fn focus_input(&mut self) {
        self.focus = Focus::Input;
    }

    /// Pasted text goes into the message.
    pub fn paste(&mut self, text: &str) {
        self.input
            .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        self.focus = Focus::Input;
    }

    pub fn on_key(&mut self, key: KeyCode) {
        if self.focus == Focus::Input {
            match key {
                KeyCode::Esc | KeyCode::Tab => {
                    self.focus = Focus::Tasks;
                    self.menu = None;
                }
                KeyCode::Up => self.cycle_choice(-1),
                KeyCode::Down => self.cycle_choice(1),
                KeyCode::Enter => self.input.push('\n'),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            }
            return;
        }
        if self.focus == Focus::Log && self.log_key(key) {
            return;
        }
        match key {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tasks => Focus::Steps,
                    Focus::Steps if self.has_log() => Focus::Log,
                    Focus::Steps | Focus::Log | Focus::Input => Focus::Input,
                }
            }
            KeyCode::Enter => self.focus = Focus::Input,
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Tasks,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Steps,
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            // Nothing else on the tab uses them: the live log's ends.
            KeyCode::Home => self.log.go_to_oldest(),
            KeyCode::End => self.log.go_to_newest(),
            _ => {}
        }
    }

    /// A click on a row of the task or step list.
    pub fn on_click(&mut self, list: ListId, index: usize) {
        match list {
            ListId::Tasks if index < self.tasks.len() => {
                self.focus = Focus::Tasks;
                if index != self.task {
                    self.task = index;
                    self.step = self.last_step();
                    self.sync_choice();
                }
            }
            ListId::Steps if index < self.current().map_or(0, |t| t.steps.len()) => {
                self.focus = Focus::Steps;
                self.step = index;
            }
            _ => return,
        }
        self.scroll = 0;
    }

    /// The wheel over a list moves its selection: that list gets the focus.
    pub fn on_click_list(&mut self, list: ListId) {
        match list {
            ListId::Tasks => self.focus = Focus::Tasks,
            ListId::Steps => self.focus = Focus::Steps,
            ListId::Projects
            | ListId::Roles
            | ListId::Folders
            | ListId::Choices
            | ListId::RoleFilter
            | ListId::Skills
            | ListId::Mcp
            | ListId::McpCatalog
            | ListId::Plugins
            | ListId::PluginCatalog
            | ListId::PluginCatalogs
            | ListId::Retros
            | ListId::RetroProposals
            | ListId::Agents
            | ListId::Tools => {}
        }
    }

    /// The mouse wheel over the step details scrolls them.
    pub fn on_wheel(&mut self, down: bool) {
        self.scroll = if down {
            self.scroll.saturating_add(3)
        } else {
            self.scroll.saturating_sub(3)
        };
    }

    fn move_by(&mut self, delta: isize) {
        let moved =
            |at: usize, len: usize| at.saturating_add_signed(delta).min(len.saturating_sub(1));
        match self.focus {
            Focus::Tasks => {
                let task = moved(self.task, self.tasks.len());
                if task != self.task {
                    self.task = task;
                    // A new task opens at its latest step.
                    self.step = self.last_step();
                    self.sync_choice();
                }
            }
            // The log takes its arrow keys before this (see `log_key`).
            Focus::Input | Focus::Log => {}
            Focus::Steps => {
                let len = self.current().map_or(0, |t| t.steps.len());
                self.step = moved(self.step, len);
            }
        }
        self.scroll = 0;
    }

    /// A click on a window's title: over the whole tab, or back.
    pub fn toggle_zoom(&mut self, zoom: Zoom) {
        self.zoom = if self.zoom == Some(zoom) {
            None
        } else {
            Some(zoom)
        };
        // The log over the whole tab: its keys scroll it at once.
        if self.zoom == Some(Zoom::Log) {
            self.focus = Focus::Log;
        }
    }

    /// The window shown over the whole tab; the log only while there is one.
    pub(super) fn zoomed(&self) -> Option<Zoom> {
        self.zoom.filter(|z| *z != Zoom::Log || self.has_log())
    }

    /// The page of the pull request of the task at `index`, if it has one
    /// that is really on GitHub (see `PullRequest::page`).
    pub fn pull_request_url(&self, index: usize) -> Option<String> {
        let pull_request = self.tasks.get(index)?.state.pull_request.as_ref()?;
        pull_request.page().map(str::to_string)
    }

    /// A click on link `index` of the selected step's «Files»: the app opens it.
    pub fn open_file(&mut self, index: usize) {
        let step = self.current().and_then(|t| t.steps.get(self.step)).cloned();
        if let Some(file) = step.and_then(|s| self.artifacts(&s).into_iter().nth(index)) {
            self.open = Some(file.path);
        }
    }
}
