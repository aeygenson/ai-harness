//! The text of the message box: drawing it, scrolling it and clearing it.
//!
//! A long text (often a pasted one) does not fit in the few rows of the box.
//! The box shows its end, where one types; the mouse wheel over the box and
//! Page Up / Page Down while writing scroll back to the start. «✗ Clear» and Ctrl+U
//! remove the whole text at once.

use std::cell::Cell;

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::message_box::wrap;
use super::tab::{Focus, TasksTab};
use crate::ui::i18n::I18n;
use crate::ui::theme;

/// Lines one turn of the mouse wheel scrolls, as in the other windows.
const WHEEL_LINES: usize = 3;

/// How far the message is scrolled back from its end.
#[derive(Debug, Default)]
pub(super) struct MessageScroll {
    /// Lines of the text hidden below the box: 0 shows the end.
    up: usize,
    /// The lines the text took and the rows the box had when it was last
    /// drawn: scrolling needs them to know where the start is. `Cell` lets
    /// the drawing, which only borrows the tab (`&self`), write them down.
    seen: Cell<(usize, usize)>,
}

impl MessageScroll {
    /// How many lines can be hidden below the box at most.
    fn most(&self) -> usize {
        let (lines, rows) = self.seen.get();
        lines.saturating_sub(rows)
    }
}

impl TasksTab {
    /// Removes the whole message; the box keeps the focus for a new one.
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_scroll.up = 0;
        self.focus = Focus::Input;
    }

    /// The text was changed: the box shows its end again, where one types.
    pub(super) fn input_changed(&mut self) {
        self.input_scroll.up = 0;
    }

    /// Scrolls the message `lines` lines towards its start (`back`) or its end.
    pub(super) fn scroll_input(&mut self, back: bool, lines: usize) {
        let scroll = &mut self.input_scroll;
        scroll.up = if back {
            scroll.up.saturating_add(lines).min(scroll.most())
        } else {
            scroll.up.saturating_sub(lines)
        };
    }

    /// The mouse wheel over the message box scrolls its text.
    pub fn on_input_wheel(&mut self, down: bool) {
        self.scroll_input(!down, WHEEL_LINES);
    }

    /// Page Up and Page Down while writing: the rows of the box at a time.
    pub(super) fn page_input(&mut self, back: bool) {
        let rows = self.input_scroll.seen.get().1.max(1);
        self.scroll_input(back, rows);
    }

    /// Draws the text of the message in `field`, or the hint when it is empty.
    /// `area` is the whole box: ▲ and ▼ on its right border say that more of
    /// the text is above or below.
    pub(super) fn draw_input_text(&self, frame: &mut Frame, field: Rect, area: Rect, tr: &I18n) {
        let typing = self.focus == Focus::Input;
        if self.input.is_empty() && !typing {
            let hint = Line::styled(format!(" {}", tr.t("tasks.input_hint")), theme::dim());
            frame.render_widget(Paragraph::new(hint), field);
            self.input_scroll.seen.set((1, usize::from(field.height)));
            return;
        }
        let cursor = if typing { "▏" } else { "" };
        let room = usize::from(field.width.saturating_sub(2)).max(1);
        let wrapped = wrap(&format!("{}{cursor}", self.input), room);
        let rows = usize::from(field.height);
        self.input_scroll.seen.set((wrapped.len(), rows));
        // `up` may be more than the text allows now (the window got taller).
        let up = self.input_scroll.up.min(self.input_scroll.most());
        let skip = self.input_scroll.most() - up;
        let lines: Vec<Line> = wrapped
            .into_iter()
            .skip(skip)
            .take(rows)
            .map(|l| Line::from(format!(" {l}")))
            .collect();
        frame.render_widget(Paragraph::new(lines), field);

        let border = area.right().saturating_sub(1);
        if skip > 0 {
            mark(frame, border, field.y, "▲");
        }
        if up > 0 && field.height > 0 {
            mark(frame, border, field.bottom() - 1, "▼");
        }
    }
}

/// Draws a one-column `sign` at column `x`, row `y`, in the accent colour.
fn mark(frame: &mut Frame, x: u16, y: u16, sign: &str) {
    let place = Rect::new(x, y, 1, 1);
    frame.render_widget(Paragraph::new(Line::styled(sign, theme::accent())), place);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with_lines(lines: usize, rows: usize) -> TasksTab {
        let folder = tempfile::tempdir().unwrap();
        let mut tab = TasksTab::load(folder.path(), None);
        tab.input = "x\n".repeat(lines - 1) + "x";
        tab.input_scroll.seen.set((lines, rows));
        tab
    }

    #[test]
    fn scrolling_back_stops_at_the_start_and_forward_at_the_end() {
        let mut tab = tab_with_lines(10, 4);
        tab.scroll_input(true, 100);
        assert_eq!(tab.input_scroll.up, 6);
        tab.scroll_input(false, 2);
        assert_eq!(tab.input_scroll.up, 4);
        tab.scroll_input(false, 100);
        assert_eq!(tab.input_scroll.up, 0);
    }

    #[test]
    fn a_page_is_the_rows_of_the_box() {
        let mut tab = tab_with_lines(20, 5);
        tab.page_input(true);
        assert_eq!(tab.input_scroll.up, 5);
        tab.on_input_wheel(true);
        assert_eq!(tab.input_scroll.up, 2);
    }

    #[test]
    fn a_short_text_does_not_scroll() {
        let mut tab = tab_with_lines(3, 5);
        tab.on_input_wheel(false);
        assert_eq!(tab.input_scroll.up, 0);
    }

    #[test]
    fn clearing_removes_the_text_and_goes_back_to_the_end() {
        let mut tab = tab_with_lines(10, 4);
        tab.scroll_input(true, 3);
        tab.clear_input();
        assert_eq!(tab.input, "");
        assert_eq!(tab.input_scroll.up, 0);
        assert!(tab.typing());
    }
}
