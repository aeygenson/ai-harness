//! Drawing the whole screen: the tab bar, the open tab, the form on top and the
//! footer with messages and hot keys.

use harness_core::projects::name_of;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::tasks::TasksTab;
use crate::ui::{self, theme};
use crate::ui::{buttons, panel, ButtonId, Target};
use crate::{App, Tab, TABS};

impl App {
    /// Draws everything and registers every clickable place in the hit map.
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        self.hits.clear();
        frame.render_widget(
            ratatui::widgets::Block::new().style(theme::base()),
            frame.area(),
        );
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
            Tab::Skills => match (&self.skills, &self.roles) {
                (Some(skills), Some(roles)) => {
                    skills.draw(frame, main, &mut self.hits, &self.tr, roles);
                }
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.skills")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Mcp => match (&self.mcp, &self.roles) {
                (Some(mcp), Some(roles)) => mcp.draw(frame, main, &mut self.hits, &self.tr, roles),
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.mcp")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Plugins => match (&self.plugins, &self.roles) {
                (Some(plugins), Some(roles)) => {
                    plugins.draw(frame, main, &mut self.hits, &self.tr, roles);
                }
                _ => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.plugins")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Retro => match &self.retro {
                Some(retro) => retro.draw(frame, main, &mut self.hits, &self.tr),
                None => placeholder(
                    frame,
                    main,
                    &format!(" {} ", self.tr.t("tabs.retro")),
                    self.tr.t("tabs.no_open_project"),
                ),
            },
            Tab::Agents => self.agents.draw(frame, main, &mut self.hits, &self.tr),
            Tab::Projects => {
                self.projects.draw(
                    frame,
                    main,
                    &mut self.hits,
                    self.project.as_deref(),
                    &self.tr,
                );
            }
        }

        // The latest result or problem first, so a narrow window still shows it.
        let mut spans = Vec::new();
        let problem = self
            .tasks
            .as_ref()
            .and_then(|t| t.problem.clone())
            .or_else(|| self.skills.as_ref().and_then(|s| s.problem.clone()))
            .or_else(|| self.projects.problem.clone())
            .or_else(|| self.tr.problems.first().cloned());
        if let Some((text, error)) = &self.message {
            let (mark, style) = if *error {
                ("✗", theme::bad())
            } else {
                ("✓", theme::ok())
            };
            spans.push(Span::styled(
                format!(" {mark} {text}  "),
                style.add_modifier(Modifier::BOLD),
            ));
        } else if let Some(problem) = problem {
            spans.push(Span::styled(
                format!(" ✗ {problem}  "),
                theme::bad().add_modifier(Modifier::BOLD),
            ));
        }
        let typing = self.tab == Tab::Tasks && self.tasks.as_ref().is_some_and(TasksTab::typing);
        let hint = if typing {
            "tasks.typing_hint"
        } else {
            "footer.hint"
        };
        // The key that lets the terminal select text while the TUI has the mouse.
        let copy = harness_platform::terminal::select_text_key();
        spans.push(Span::styled(
            format!(" {}", self.tr.f(hint, &[("copy", &copy)])),
            theme::dim(),
        ));
        frame.render_widget(Line::from(spans), footer);

        if let Some((_, form)) = &self.form {
            form.draw(frame, &mut self.hits, self.tr.t("form.cancel"));
        }
        if let Some((_, browser)) = &self.browser {
            browser.draw(frame, &mut self.hits, &self.tr);
        }
    }

    /// The top line: project name, the tabs and the buttons on the right.
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
        put(frame, format!(" ◆ {name} "), theme::primary());
        put(frame, " ".into(), Style::new());
        for (index, (tab, label)) in TABS.iter().enumerate() {
            if index > 0 {
                put(frame, "│".into(), theme::dim());
            }
            let style = if *tab == self.tab {
                theme::selected().patch(theme::accent())
            } else {
                theme::dim()
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
        let theme_label = format!("◐ {}", self.tr.t(theme::current().name));
        let mut items = [
            (self.tr.t("tabs.new_project"), ButtonId::NewProject),
            (self.tr.label(), ButtonId::Language),
            (theme_label.as_str(), ButtonId::Theme),
        ];
        let width = |items: &[(&str, ButtonId)]| -> u16 {
            items
                .iter()
                .map(|(label, _)| ui::button_width(label) + 1)
                .sum()
        };
        // In a narrow window the theme button shows only its sign.
        if width(&items) >= right.width {
            items[2].0 = "◐";
        }
        let width = width(&items);
        let start = right.right().saturating_sub(width);
        if start > right.x {
            let area = Rect::new(start, area.y, width, 1);
            let items: Vec<_> = items.iter().map(|(l, id)| (*l, *id, true)).collect();
            buttons(frame, area, &mut self.hits, &items);
        }
    }
}

/// A framed text where a tab has nothing to show, such as "no project open".
fn placeholder(frame: &mut Frame, area: Rect, title: &str, text: &str) {
    frame.render_widget(
        Paragraph::new(text.to_string())
            .block(panel(title, false))
            .wrap(Wrap { trim: false }),
        area,
    );
}
