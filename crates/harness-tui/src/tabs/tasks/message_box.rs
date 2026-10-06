//! Drawing the message box at the bottom of the Tasks tab and its open list.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{ListItem, Paragraph};
use ratatui::Frame;

use super::choice::{Choice, Menu};
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
            Choice::Finish => tr.t("tasks.choice_finish").to_string(),
        }
    }

    /// The message box: several lines of text, «To ▾» and «Send».
    pub(super) fn draw_input(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let block = panel(tr.t("tasks.input_title"), self.focus == Focus::Input);
        let inner = block.inner(area);
        frame.render_widget(block, area);
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
        items.push((send.to_string(), ButtonId::Send, self.running.is_none()));
        let width = |label: &str| crate::ui::button_width(label);
        let total = |items: &[(String, ButtonId, bool)]| {
            items
                .iter()
                .map(|(label, _, _)| width(label) + 1)
                .sum::<u16>()
                .saturating_sub(1)
        };
        // In a narrow window «Send» stays; the model and level go first.
        if total(&items) > inner.width && items.len() > 2 {
            items.drain(1..3);
        }
        let buttons_width = total(&items).min(inner.width);
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
            x += width(label) + 1;
        }
        self.draw_menu(frame, hits, menu, x, area.y, tr);
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
