//! The bordered, clickable list every tab draws its rows with.

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
pub fn draw_list(frame: &mut Frame, hits: &mut Hits, area: Rect, list: ListView<'_>) -> usize {
    let ListView {
        id,
        title,
        items,
        selected_row,
        focused,
    } = list;
    let block = panel(title, focused);
    let inner = block.inner(area);
    let mut state = ListState::default().with_selected(Some(selected_row));
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
    state.offset()
}
