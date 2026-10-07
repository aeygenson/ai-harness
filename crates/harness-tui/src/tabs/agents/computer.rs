//! The «Computer» panel under the agent list: the other programs the harness
//! needs (Git, Node.js, ...), the same check as `harness doctor`, and the
//! installer command when something is missing.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use harness_agents::install::tools::{Health, ToolStatus};

use super::tab::AgentsTab;
use crate::ui::i18n::I18n;
use crate::ui::{panel, theme};

impl AgentsTab {
    /// The panel's lines: one per program, then the fix if one is needed.
    pub fn computer_lines(&self, tr: &I18n) -> Vec<Line<'static>> {
        if self.tools.is_empty() {
            return vec![Line::styled(
                tr.t("agents.checking").to_string(),
                theme::dim(),
            )];
        }
        let mut lines: Vec<Line> = self.tools.iter().map(|s| tool_line(s, tr)).collect();
        if self.tools.iter().any(|s| s.health() != Health::Good) {
            lines.push(Line::from(tr.t("agents.tools_install").to_string()));
            lines.push(Line::styled(
                harness_platform::program::installer_command().to_string(),
                theme::accent(),
            ));
        }
        lines
    }

    /// Draws the panel into `area`.
    pub fn draw_computer(&self, frame: &mut Frame, area: Rect, tr: &I18n) {
        frame.render_widget(
            Paragraph::new(self.computer_lines(tr))
                .block(panel(tr.t("agents.computer"), false))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// How many rows the panel needs at `width` columns, with its border:
/// wrapped lines take more than one row.
pub fn computer_height(lines: &[Line], width: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(2)).max(1);
    let rows: usize = lines
        .iter()
        .map(|l| wrapped_rows(&l.to_string(), inner))
        .sum();
    u16::try_from(rows + 2).unwrap_or(u16::MAX)
}

/// The rows `text` takes when wrapped at word ends into `width` columns,
/// the way the panel's `Wrap` does it: a word that does not fit on the row
/// goes to the next one, a word longer than a row is cut.
fn wrapped_rows(text: &str, width: usize) -> usize {
    let mut rows = 1;
    let mut used = 0;
    for word in text.split(' ') {
        let length = word.chars().count();
        let needed = if used == 0 { length } else { used + 1 + length };
        if needed <= width {
            used = needed;
            continue;
        }
        // An empty row is not left behind for a long word.
        if used > 0 {
            rows += 1;
        }
        used = length;
        while used > width {
            rows += 1;
            used -= width;
        }
    }
    rows
}

/// `✓ Git 2.43.0`, `! Node.js 20.1.0 · older than 22.19`, `✗ npx · not found`.
fn tool_line(status: &ToolStatus, tr: &I18n) -> Line<'static> {
    let (mark, style) = match status.health() {
        Health::Good => ("✓", theme::ok()),
        Health::Warning => ("!", theme::warn()),
        Health::Missing => ("✗", theme::bad()),
    };
    let about = match (&status.path, &status.version, status.tool.min_version()) {
        (None, _, _) => tr.t("agents.tool_missing").to_string(),
        (Some(_), Some(version), Some(min)) if status.old() => {
            format!("{version} · {}", tr.f("agents.tool_old", &[("min", &min)]))
        }
        (Some(_), Some(version), _) => version.clone(),
        (Some(_), None, _) => String::new(),
    };
    Line::from(vec![
        Span::styled(format!("{mark} "), style),
        Span::raw(format!("{} ", status.tool.name())),
        Span::styled(about, style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_that_do_not_fit_go_to_the_next_row() {
        assert_eq!(wrapped_rows("", 10), 1);
        assert_eq!(wrapped_rows("one two", 10), 1);
        // "one two" fits, "three" does not.
        assert_eq!(wrapped_rows("one two three", 10), 2);
        // A word of 25 letters takes three rows of 10.
        assert_eq!(wrapped_rows(&"x".repeat(25), 10), 3);
    }
}
