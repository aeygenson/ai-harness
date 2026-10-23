//! The live log panel of the Tasks tab: its title (the running role and how
//! long it has worked), the lines, and scrolling them with the wheel and keys.
//!
//! The wheel over the panel scrolls it; a click gives it the focus, and then
//! ↑↓ (or k j) and PageUp/PageDown scroll it. Home and End go to the oldest
//! and the newest line from anywhere outside the message box. While Lisa is
//! scrolled up, new lines do not move her view and the title says how to get
//! back to the newest one.

use std::time::{Duration, SystemTime};

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use harness_core::task::handoff::Role;
use harness_core::task::Stage;

use super::tab::{Focus, TasksTab, Zoom};
use super::times::{between, duration_label, TaskTimes};
use crate::ui::i18n::I18n;
use crate::ui::{panel, Hits, Target};

/// Lines one turn of the wheel scrolls, as in the other panels.
const WHEEL_LINES: isize = 3;

/// Lines `PageUp` and `PageDown` scroll, as much as in the step's text.
const PAGE_LINES: isize = 10;

impl TasksTab {
    /// Is the live log on the screen? From the start of a run until the next one.
    pub(super) fn has_log(&self) -> bool {
        self.running.is_some() || !self.log.is_empty()
    }

    /// What the agent prints, the newest lines at the bottom unless Lisa
    /// scrolled up. The whole panel takes the wheel and clicks.
    pub(super) fn draw_log(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        if area.height == 0 {
            return;
        }
        let inner = panel("", false).inner(area);
        // The view first: it decides whether the log still follows.
        let (lines, hidden) = self.log.view(inner.width, inner.height);
        let title = self.log_title(tr);
        let hint = if !self.log.following() {
            Some(tr.t("tasks.log_not_newest"))
        } else if self.focus == Focus::Log {
            Some(tr.t("tasks.log_keys"))
        } else {
            None
        };
        let title = match hint {
            Some(hint) => format!("{} · {hint}", title.trim()),
            None => title,
        };
        let title = self.zoom_title(Zoom::Log, &title, area, hits);
        hits.add(inner, Target::Log);
        let focused = self.focus == Focus::Log || self.running.is_some();
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(&title, focused))
                .wrap(Wrap { trim: false })
                .scroll((hidden, 0)),
            area,
        );
    }

    /// «Agent log», or the running task with its role and how long it has worked.
    fn log_title(&self, tr: &I18n) -> String {
        let Some(running) = &self.running else {
            return tr.t("tasks.log_title").to_string();
        };
        let Some(task) = &running.task else {
            return tr.f("tasks.log_running", &[("task", &"…")]);
        };
        match self.running_role(task, running.started, SystemTime::now()) {
            Some((role, took)) => tr.f(
                "tasks.log_running_role",
                &[
                    ("task", task),
                    ("role", &role),
                    ("time", &duration_label(took, tr)),
                ],
            ),
            None => tr.f("tasks.log_running", &[("task", task)]),
        }
    }

    /// The role working on `task` and how long it has worked at `now`: since
    /// the run `started` or the task's last commit (the step before it),
    /// whichever came later. `None` when no role works on it.
    pub(super) fn running_role(
        &self,
        task: &str,
        started: SystemTime,
        now: SystemTime,
    ) -> Option<(Role, Duration)> {
        let view = self.all.iter().find(|t| t.id == task)?;
        let Stage::Working(role) = view.state.stage else {
            return None;
        };
        let last = self.times_of(task).and_then(TaskTimes::last);
        let start = last.map_or(started, |last| last.max(started));
        Some((role, between(start, now)))
    }

    /// A key while the log has the focus; `false` when it does not scroll it.
    pub(super) fn log_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.log.scroll(1),
            KeyCode::Down | KeyCode::Char('j') => self.log.scroll(-1),
            KeyCode::PageUp => self.log.scroll(PAGE_LINES),
            KeyCode::PageDown => self.log.scroll(-PAGE_LINES),
            _ => return false,
        }
        true
    }

    /// The wheel over the log scrolls it, and the log gets the focus.
    pub fn on_log_wheel(&mut self, down: bool) {
        self.focus = Focus::Log;
        self.log
            .scroll(if down { -WHEEL_LINES } else { WHEEL_LINES });
    }

    /// A click on the log: its keys scroll it now.
    pub fn focus_log(&mut self) {
        self.focus = Focus::Log;
    }
}
