//! The bordered, clickable list every tab draws its rows with.

use ratatui::layout::Rect;
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use super::{panel, selected, Hits, ListId, Target};

/// The selection mark every list puts before its rows (see [`draw_list`]).
const LIST_MARK: &str = "▶ ";

/// Its width on the screen: every row of a list starts this far to the right.
pub const LIST_MARK_WIDTH: usize = 2;

/// A bordered list whose rows can be clicked.
#[expect(
    clippy::too_many_arguments,
    reason = "every list on every tab is drawn by this one function with these inputs"
)]
pub fn draw_list(
    frame: &mut Frame,
    hits: &mut Hits,
    area: Rect,
    id: ListId,
    title: &str,
    items: Vec<ListItem>,
    selected_row: usize,
    focused: bool,
) -> usize {
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
