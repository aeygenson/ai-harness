//! The live log of the Tasks tab: what the agent prints, and where Lisa
//! scrolled to in it.
//!
//! The view is kept as the number of lines below it («back»): 0 follows the
//! newest line. Counting from the end means a new line only has to add one
//! to keep a scrolled view still, and whole lines (not screen rows) keep the
//! view right when a window resize wraps the lines differently.

use std::cell::Cell;
use std::collections::VecDeque;

use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::runner::push_line;

/// What the agent printed in the last run, and the scrolled view of it.
#[derive(Debug, Default)]
pub struct LiveLog {
    lines: VecDeque<String>,
    /// How many of the last lines are below the view; 0 shows the newest.
    back: usize,
    /// The largest useful `back` for the panel as last drawn: more would
    /// only scroll past the top. Drawing sets it (it takes `&self`), the
    /// keys and the wheel read it.
    max_back: Cell<usize>,
}

/// What the panel shows: the lines from the top of the view, and how many
/// screen rows of the first one are scrolled away (it may wrap over several).
pub type View<'a> = (Vec<Line<'a>>, u16);

impl LiveLog {
    /// Adds a line at the end. A scrolled view stays where it is.
    pub fn push(&mut self, line: String) {
        let full = self.lines.len() >= super::runner::LOG_LINES;
        push_line(&mut self.lines, line);
        if self.back > 0 {
            // The new line is below the view; when the oldest line was
            // dropped to make room, the view still has the same lines above.
            self.back = (self.back + 1).min(self.lines.len().saturating_sub(1));
        }
        if !full {
            // Drawing corrects it; until then the keys can reach the new line.
            self.max_back.set(self.max_back.get() + 1);
        }
    }

