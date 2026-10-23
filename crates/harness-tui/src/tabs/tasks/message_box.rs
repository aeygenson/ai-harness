//! Drawing the message box at the bottom of the Tasks tab and its open list.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use harness_core::task::handoff::MAX_REASON_CHARS;
use harness_core::text;

use super::choice::{Choice, Menu, MORE_ROUNDS};
use super::draw::draw_list;
use super::tab::{Focus, TasksTab};
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId, Target};

impl TasksTab {
    /// The label of an option of «To».
    fn choice_label(choice: Choice, tr: &I18n) -> String {
        match choice {
            Choice::NewTask => tr.t("tasks.choice_new").to_string(),
            Choice::Role(role) => role.as_str().to_string(),
            Choice::Continue(role) => tr.f("tasks.choice_continue", &[("role", &role)]),
            Choice::MoreRounds(role) => tr.f(
                "tasks.choice_more_rounds",
                &[("role", &role), ("rounds", &MORE_ROUNDS)],
            ),
            Choice::Finish => tr.t("tasks.choice_finish").to_string(),
            Choice::Publish => tr.t("tasks.choice_publish").to_string(),
            Choice::Merge => tr.t("tasks.choice_merge").to_string(),
        }
    }

    /// The message box: several lines of text, «To ▾» and «Send».
    pub(super) fn draw_input(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let title = tr.t("tasks.input_title");
        let block = panel(title, self.focus == Focus::Input);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.draw_questions(frame, area, title, hits, tr);
        if inner.height == 0 {
            return;
        }

        // The buttons on the bottom row, on the right.
        let to = format!(
            "{} {} ▾",
            tr.t("tasks.to"),
            Self::choice_label(self.choice, tr)
        );
        let send = if self.running.is_some() {
            tr.t("tasks.working")
        } else {
            tr.t("tasks.send")
        };
        let default = tr.t("tasks.default");
        let mut items: Vec<(String, ButtonId, bool)> = vec![(to, ButtonId::Menu(Menu::To), true)];
        if let Some((_, model, effort)) = self.run_choice() {
            let levels = !self.level_items().is_empty();
            items.push((
                format!("{} {} ▾", tr.t("tasks.model"), model.unwrap_or(default)),
                ButtonId::Menu(Menu::Model),
                true,
            ));
            items.push((
                format!("{} {} ▾", tr.t("tasks.level"), effort.unwrap_or(default)),
                ButtonId::Menu(Menu::Level),
                levels,
            ));
        }
        let stops = self.stop_buttons(tr);
        let stopping = !stops.is_empty();
        for (label, id) in stops {
            items.push((label, id, true));
        }
        items.push((send.to_string(), ButtonId::Send, self.running.is_none()));
        fit_buttons(&mut items, inner.width, stopping);
        let buttons_width = buttons_total(&items).min(inner.width);
        let bottom = inner.bottom() - 1;
        let row = Rect::new(inner.right() - buttons_width, bottom, buttons_width, 1);

        // The text above them; with a short window it shares the bottom row.
        let text_rows = if inner.height > 1 {
            inner.height - 1
        } else {
            1
        };
        let text_width = if inner.height > 1 {
            inner.width
        } else {
            inner.width.saturating_sub(buttons_width + 1)
        };
        let field = Rect::new(inner.x, inner.y, text_width, text_rows);
        let typing = self.focus == Focus::Input;
        let lines: Vec<Line> = if self.input.is_empty() && !typing {
            vec![Line::styled(
                format!(" {}", tr.t("tasks.input_hint")),
                theme::dim(),
            )]
        } else {
            let cursor = if typing { "▏" } else { "" };
            let room = usize::from(text_width.saturating_sub(2)).max(1);
            let wrapped = wrap(&format!("{}{cursor}", self.input), room);
            // The end of a long text stays visible: that is where one types.
            let skip = wrapped.len().saturating_sub(usize::from(text_rows));
            wrapped
                .into_iter()
                .skip(skip)
                .map(|l| Line::from(format!(" {l}")))
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), field);
        hits.add(field, Target::Button(ButtonId::Input));

