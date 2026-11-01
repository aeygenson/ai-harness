//! The cursor of the message box: moving it and changing the text where it is.
//!
//! The cursor is a position in the text counted in characters, not bytes: a
//! Russian letter takes two bytes in a Rust `String`, and cutting a string in
//! the middle of a letter would panic. ← and → move it by one character, Home
//! and End to the start and end of the line, a click puts it where the mouse
//! is. Letters, Enter, a paste, Backspace and Delete change the text there.

use super::tab::TasksTab;

/// The mark drawn where the cursor is. It takes one cell, like a letter.
pub(super) const CURSOR: char = '▏';

impl TasksTab {
    /// The cursor, never past the end: the text may have been cleared or sent.
    pub(super) fn cursor(&self) -> usize {
        self.input_cursor.min(self.input.chars().count())
    }

    /// Puts `text` where the cursor is; the cursor goes after it.
    pub(super) fn insert_text(&mut self, text: &str) {
        let cursor = self.cursor();
        let at = byte_at(&self.input, cursor);
        self.input.insert_str(at, text);
        self.input_cursor = cursor + text.chars().count();
        self.input_changed();
    }

    /// Backspace: removes the character before the cursor.
    pub(super) fn delete_back(&mut self) {
        let cursor = self.cursor();
        if cursor == 0 {
            return;
        }
        self.input.remove(byte_at(&self.input, cursor - 1));
        self.input_cursor = cursor - 1;
        self.input_changed();
    }

    /// Delete: removes the character after the cursor.
    pub(super) fn delete_forward(&mut self) {
        let cursor = self.cursor();
        if cursor == self.input.chars().count() {
            return;
        }
        self.input.remove(byte_at(&self.input, cursor));
        self.input_changed();
    }

    /// ← and →: one character back or forward.
    pub(super) fn move_cursor(&mut self, forward: bool) {
        let cursor = self.cursor();
        self.input_cursor = if forward {
            (cursor + 1).min(self.input.chars().count())
        } else {
            cursor.saturating_sub(1)
        };
        self.show_cursor();
    }

    /// Home and End: the start or the end of the line the cursor is on.
    pub(super) fn cursor_to_line_edge(&mut self, end: bool) {
        let chars: Vec<char> = self.input.chars().collect();
        let mut cursor = self.cursor();
        if end {
            while cursor < chars.len() && chars[cursor] != '\n' {
                cursor += 1;
            }
        } else {
            while cursor > 0 && chars[cursor - 1] != '\n' {
                cursor -= 1;
            }
        }
        self.input_cursor = cursor;
        self.show_cursor();
    }

    /// A click on row `row` of the text (counted from the top of the box) at
    /// screen column `column`: the cursor goes there. The rows hidden above
    /// the box and the box's place come from the last drawing, so a scrolled
    /// text puts the cursor where the user sees the letter.
    pub fn click_text(&mut self, row: u16, column: u16) {
        let seen = self.input_scroll.seen.get();
        let line = seen.skip + usize::from(row);
        // The text is drawn one column in from the box's left edge.
        let column = usize::from(column.saturating_sub(seen.left + 1));
        let drawn = if seen.with_cursor {
            with_cursor(&self.input, self.cursor())
        } else {
            self.input.clone()
        };
        let mut place = char_at(&drawn, seen.width, line, column);
        // In the drawn text everything after the cursor mark is one further on.
        if seen.with_cursor && place > self.cursor() {
            place -= 1;
        }
        self.input_cursor = place;
    }
}

/// The text as the box draws it while one writes: the cursor mark inside it.
pub(super) fn with_cursor(text: &str, cursor: usize) -> String {
    let mut shown = text.to_string();
    shown.insert(byte_at(text, cursor), CURSOR);
    shown
}

/// The byte where character number `index` starts; the end of the text when
/// there are fewer characters.
fn byte_at(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// The rows of `text` cut into lines of `width` characters, the same way as
/// `message_box::wrap`, each as the characters it holds: `start..end`.
fn rows(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let length = line.chars().count();
        if length == 0 {
            rows.push((start, start));
        }
        let mut from = 0;
        while from < length {
            let to = (from + width).min(length);
            rows.push((start + from, start + to));
            from = to;
        }
        // The line break itself.
        start += length + 1;
    }
    rows
}

