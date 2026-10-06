//! The Projects tab: the list from `~/.harness/projects.toml`.

use std::path::{Path, PathBuf};

use harness_core::config::projects::{Project, Projects};
use harness_core::config::CONFIG_FILE;
use harness_core::git::HARNESS_DIR;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId};

#[derive(Debug)]
pub struct ProjectsTab {
    /// `~/.harness`; without it nothing can be saved.
    home: Option<PathBuf>,
    pub list: Projects,
    pub selected: usize,
    pub problem: Option<String>,
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

    /// Changes the list with `change` and saves it.
    pub fn update(&mut self, change: impl FnOnce(&mut Projects), tr: &I18n) -> Result<(), String> {
        let Some(home) = &self.home else {
            return Err(tr.t("errors.no_home").to_string());
        };
        change(&mut self.list);
        self.list.save(home).map_err(|e| e.to_string())?;
        self.selected = self
            .selected
            .min(self.list.projects.len().saturating_sub(1));
        Ok(())
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
                ListItem::new(format!("{mark}{}", p.name))
            })
            .collect();
        draw_list(
            frame,
            hits,
            left,
            ListId::Projects,
            tr.t("projects.title"),
            items,
            self.selected,
            true,
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
                vec![
                    Line::from(project.name.clone()),
                    Line::from(project.path.display().to_string()),
                    Line::default(),
                    state,
                    Line::default(),
                    Line::from(tr.t("projects.open_hint").to_string()),
                ]
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