        let shown: Vec<(&str, ButtonId, bool)> = items
            .iter()
            .map(|(label, id, enabled)| (label.as_str(), *id, *enabled))
            .collect();
        buttons(frame, row, hits, &shown);
        let Some(menu) = self.menu else {
            return;
        };
        // The list opens above its own button.
        let wanted = ButtonId::Menu(menu);
        let mut x = row.x;
        for (label, id, _) in &items {
            if *id == wanted {
                break;
            }
            x += crate::ui::button_width(label) + 1;
        }
        self.draw_menu(frame, hits, menu, x, area.y, tr);
    }

    /// The link to the notes Lisa answers, on the box's top border after its
    /// title, so it is right where she writes; then whom the asking role
    /// suggests for the answer. Nothing when there is neither.
    fn draw_questions(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        hits: &mut Hits,
        tr: &I18n,
    ) {
        if area.height == 0 {
            return;
        }
        // The title starts one column in, after the corner; one dash between.
        let title_width = u16::try_from(Line::from(title).width()).unwrap_or(u16::MAX);
        let mut x = area.x.saturating_add(title_width).saturating_add(2);
        if let Some((role, _)) = self.questions() {
            let label = format!(" {} ", tr.f("tasks.questions", &[("role", &role)]));
            let look = theme::accent().add_modifier(Modifier::UNDERLINED);
            let Some(link) = border_text(frame, area, x, label, look) else {
                return;
            };
            hits.add(link, Target::Button(ButtonId::TaskQuestions));
            x = link.right().saturating_add(1);
        }
        if let Some(answer) = self.suggested_answer() {
            // The reason is the agent's text: one clean line, nothing more.
            let reason = text::safe_line(&answer.reason, MAX_REASON_CHARS);
            let label = tr.f(
                "tasks.suggested",
                &[("role", &answer.role), ("reason", &reason)],
            );
            border_text(frame, area, x, format!(" {label} "), Style::new());
        }
    }

    /// The open list, above the box; what cannot be chosen now is grey.
    fn draw_menu(
        &self,
        frame: &mut Frame,
        hits: &mut Hits,
        menu: Menu,
        x: u16,
        top: u16,
        tr: &I18n,
    ) {
        let default = || tr.t("tasks.default").to_string();
        let (title, items, current): (&str, Vec<(String, bool)>, usize) = match menu {
            Menu::To => {
                let items = self.menu_items();
                let current = items
                    .iter()
                    .position(|(c, _)| *c == self.choice)
                    .unwrap_or(0);
                let labels = items
                    .iter()
                    .map(|(c, enabled)| (Self::choice_label(*c, tr), *enabled))
                    .collect();
                (tr.t("tasks.to"), labels, current)
            }
            Menu::Model | Menu::Level => {
                let (items, now) = if menu == Menu::Model {
                    (self.model_items(), self.run_choice().and_then(|c| c.1))
                } else {
                    (self.level_items(), self.run_choice().and_then(|c| c.2))
                };
                let current = items.iter().position(|m| m.as_deref() == now).unwrap_or(0);
                let labels = items
                    .into_iter()
                    .map(|m| (m.unwrap_or_else(default), true))
                    .collect();
                let title = if menu == Menu::Model {
                    tr.t("tasks.model")
                } else {
                    tr.t("tasks.level")
                };
                (title, labels, current)
            }
        };
        let width = items
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0)
            .max(title.chars().count())
            + 4;
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        let height = u16::try_from(items.len() + 2).unwrap_or(u16::MAX).min(top);
        let x = x.min(frame.area().right().saturating_sub(width));
        let area = Rect::new(x, top.saturating_sub(height), width, height);
        crate::ui::clear(frame, area);
        let list = items
            .into_iter()
            .map(|(label, enabled)| {
                let style = if enabled { Style::new() } else { theme::dim() };
                ListItem::new(Line::styled(label, style))
            })
            .collect();
        draw_list(
            frame,
            hits,
            area,
            ListId::Choices,
            title,
            list,
            current,
            true,
        );
    }
}

/// The width the buttons take in a row, with one column between them.
fn buttons_total(items: &[(String, ButtonId, bool)]) -> u16 {
    items
        .iter()
        .map(|(label, _, _)| crate::ui::button_width(label) + 1)
        .sum::<u16>()
        .saturating_sub(1)
}

