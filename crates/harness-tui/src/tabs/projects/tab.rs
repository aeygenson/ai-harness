//! The Projects tab: the list from `~/.harness/projects.toml`, with what is
//! going on in each project (a running task, tasks waiting for the user),
//! read from disk, so the work of other windows shows too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use harness_core::config::projects::{Project, Projects};
use harness_core::config::CONFIG_FILE;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::task::overview::{self, Overview};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, draw_list, panel, ButtonId, Hits, ListId, ListView};

#[derive(Debug)]
pub struct ProjectsTab {
    /// `~/.harness`; without it nothing can be saved.
    home: Option<PathBuf>,
    pub list: Projects,
    pub selected: usize,
    pub problem: Option<String>,
    /// What is going on in each project, by folder (see [`Self::refresh`]).
    overviews: HashMap<PathBuf, Overview>,
}

/// Is there a harness project in `root`?
pub fn has_config(root: &Path) -> bool {
    root.join(HARNESS_DIR).join(CONFIG_FILE).is_file()
}

impl ProjectsTab {
    pub fn load(home: Option<PathBuf>, tr: &I18n) -> Self {
        let mut tab = Self {
            home,
            list: Projects::default(),
            selected: 0,
            problem: None,
            overviews: HashMap::new(),
        };
        tab.reload(tr);
        tab
    }

    /// Reads the list again from `~/.harness/projects.toml`.
    pub fn reload(&mut self, tr: &I18n) {
        self.problem = None;
        match &self.home {
            Some(home) => match Projects::load(home) {
                Ok(list) => self.list = list,
                Err(error) => self.problem = Some(error.to_string()),
            },
            None => self.problem = Some(tr.t("errors.no_home").to_string()),
        }
        self.selected = self
            .selected
            .min(self.list.projects.len().saturating_sub(1));
    }

    /// Changes the list with `change` and saves it. The list is read from
    /// the file again first, so what another window added is kept.
    pub fn update(&mut self, change: impl FnOnce(&mut Projects), tr: &I18n) -> Result<(), String> {
        let Some(home) = &self.home else {
            return Err(tr.t("errors.no_home").to_string());
        };
        self.list = Projects::change(home, change).map_err(|e| e.to_string())?;
        self.selected = self
            .selected
            .min(self.list.projects.len().saturating_sub(1));
        Ok(())
    }

    /// Reads again what is going on in every project on the list. A
    /// project that cannot be read (a missing folder, no settings) shows
    /// nothing.
    pub fn refresh(&mut self) {
        self.overviews = self
            .list
            .projects
            .iter()
            .filter(|p| has_config(&p.path))
            .filter_map(|p| {
                let repo = Repo::open(&p.path).ok()?;
                let seen = overview::overview(&repo).ok()?;
                Some((p.path.clone(), seen))
            })
            .collect();
    }

    /// The short state of a project after its name: the running task and
    /// how many tasks wait for the user; empty when nothing goes on.
    fn status(&self, path: &Path, tr: &I18n) -> String {
        let Some(seen) = self.overviews.get(path) else {
            return String::new();
        };
        let mut parts = Vec::with_capacity(2);
        if let Some(working) = &seen.working {
            parts.push(tr.f("projects.row_running", &[("task", &working.task)]));
        }
        if !seen.waiting.is_empty() {
            let count = seen.waiting.len();
            parts.push(tr.f("projects.row_waiting", &[("count", &count)]));
        }
        parts.join(" · ")
    }

    /// The running task and the waiting tasks of a project, one line each.
    fn details(&self, path: &Path, tr: &I18n) -> Vec<Line<'static>> {
        let Some(seen) = self.overviews.get(path) else {
            return vec![];
        };
        let mut lines = Vec::with_capacity(2);
        if let Some(working) = &seen.working {
            let text = tr.f("projects.about_running", &[("task", &working.task)]);
            lines.push(Line::styled(text, theme::accent()));
        }
        if !seen.waiting.is_empty() {
            let tasks = seen.waiting.join(", ");
            let text = tr.f("projects.about_waiting", &[("tasks", &tasks)]);
            lines.push(Line::styled(text, theme::warn()));
        }
        lines
    }

    pub fn current(&self) -> Option<&Project> {
        self.list.projects.get(self.selected)
    }

    pub fn on_key(&mut self, key: KeyCode) {
        let last = self.list.projects.len().saturating_sub(1);
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            _ => {}
        }
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        hits: &mut Hits,
        open: Option<&Path>,
        tr: &I18n,
    ) {
        let [bar, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let has_selection = self.current().is_some();
        buttons(
            frame,
            bar,
            hits,
            &[
                (tr.t("projects.open"), ButtonId::UseProject, has_selection),
                (tr.t("projects.new"), ButtonId::NewProject, true),
                (tr.t("projects.open_folder"), ButtonId::OpenFolder, true),
                (
                    tr.t("projects.connect_github"),
                    ButtonId::ConnectGitHub,
                    has_selection,
                ),
                (
                    tr.t("projects.remove"),
                    ButtonId::RemoveProject,
                    has_selection,
                ),
            ],
        );
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);

        let items: Vec<ListItem> = self
            .list
            .projects
            .iter()
            .map(|p| {
                let mark = if Some(p.path.as_path()) == open {
                    "● "
                } else {
                    "  "
                };
                let status = self.status(&p.path, tr);
                if status.is_empty() {
                    ListItem::new(format!("{mark}{}", p.name))
                } else {
                    ListItem::new(format!("{mark}{}  {status}", p.name))
                }
            })
            .collect();
        draw_list(
            frame,
            hits,
            left,
            ListView {
                id: ListId::Projects,
                title: tr.t("projects.title"),
                items,
                selected_row: self.selected,
                focused: true,
            },
        );

        let text: Vec<Line> = match self.current() {
            None => tr
                .t("projects.empty")
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect(),
            Some(project) => {
                let state = if !project.path.is_dir() {
                    Line::styled(tr.t("projects.missing").to_string(), theme::bad())
                } else if has_config(&project.path) {
                    Line::styled(tr.t("projects.ready").to_string(), theme::ok())
                } else {
                    Line::styled(tr.t("projects.no_config").to_string(), theme::warn())
                };
                let mut lines = vec![
                    Line::from(project.name.clone()),
                    Line::from(project.path.display().to_string()),
                    Line::default(),
                    state,
                ];
                lines.extend(self.details(&project.path, tr));
                lines.push(Line::default());
                lines.push(Line::from(tr.t("projects.open_hint").to_string()));
                lines
            }
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(panel(tr.t("projects.about"), false))
                .wrap(Wrap { trim: false }),
            right,
        );
    }
}
