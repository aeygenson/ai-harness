//! The «Проекты» tab: the list from `~/.harness/projects.toml`.

use std::path::{Path, PathBuf};

use harness_core::config::CONFIG_FILE;
use harness_core::git::HARNESS_DIR;
use harness_core::projects::{Project, Projects};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tasks::draw_list;
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
    pub fn load(home: Option<PathBuf>) -> Self {
        let mut tab = Self {
            home,
            list: Projects::default(),
            selected: 0,
            problem: None,
        };
        tab.reload();
        tab
    }

    pub fn reload(&mut self) {
        self.problem = None;
        match &self.home {
            Some(home) => match Projects::load(home) {
                Ok(list) => self.list = list,
                Err(error) => self.problem = Some(error.to_string()),
            },
            None => self.problem = Some("HOME is not set".into()),
        }
        self.selected = self
            .selected
            .min(self.list.projects.len().saturating_sub(1));
    }

    /// Changes the list with `change` and saves it.
    pub fn update(&mut self, change: impl FnOnce(&mut Projects)) -> Result<(), String> {
        let Some(home) = &self.home else {
            return Err("HOME is not set".into());
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

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, open: Option<&Path>) {
        let [bar, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        let has_selection = self.current().is_some();
        buttons(
            frame,
            bar,
            hits,
            &[
                ("Открыть", ButtonId::UseProject, has_selection),
                ("Новый проект", ButtonId::NewProject, true),
                ("Открыть папку…", ButtonId::OpenFolder, true),
                ("Убрать из списка", ButtonId::RemoveProject, has_selection),
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
            " Проекты ",
            items,
            self.selected,
            true,
        );

        let text: Vec<Line> = match self.current() {
            None => vec![
                Line::from("Проектов в списке пока нет."),
                Line::default(),
                Line::from("«Новый проект» создаст папку, git и настройки harness."),
                Line::from("«Открыть папку…» добавит существующую папку."),
            ],
            Some(project) => {
                let state = if !project.path.is_dir() {
                    Line::styled("Папки больше нет.", Style::new().fg(Color::Red))
                } else if has_config(&project.path) {
                    Line::styled("Проект harness.", Style::new().fg(Color::Green))
                } else {
                    Line::styled(
                        "В папке нет .harness/harness.toml.",
                        Style::new().fg(Color::Yellow),
                    )
                };
                vec![
                    Line::from(project.name.clone()),
                    Line::from(project.path.display().to_string()),
                    Line::default(),
                    state,
                    Line::default(),
                    Line::from("Двойной клик или Enter открывает проект."),
                ]
            }
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(panel(" О проекте ", false))
                .wrap(Wrap { trim: false }),
            right,
        );
    }
}
