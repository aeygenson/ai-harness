//! The Agents tab: the catalog of console agents, with the ones installed on
//! this computer marked (see `harness_agents::catalog`). It does not need an
//! open project: agents are installed on the computer, not in a project.

use harness_agents::catalog::{self, Status, CATALOG};
use harness_agents::credentials;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::tasks::draw_list;
use crate::theme;
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
        buttons(
            frame,
            bar,
            hits,
            &[(check, ButtonId::AgentsCheck, !self.checking)],
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
                        theme::dim(),
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
        let state = if !self.known {
            Line::styled(tr.t("agents.checking").to_string(), theme::dim())
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
        } else {
            lines.push(Line::styled(
                tr.t("agents.not_run").to_string(),
                theme::dim(),
            ));
        }
        match status.login {
            Some(true) if self.known => {
                lines.push(Line::styled(tr.t("agents.login").to_string(), theme::ok()));
            }
            Some(false) if self.known => {
                let command = credentials::login_command(entry.id);
                let text = tr.f("agents.no_login", &[("command", &command)]);
                lines.push(Line::styled(text, theme::warn()));
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
        let (key, commands) = if status.installed() {
            ("agents.update", entry.update)
        } else {
            ("agents.install", entry.install)
        };
        match commands.here() {
            Some(command) => {
                lines.push(Line::from(tr.t(key).to_string()));
                lines.push(Line::styled(format!("  {command}"), theme::accent()));
            }
            None => lines.push(Line::from(
                tr.f("agents.see_site", &[("site", &entry.site)]),
            )),
        }
        lines.push(Line::from(tr.f("agents.site", &[("site", &entry.site)])));
        lines
    }
}

/// Checks the computer: which agents are installed, their versions, logins.
pub type AgentChecker = fn(Option<&std::path::Path>) -> Vec<Status>;

pub fn check(credentials_dir: Option<&std::path::Path>) -> Vec<Status> {
    catalog::check_all(credentials_dir)
}