    /// Empties the log for a new run, following the newest line again.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.back = 0;
        self.max_back.set(0);
    }

    /// No line yet.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The view shows the newest line, and new lines move it along.
    pub fn following(&self) -> bool {
        self.back.min(self.max_back.get()) == 0
    }

    /// Moves the view `lines` lines towards older ones (a negative number:
    /// towards newer ones), never past either end.
    pub fn scroll(&mut self, lines: isize) {
        let back = self.back.min(self.max_back.get());
        self.back = back.saturating_add_signed(lines).min(self.max_back.get());
    }

    /// Back to the newest line: following again (End).
    pub fn go_to_newest(&mut self) {
        self.back = 0;
    }

    /// Up to the oldest line kept (Home).
    pub fn go_to_oldest(&mut self) {
        self.back = self.max_back.get();
    }

    /// The lines that fit in a panel `width` wide and `rows` high, long lines
    /// wrapped. Remembers how far up the view can go in this panel.
    pub fn view(&self, width: u16, rows: u16) -> View<'_> {
        let rows = usize::from(rows);
        let height = |line: &String| {
            Paragraph::new(line.as_str())
                .wrap(Wrap { trim: false })
                .line_count(width)
                .max(1)
        };
        // The first lines that fill the panel: the view can go up until they show.
        let mut filled = 0;
        let mut first_page = 0;
        for line in &self.lines {
            if filled >= rows {
                break;
            }
            filled += height(line);
            first_page += 1;
        }
        let max_back = self.lines.len() - first_page;
        self.max_back.set(max_back);
        let back = self.back.min(max_back);
        if back == max_back {
            // At the top (or everything fits): from the first line down.
            let lines = self.lines.iter().take(first_page);
            return (lines.map(String::as_str).map(Line::from).collect(), 0);
        }
        // Otherwise the line before `end` is at the bottom; the lines above
        // it are taken until the panel is full.
        let end = self.lines.len() - back;
        let mut start = end;
        let mut height_sum = 0;
        while start > 0 && height_sum < rows {
            start -= 1;
            height_sum += height(&self.lines[start]);
        }
        let hidden = u16::try_from(height_sum.saturating_sub(rows)).unwrap_or(u16::MAX);
        let lines = self.lines.range(start..end);
        let lines = lines.map(String::as_str).map(Line::from).collect();
        (lines, hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log with lines «0», «1», … «count - 1».
    fn log_of(count: usize) -> LiveLog {
        let mut log = LiveLog::default();
        for i in 0..count {
            log.push(i.to_string());
        }
        log
    }

    /// The texts of the view's lines.
    fn shown(log: &LiveLog, width: u16, rows: u16) -> (Vec<String>, u16) {
        let (lines, hidden) = log.view(width, rows);
        (lines.iter().map(ToString::to_string).collect(), hidden)
    }

    #[test]
    fn the_view_follows_the_newest_lines() {
        let log = log_of(10);
        assert_eq!(
            shown(&log, 20, 3),
            (vec!["7".into(), "8".into(), "9".into()], 0)
        );
        assert!(log.following());
        // Everything fits: from the top.
        let short = log_of(2);
        assert_eq!(shown(&short, 20, 5), (vec!["0".into(), "1".into()], 0));
        assert!(short.following());
    }

    #[test]
    fn scrolling_up_stops_following_and_new_lines_do_not_move_the_view() {
        let mut log = log_of(10);
        log.view(20, 3);
        log.scroll(2);
        assert!(!log.following());
        assert_eq!(shown(&log, 20, 3).0, ["5", "6", "7"]);

        log.push("10".into());
        log.push("11".into());
        assert_eq!(shown(&log, 20, 3).0, ["5", "6", "7"]);

        // Back to the end: it follows again.
        log.go_to_newest();
        assert!(log.following());
        assert_eq!(shown(&log, 20, 3).0, ["9", "10", "11"]);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut log = log_of(10);
        log.view(20, 3);
        log.scroll(100);
        assert_eq!(shown(&log, 20, 3).0, ["0", "1", "2"]);
        // One step down moves at once, however far up it was pushed.
        log.scroll(-1);
        assert_eq!(shown(&log, 20, 3).0, ["1", "2", "3"]);
        log.scroll(-100);
        assert!(log.following());
        log.go_to_oldest();
        assert_eq!(shown(&log, 20, 3).0, ["0", "1", "2"]);
    }

    #[test]
    fn a_taller_window_clamps_the_view_to_the_top() {
        let mut log = log_of(10);
        log.view(20, 3);
        log.go_to_oldest();
        // Taller: the oldest lines are on top and the panel is full, not
        // half empty.
        assert_eq!(
            shown(&log, 20, 8).0,
            ["0", "1", "2", "3", "4", "5", "6", "7"]
        );
        // The next step down moves the view at once.
        log.scroll(-1);
        assert_eq!(
            shown(&log, 20, 8).0,
            ["1", "2", "3", "4", "5", "6", "7", "8"]
        );
        // So tall everything fits: it follows.
        log.view(20, 20);
        assert!(log.following());
    }

    #[test]
    fn long_lines_wrap_and_the_newest_stays_whole_at_the_bottom() {
        let mut log = LiveLog::default();
        log.push("a".into());
        log.push("bbbbbbbbbb".into());
        log.push("c".into());
        // 5 wide: «bbbbb», «bbbbb»: four rows in all, three shown.
        assert_eq!(
            shown(&log, 5, 3),
            (vec!["bbbbbbbbbb".into(), "c".into()], 0)
        );
        assert_eq!(
            shown(&log, 5, 2),
            (vec!["bbbbbbbbbb".into(), "c".into()], 1)
        );
        // One line up: the long line at the bottom, «a» above it.
        log.scroll(1);
        assert_eq!(
            shown(&log, 5, 3),
            (vec!["a".into(), "bbbbbbbbbb".into()], 0)
        );
        // Wider: everything fits again.
        assert_eq!(shown(&log, 20, 3).0, ["a", "bbbbbbbbbb", "c"]);
        assert!(log.following());
    }

    #[test]
    fn a_full_log_keeps_a_scrolled_view_on_the_same_lines() {
        let count = super::super::runner::LOG_LINES;
        let mut log = log_of(count);
        log.view(20, 3);
        log.scroll(5);
        let before = shown(&log, 20, 3).0;
        log.push("new".into());
        assert_eq!(shown(&log, 20, 3).0, before);

        log.clear();
        assert!(log.is_empty());
        assert!(log.following());
    }
}
