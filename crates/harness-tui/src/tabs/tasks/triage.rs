//! The «Triage (t)» button on the top border of the task's steps: the user's
//! own agent finds out why the task went wrong (see `harness_agents::triage`),
//! the same as `harness triage <task>`.

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

    /// Draws the button at the right of the top border of `area` (the steps
    /// window), when there is a task to look into and room next to `title`.
    pub(super) fn draw_triage(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        hits: &mut Hits,
        tr: &I18n,
    ) {
        if self.triage_target().is_none() {
            return;
        }
        let label = tr.t("tasks.triage");
        if let Some(place) = button_place(area, title, label) {
            buttons(frame, place, hits, &[(label, ButtonId::Triage, true)]);
        }
    }
}

/// Where the button goes: at the right end of the top border, inside the
/// corners, without covering `title` (one column of border between them).
fn button_place(area: Rect, title: &str, label: &str) -> Option<Rect> {
    let width = button_width(label);
    let title_width = u16::try_from(title.chars().count()).unwrap_or(u16::MAX);
    // Two corners, the title, a column of border, the button.
    let needed = 2 + title_width + 1 + width;
    if area.height == 0 || area.width < needed {
        return None;
    }
    let x = area.right() - 1 - width;
    Some(Rect::new(x, area.y, width, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_sits_at_the_right_inside_the_corner() {
        let area = Rect::new(10, 3, 60, 10);

        let place = button_place(area, " ⤢ task-001 ", "Triage (t)").unwrap();

        assert_eq!(place, Rect::new(57, 3, 12, 1));
        assert_eq!(place.right(), area.right() - 1);
    }

    #[test]
    fn no_room_next_to_the_title_means_no_button() {
        let title = " ⤢ task-001 · round 2 ";
        let fits = 2 + 22 + 1 + button_width("Triage (t)");

        assert!(button_place(Rect::new(0, 0, fits, 5), title, "Triage (t)").is_some());
        assert!(button_place(Rect::new(0, 0, fits - 1, 5), title, "Triage (t)").is_none());
        assert!(button_place(Rect::new(0, 0, 80, 0), title, "Triage (t)").is_none());
    }
}
