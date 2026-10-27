//! The bordered, clickable list every tab draws its rows with.

use std::cell::Cell;

use ratatui::layout::Rect;
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use super::{panel, selected, Hits, ListId, Target};

/// The selection mark every list puts before its rows (see [`draw_list`]).
const LIST_MARK: &str = "▶ ";

/// Its width on the screen: every row of a list starts this far to the right.
pub const LIST_MARK_WIDTH: usize = 2;

/// What one list shows: which list it is, its title, its rows and the selection.
#[derive(Debug)]
pub struct ListView<'a> {
    /// Which list it is: a click on a row reports it.
    pub id: ListId,
    /// The title on the border.
    pub title: &'a str,
    /// The rows.
    pub items: Vec<ListItem<'a>>,
    /// The selected row, scrolled into view.
    pub selected_row: usize,
    /// The list has the keyboard focus: its border is in the accent colour.
    pub focused: bool,
}

/// Draws a bordered list whose rows can be clicked; returns the first row
/// on screen (the scroll offset).
///
/// The list scrolls only as far as the selection needs from the top. A list
/// that should stay where it is between frames uses [`draw_list_from`].
pub fn draw_list(frame: &mut Frame, hits: &mut Hits, area: Rect, list: ListView<'_>) -> usize {
    draw_list_from(frame, hits, area, list, &Cell::new(0))
}

/// Draws a list like [`draw_list`], starting at row `first` and saving there
/// the first row on screen.
///
/// The tab keeps `first` between frames, so the list does not jump when the
/// selection moves inside the rows already on screen (for example a click on
/// a row near the top). `Cell` lets drawing, which only borrows the tab,
/// change it.
pub fn draw_list_from(
    frame: &mut Frame,
    hits: &mut Hits,
    area: Rect,
    list: ListView<'_>,
    first: &Cell<usize>,
) -> usize {
    let ListView {
        id,
        title,
        items,
        selected_row,
        focused,
    } = list;
    let block = panel(title, focused);
    let inner = block.inner(area);
    // A list that got shorter (or a window that got taller) would otherwise
    // keep empty rows at the bottom while rows above are hidden.
    let most = items.len().saturating_sub(usize::from(inner.height));
    let mut state = ListState::default()
        .with_selected(Some(selected_row))
        .with_offset(first.get().min(most));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(selected())
            .highlight_symbol(LIST_MARK),
        area,
        &mut state,
    );
    hits.add(
        inner,
        Target::List {
            list: id,
            first: state.offset(),
        },
    );
    first.set(state.offset());
    state.offset()
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;

    /// Draws `rows` rows in a list 5 rows high inside, starting at `first`.
    fn draw(rows: usize, selected_row: usize, first: &Cell<usize>) -> usize {
        let mut terminal = Terminal::new(TestBackend::new(20, 7)).unwrap();
        let mut shown = 0;
        terminal
            .draw(|frame| {
                let items = (0..rows).map(|i| ListItem::new(i.to_string())).collect();
                let list = ListView {
                    id: ListId::Tasks,
                    title: "",
                    items,
                    selected_row,
                    focused: false,
                };
                shown = draw_list_from(frame, &mut Hits::default(), frame.area(), list, first);
            })
            .unwrap();
        shown
    }

    #[test]
    fn the_list_stays_where_it_was_while_the_selection_is_on_screen() {
        let first = Cell::new(10);
        assert_eq!(draw(30, 12, &first), 10);
        assert_eq!(first.get(), 10);
    }

    #[test]
    fn the_list_follows_a_selection_outside_it() {
        let first = Cell::new(10);
        assert_eq!(draw(30, 20, &first), 16);
        assert_eq!(draw(30, 3, &first), 3);
    }

    #[test]
    fn a_shorter_list_shows_its_rows_from_the_top() {
        let first = Cell::new(20);
        assert_eq!(draw(4, 2, &first), 0);
        assert_eq!(first.get(), 0);
    }
}