/// Drops buttons until they fit in `room` columns. The model and level go
/// first. While roles work (`stopping`), the stop buttons matter most: then
/// «To» and the grey «Working…» go too (nothing can be sent then anyway),
/// and last «Stop after this step».
fn fit_buttons(items: &mut Vec<(String, ButtonId, bool)>, room: u16, stopping: bool) {
    if buttons_total(items) > room {
        items.retain(|(_, id, _)| !matches!(id, ButtonId::Menu(Menu::Model | Menu::Level)));
    }
    if stopping && buttons_total(items) > room {
        items.retain(|(_, id, _)| !matches!(id, ButtonId::Menu(Menu::To) | ButtonId::Send));
    }
    // Still too wide: «Stop now» stays, since nothing else stops the agent
    // at once; «Stop after this step» keeps its key (s).
    if stopping && buttons_total(items) > room {
        items.retain(|(_, id, _)| *id != ButtonId::StopAfterStep);
    }
}

/// Draws `label` on the top border of `area` from column `x`, cut at the
/// right corner; the place it took, or `None` when there is no room.
fn border_text(frame: &mut Frame, area: Rect, x: u16, label: String, look: Style) -> Option<Rect> {
    let room = area.right().saturating_sub(1).saturating_sub(x);
    let width = u16::try_from(Line::from(label.as_str()).width())
        .unwrap_or(u16::MAX)
        .min(room);
    if width == 0 {
        return None;
    }
    let place = Rect::new(x, area.y, width, 1);
    frame.render_widget(Paragraph::new(Line::styled(label, look)), place);
    Some(place)
}

/// Room for five lines of text and the buttons; less in a small window.
pub(super) fn input_height(area: Rect) -> u16 {
    match area.height {
        0..=15 => 3,
        16..=24 => 5,
        _ => 8,
    }
}

/// `text` cut into lines of at most `width` characters; line breaks stay.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        for chunk in chars.chunks(width.max(1)) {
            lines.push(chunk.iter().collect());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(stopping: bool) -> Vec<(String, ButtonId, bool)> {
        let mut items = vec![
            (
                "To: developer ▾".to_string(),
                ButtonId::Menu(Menu::To),
                true,
            ),
            (
                "Model: default ▾".to_string(),
                ButtonId::Menu(Menu::Model),
                true,
            ),
            (
                "Level: default ▾".to_string(),
                ButtonId::Menu(Menu::Level),
                true,
            ),
        ];
        if stopping {
            let after = "✓ Stops after this step (s: cancel)".to_string();
            items.push((after, ButtonId::StopAfterStep, true));
            items.push(("Stop now (x)".to_string(), ButtonId::StopNow, true));
        }
        items.push(("Send".to_string(), ButtonId::Send, !stopping));
        items
    }

    fn ids(items: &[(String, ButtonId, bool)]) -> Vec<ButtonId> {
        items.iter().map(|(_, id, _)| *id).collect()
    }

    #[test]
    fn a_narrow_row_keeps_to_and_send_when_nothing_runs() {
        let mut items = row(false);
        fit_buttons(&mut items, 10, false);
        assert_eq!(ids(&items), [ButtonId::Menu(Menu::To), ButtonId::Send]);
    }

    #[test]
    fn a_narrow_row_keeps_the_stop_buttons_while_roles_work() {
        let stops = [ButtonId::StopAfterStep, ButtonId::StopNow];
        let both = buttons_total(&row(true)[3..5]);
        let mut items = row(true);
        fit_buttons(&mut items, both, true);
        assert_eq!(ids(&items), stops);
        assert!(buttons_total(&items) <= both);
        // Narrower still: «Stop now» is the one that stays.
        let mut items = row(true);
        fit_buttons(&mut items, both - 1, true);
        assert_eq!(ids(&items), [ButtonId::StopNow]);
        // With room enough only the model and level go.
        let mut items = row(true);
        fit_buttons(&mut items, buttons_total(&row(true)) - 1, true);
        assert_eq!(
            ids(&items),
            [
                ButtonId::Menu(Menu::To),
                ButtonId::StopAfterStep,
                ButtonId::StopNow,
                ButtonId::Send
            ]
        );
    }
}
