//! Short notes drawn at the right end of a button bar or a title row.

use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::Frame;

use super::i18n::I18n;
use super::theme;

/// The note about a role's agent is drawn only when this many columns are
/// left for the role selector on its left: on a narrow screen the note is
/// left out rather than drawn over the selector.
const ROLE_SELECTOR_ROOM: u16 = 60;

/// «Unsaved changes» at the right end of `bar`, in the warning colour.
pub fn unsaved_note(frame: &mut Frame, bar: Rect, tr: &I18n) {
    let note = tr.t("roles.unsaved");
    let width = u16::try_from(note.chars().count()).unwrap_or(0);
    let x = bar.right().saturating_sub(width);
    frame.render_widget(
        Span::styled(note.to_string(), theme::warn()),
        Rect::new(x.max(bar.x), bar.y, width.min(bar.width), 1),
    );
}

/// `text` (the role and its agent) at the right end of `top`, dimmed; left
/// out when the row has no room for it next to the role selector.
pub fn agent_note(frame: &mut Frame, top: Rect, text: String) {
    let width = u16::try_from(text.chars().count()).unwrap_or(0);
    let x = top.right().saturating_sub(width);
    if x > top.x + ROLE_SELECTOR_ROOM {
        frame.render_widget(
            Span::styled(text, theme::dim()),
            Rect::new(x, top.y, width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;

    /// The first row of a screen `width` wide after `draw`.
    fn top_row(width: u16, draw: impl FnOnce(&mut Frame, Rect)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal.draw(|frame| draw(frame, frame.area())).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..width).map(|x| buffer[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_unsaved_note_ends_at_the_right_edge() {
        let home = tempfile::tempdir().unwrap();
        let tr = I18n::load(Some(home.path()));

        let row = top_row(40, |frame, bar| unsaved_note(frame, bar, &tr));

        assert!(row.ends_with("Changes not saved "), "{row:?}");
    }

    #[test]
    fn the_agent_note_leaves_room_for_the_role_selector() {
        let note = || "tester · claude ".to_string();

        let wide = top_row(100, |frame, top| agent_note(frame, top, note()));
        let narrow = top_row(70, |frame, top| agent_note(frame, top, note()));

        assert!(wide.ends_with("tester · claude "), "{wide:?}");
        assert_eq!(narrow.trim(), "");
    }
}
