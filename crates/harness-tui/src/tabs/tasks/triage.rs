//! The buttons on the top border of the selected step's window: «Triage (t)»,
//! where the user's own agent finds out why the task went wrong (see
//! `harness_agents::triage`, the same as `harness triage <task>`), and
//! «↶ Restore code here (b)» (see `restore.rs`).

use ratatui::layout::Rect;
use ratatui::Frame;

use super::tab::TasksTab;
use crate::ui::i18n::I18n;
use crate::ui::{button_width, buttons, ButtonId, Hits};

impl TasksTab {
    /// The task «Triage» looks into: the selected one, while this window runs
    /// no roles (the agent would read files the roles are still writing).
    pub fn triage_target(&self) -> Option<String> {
        if self.is_running() {
            return None;
        }
        self.current().map(|task| task.id.clone())
    }

    /// Draws the buttons that apply now at the right of the top border of
    /// `area`, after its `title`, when there is room for all of them.
    pub(super) fn draw_step_buttons(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        hits: &mut Hits,
        tr: &I18n,
    ) {
        let mut items = Vec::new();
        if let Some(key) = self.restore_label() {
            items.push((tr.t(key), ButtonId::RestoreCode, true));
        }
        if self.triage_target().is_some() {
            items.push((tr.t("tasks.triage"), ButtonId::Triage, true));
        }
        // When both do not fit, Triage stays (`b` still marks a step).
        let mut place = row_place(area, title, row_width(&items));
        if place.is_none() && items.len() > 1 {
            items.remove(0);
            place = row_place(area, title, row_width(&items));
        }
        if let Some(place) = place {
            buttons(frame, place, hits, &items);
        }
    }
}

/// How wide a row of buttons is: each button and one column between them.
fn row_width(items: &[(&str, ButtonId, bool)]) -> u16 {
    items
        .iter()
        .map(|(label, _, _)| button_width(label) + 1)
        .sum::<u16>()
        .saturating_sub(1)
}

/// Where a row of buttons `width` wide goes: at the right end of the top
/// border, inside the corners, without covering `title` (one column of border
/// between them). `None` when it does not fit or there are no buttons.
fn row_place(area: Rect, title: &str, width: u16) -> Option<Rect> {
    let title_width = u16::try_from(title.chars().count()).unwrap_or(u16::MAX);
    // Two corners, the title, a column of border, the buttons.
    let needed = 2 + title_width + 1 + width;
    if width == 0 || area.height == 0 || area.width < needed {
        return None;
    }
    let x = area.right() - 1 - width;
    Some(Rect::new(x, area.y, width, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_sit_at_the_right_inside_the_corner() {
        let area = Rect::new(10, 3, 60, 10);

        let place = row_place(area, " ⤢ round 1 · architect ", 12).unwrap();

        assert_eq!(place, Rect::new(57, 3, 12, 1));
        assert_eq!(place.right(), area.right() - 1);
    }

    #[test]
    fn a_row_is_its_buttons_and_one_column_between_them() {
        let items = [
            ("ab", ButtonId::Triage, true),
            ("c", ButtonId::RestoreCode, true),
        ];

        assert_eq!(row_width(&items), 4 + 1 + 3);
        assert_eq!(row_width(&[]), 0);
    }

    #[test]
    fn no_room_next_to_the_title_means_no_buttons() {
        let title = " ⤢ round 1 · architect ";
        let fits = 2 + 23 + 1 + 12;

        assert!(row_place(Rect::new(0, 0, fits, 5), title, 12).is_some());
        assert!(row_place(Rect::new(0, 0, fits - 1, 5), title, 12).is_none());
        assert!(row_place(Rect::new(0, 0, 80, 0), title, 12).is_none());
        assert!(row_place(Rect::new(0, 0, 80, 5), title, 0).is_none());
    }
}
