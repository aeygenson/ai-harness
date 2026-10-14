//! The bottom of the screen: the latest message or problem, then the hot keys.
//! A message too long for one line can be opened over several lines (⤢) and
//! closed again (⤡), like the windows of the Tasks tab.

use ratatui::layout::{Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::tasks::TasksTab;
use crate::ui::message::MessageKind;
use crate::ui::{theme, ButtonId, Target};
use crate::{App, Tab};

/// The mark in front of a long message: a click opens it over several lines.
const OPEN_MARK: &str = " ⤢";
/// The mark in front of an opened message: a click puts it back on one line.
const CLOSE_MARK: &str = " ⤡";

/// How the message at the bottom is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusView {
    /// On one line, cut off at the right edge when it is too long.
    #[default]
    OneLine,
    /// Over as many lines as it needs (up to half the screen).
    Open,
}

impl App {
    /// How many rows the bottom takes: one, or the opened message plus the hot keys.
    /// An opened message takes at most half the screen, so the tab stays usable.
    pub(super) fn footer_height(&self, width: u16, screen_height: u16) -> u16 {
        let Some((text, _)) = self
            .status()
            .filter(|_| self.status_view == StatusView::Open)
        else {
            return 1;
        };
        let rows = status_paragraph(&text, Style::new()).line_count(width);
        let rows = u16::try_from(rows).unwrap_or(u16::MAX);
        rows.saturating_add(1).clamp(1, (screen_height / 2).max(1))
    }

    /// Draws the bottom: the message (opened or on one line), then the hot keys.
    pub(super) fn draw_footer(&mut self, frame: &mut Frame, footer: Rect) {
        let hint = Span::styled(format!(" {}", self.hint()), theme::dim());
        let Some((text, style)) = self.status() else {
            frame.render_widget(Line::from(hint), footer);
            return;
        };
        if self.status_view == StatusView::Open && footer.height > 1 {
            let [message, keys] =
                Layout::vertical([ratatui::layout::Constraint::Min(0), 1.into()]).areas(footer);
            frame.render_widget(status_paragraph(&text, style), message);
            frame.render_widget(Line::from(hint), keys);
            self.add_mark(message, CLOSE_MARK);
            return;
        }
        // On one line: the ⤢ mark only when the message does not fit.
        let width = usize::from(footer.width);
        let mut spans = Vec::with_capacity(3);
        if self.status_view == StatusView::Open || text.chars().count() + 2 > width {
            spans.push(Span::styled(OPEN_MARK, style));
            self.add_mark(footer, OPEN_MARK);
        }
        spans.push(Span::styled(format!("{text}  "), style));
        spans.push(hint);
        frame.render_widget(Line::from(spans), footer);
    }

    /// Opens a long message over several lines, or puts it back on one line.
    pub(crate) fn toggle_status(&mut self) {
        self.status_view = match self.status_view {
            StatusView::OneLine if self.status().is_some() => StatusView::Open,
            StatusView::OneLine | StatusView::Open => StatusView::OneLine,
        };
    }

    /// The message to show at the bottom with its style: the latest result, or
    /// else a problem of an open tab.
    fn status(&self) -> Option<(String, Style)> {
        if let Some(message) = &self.message {
            let (mark, style) = match message.kind {
                MessageKind::Error => ("✗", theme::bad()),
                MessageKind::Info => ("✓", theme::ok()),
            };
            return Some((
                format!(" {mark} {}", message.text),
                style.add_modifier(Modifier::BOLD),
            ));
        }
        self.tasks
            .as_ref()
            .and_then(|t| t.problem.clone())
            .or_else(|| self.skills.as_ref().and_then(|s| s.problem.clone()))
            .or_else(|| self.projects.problem.clone())
            .or_else(|| self.tr.problems.first().cloned())
            .map(|problem| {
                (
                    format!(" ✗ {problem}"),
                    theme::bad().add_modifier(Modifier::BOLD),
                )
            })
    }

    /// The hot keys for the bottom line; other keys while writing a message.
    fn hint(&self) -> String {
        let typing = self.tab == Tab::Tasks && self.tasks.as_ref().is_some_and(TasksTab::typing);
        let hint = if typing {
            "tasks.typing_hint"
        } else {
            "footer.hint"
        };
        // The key that lets the terminal select text while the TUI has the mouse.
        let copy = harness_platform::terminal::select_text_key();
        self.tr.f(hint, &[("copy", &copy)])
    }

    /// Makes the mark at the top left of `area` clickable, with a column more
    /// on its right so a click next to the narrow sign still hits it.
    fn add_mark(&mut self, area: Rect, mark: &str) {
        let width = u16::try_from(mark.chars().count() + 1).unwrap_or(u16::MAX);
        let rect = Rect::new(area.x, area.y, width.min(area.width), 1.min(area.height));
        self.hits.add(rect, Target::Button(ButtonId::Status));
    }
}

/// The opened message, wrapped at word ends, with the ⤡ mark in front.
fn status_paragraph(text: &str, style: Style) -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(CLOSE_MARK, style),
        Span::styled(text.to_string(), style),
    ]))
    .wrap(Wrap { trim: false })
}