/// The character at `column` of row `row` of `text` wrapped to `width`; the
/// end of that row when the column is past it, the last row when `row` is.
pub(super) fn char_at(text: &str, width: usize, row: usize, column: usize) -> usize {
    let rows = rows(text, width);
    let Some(&(start, end)) = rows.get(row).or(rows.last()) else {
        return 0;
    };
    (start + column).min(end)
}

/// The row of `text` wrapped to `width` that holds character `index`.
pub(super) fn row_of(text: &str, width: usize, index: usize) -> usize {
    rows(text, width)
        .iter()
        .rposition(|&(start, _)| start <= index)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with(text: &str, cursor: usize) -> TasksTab {
        let folder = tempfile::tempdir().unwrap();
        let mut tab = TasksTab::load(folder.path(), None);
        tab.input = text.to_string();
        tab.input_cursor = cursor;
        tab
    }

    #[test]
    fn letters_go_where_the_cursor_is() {
        let mut tab = tab_with("Привет мир", 6);

        tab.insert_text(",");
        tab.insert_text(" дорогой");

        assert_eq!(tab.input, "Привет, дорогой мир");
        assert_eq!(tab.cursor(), 15);
    }

    #[test]
    fn backspace_and_delete_remove_around_the_cursor() {
        let mut tab = tab_with("abcd", 2);

        tab.delete_back();
        assert_eq!((tab.input.as_str(), tab.cursor()), ("acd", 1));
        tab.delete_forward();
        assert_eq!((tab.input.as_str(), tab.cursor()), ("ad", 1));

        // At the edges there is nothing to remove.
        tab.input_cursor = 0;
        tab.delete_back();
        tab.input_cursor = 2;
        tab.delete_forward();
        assert_eq!(tab.input, "ad");
    }

    #[test]
    fn arrows_stop_at_the_ends_and_home_end_stay_on_the_line() {
        let mut tab = tab_with("one\ntwo\nthree", 5);

        tab.cursor_to_line_edge(false);
        assert_eq!(tab.cursor(), 4);
        tab.cursor_to_line_edge(true);
        assert_eq!(tab.cursor(), 7);
        tab.move_cursor(false);
        assert_eq!(tab.cursor(), 6);

        tab.input_cursor = 0;
        tab.move_cursor(false);
        assert_eq!(tab.cursor(), 0);
        tab.input_cursor = 13;
        tab.move_cursor(true);
        assert_eq!(tab.cursor(), 13);
    }

    #[test]
    fn a_cleared_text_puts_the_cursor_at_its_start() {
        let mut tab = tab_with("long text", 9);

        tab.input.clear();

        assert_eq!(tab.cursor(), 0);
    }

    #[test]
    fn a_click_after_the_cursor_mark_does_not_count_the_mark() {
        let mut tab = tab_with("abcdef", 1);
        tab.input_scroll
            .seen
            .set(crate::tabs::tasks::message_text::Seen {
                lines: 1,
                rows: 3,
                width: 10,
                with_cursor: true,
                ..Default::default()
            });

        // Drawn as " a▏bcdef": screen column 5 is «d».
        tab.click_text(0, 5);

        assert_eq!(tab.cursor(), 3);
    }

    #[test]
    fn rows_are_cut_like_the_drawing() {
        // "abcde" on rows of 2: ab, cd, e; then the empty line; then "x".
        let text = "abcde\n\nx";

        assert_eq!(rows(text, 2), vec![(0, 2), (2, 4), (4, 5), (6, 6), (7, 8)]);
        assert_eq!(char_at(text, 2, 1, 1), 3);
        assert_eq!(char_at(text, 2, 2, 9), 5);
        assert_eq!(char_at(text, 2, 3, 0), 6);
        assert_eq!(char_at(text, 2, 99, 0), 7);
        assert_eq!(row_of(text, 2, 3), 1);
        assert_eq!(row_of(text, 2, 6), 3);
        assert_eq!(with_cursor("ab", 1), "a▏b");
    }
}
