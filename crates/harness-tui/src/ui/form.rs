//! A small window over the screen with text fields, boxes to tick and OK / Cancel.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{buttons, clear, input, panel, theme, ButtonId, Hits, Target};

/// A small window over the screen with text fields and OK / Cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub title: String,
    /// Shown above the fields.
    pub text: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub ok: String,
    /// A problem from the last OK, shown in red.
    pub error: Option<String>,
}

/// What a form field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Text that is typed.
    Text,
    /// A secret: shown as dots, never printed.
    Secret,
    /// A box that is ticked or not; Space or a click switches it.
    Check(bool),
}

#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    /// What was typed; always empty for a [`FieldKind::Check`].
    pub value: String,
    pub kind: FieldKind,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self.kind {
            FieldKind::Secret => "***",
            FieldKind::Text | FieldKind::Check(_) => &self.value,
        };
        f.debug_struct("Field")
            .field("label", &self.label)
            .field("value", &value)
            .field("kind", &self.kind)
            .finish()
    }
}

impl Form {
    pub fn new(title: &str, text: &str, ok: &str) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            fields: Vec::new(),
            focus: 0,
            ok: ok.into(),
            error: None,
        }
    }

    pub fn field(mut self, label: &str, value: &str) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: value.into(),
            kind: FieldKind::Text,
        });
        self
    }

    /// A field for a secret: what is typed or pasted shows as dots.
    pub fn secret(mut self, label: &str) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: String::new(),
            kind: FieldKind::Secret,
        });
        self
    }

    /// A box to tick, ticked from the start when `on`.
    pub fn check(mut self, label: &str, on: bool) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: String::new(),
            kind: FieldKind::Check(on),
        });
        self
    }

    pub fn value(&self, index: usize) -> &str {
        self.fields.get(index).map_or("", |f| f.value.trim())
    }

    /// Is the box `index` ticked? `false` for a field that is not a box.
    pub fn is_checked(&self, index: usize) -> bool {
        self.fields
            .get(index)
            .is_some_and(|f| f.kind == FieldKind::Check(true))
    }

    /// Ticks the box `index`, or takes the tick away; other fields stay as they are.
    pub fn switch(&mut self, index: usize) {
        if let Some(Field {
            kind: FieldKind::Check(on),
            ..
        }) = self.fields.get_mut(index)
        {
            *on = !*on;
        }
    }

    /// Typing goes into the focused field. A box takes only Space, which
    /// switches it; pasted text never changes a box.
    pub fn type_char(&mut self, c: char) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            match field.kind {
                FieldKind::Text | FieldKind::Secret => field.value.push(c),
                FieldKind::Check(_) => {}
            }
        }
    }

    /// A key typed on the keyboard: like [`Form::type_char`], but Space
    /// switches a focused box.
    pub fn key_char(&mut self, c: char) {
        let on_box = self
            .fields
            .get(self.focus)
            .is_some_and(|f| matches!(f.kind, FieldKind::Check(_)));
        if on_box && c == ' ' {
            self.switch(self.focus);
        } else {
            self.type_char(c);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            field.value.pop();
        }
    }

    pub fn next_field(&mut self) {
        if !self.fields.is_empty() {
            self.focus = (self.focus + 1) % self.fields.len();
        }
    }

    /// `cancel` is the label of the Cancel button in the current language.
    pub fn draw(&self, frame: &mut Frame, hits: &mut Hits, cancel: &str) {
        let width = frame.area().width.saturating_sub(4).min(72);
        // Long lines wrap inside the window's borders.
        let inner_width = usize::from(width.saturating_sub(2)).max(1);
        let text_lines = self
            .text
            .lines()
            .map(|line| wrapped_lines(line, inner_width))
            .sum::<usize>();
        let text_lines = u16::try_from(text_lines).unwrap_or(0);
        let fields = u16::try_from(self.fields.len()).unwrap_or(0);
        let height = 2 + text_lines + u16::from(text_lines > 0) + fields * 2 + 2 + 2;
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        clear(frame, area);
        let block = panel(&format!(" {} ", self.title), true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        // Clicks anywhere in the window stay in it.
        hits.add(area, Target::Window);

        let mut y = inner.y;
        if text_lines > 0 {
            let rect = Rect::new(inner.x, y, inner.width, text_lines);
            frame.render_widget(
                Paragraph::new(self.text.clone()).wrap(Wrap { trim: false }),
                rect,
            );
            y += text_lines + 1;
        }
        for (i, field) in self.fields.iter().enumerate() {
            frame.render_widget(
                Line::from(field.label.clone()),
                Rect::new(inner.x, y, inner.width, 1),
            );
            let rect = Rect::new(inner.x, y + 1, inner.width, 1);
            let focused = i == self.focus;
            let cursor = if focused { "▏" } else { "" };
            let style = if focused { input() } else { theme::chip() };
            let shown = match field.kind {
                FieldKind::Text => field.value.clone(),
                FieldKind::Secret => "•".repeat(field.value.chars().count()),
                FieldKind::Check(true) => "[x]".to_string(),
                FieldKind::Check(false) => "[ ]".to_string(),
            };
            frame.render_widget(Span::styled(format!(" {shown}{cursor}"), style), rect);
            hits.add(rect, Target::Field(i));
            y += 2;
        }
        if let Some(error) = &self.error {
            frame.render_widget(
                Span::styled(error.clone(), theme::bad()),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
        let row = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        buttons(
            frame,
            row,
            hits,
            &[
                (&self.ok, ButtonId::Ok, true),
                (cancel, ButtonId::Cancel, true),
            ],
        );
    }
}

/// How many rows `line` takes when wrapped at words to `width` columns.
fn wrapped_lines(line: &str, width: usize) -> usize {
    let mut rows = 1;
    let mut used = 0;
    for word in line.split(' ') {
        let len = word.chars().count();
        let needed = if used == 0 { len } else { used + 1 + len };
        if needed <= width {
            used = needed;
        } else {
            if used > 0 {
                rows += 1;
            }
            used = len;
            // A word longer than the row is broken.
            while used > width {
                rows += 1;
                used -= width;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_takes_typing_into_the_focused_field() {
        let mut form = Form::new("New", "", "OK").field("a", "x").field("b", "");
        form.type_char('y');
        form.next_field();
        form.type_char('z');
        form.backspace();
        form.type_char('w');
        assert_eq!((form.value(0), form.value(1)), ("xy", "w"));
        form.next_field();
        assert_eq!(form.focus, 0);
    }

    #[test]
    fn typed_text_has_its_own_colours() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut form = Form::new("New", "", "OK").field("a", "").field("b", "old");
        form.type_char('q');
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        for code in ["night", "day", "terminal"] {
            assert!(theme::select(code));
            terminal
                .draw(|frame| form.draw(frame, &mut Hits::default(), "Cancel"))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let cell = |text: &str| {
                let (x, y) = (0..20u16)
                    .flat_map(|y| (0..60u16).map(move |x| (x, y)))
                    .find(|&(x, y)| buffer[(x, y)].symbol() == text)
                    .unwrap();
                buffer[(x, y)].clone()
            };
            let theme = theme::current();
            let typed = cell("q");
            assert_eq!(
                (typed.fg, typed.bg),
                (theme.on_input, theme.input),
                "{code}"
            );
            // Other fields are drawn as fields too, with their own colours.
            let other = cell("o");
            assert_eq!((other.fg, other.bg), (theme.on_chip, theme.chip), "{code}");
        }
    }

    #[test]
    fn long_lines_wrap_at_words() {
        assert_eq!(wrapped_lines("", 10), 1);
        assert_eq!(wrapped_lines("one two three", 9), 2);
        assert_eq!(wrapped_lines("abcdefghijkl", 5), 3);
    }

    #[test]
    fn a_box_switches_with_space_and_ignores_other_keys() {
        let mut form = Form::new("t", "", "OK")
            .field("name", "")
            .check("sign in", false);
        form.key_char(' ');
        assert_eq!(form.value(0), "");
        form.next_field();
        form.key_char('y');
        assert!(!form.is_checked(1));
        form.key_char(' ');
        assert!(form.is_checked(1));
        // Pasted text never changes a box.
        form.type_char(' ');
        assert!(form.is_checked(1));
        form.switch(1);
        assert!(!form.is_checked(1));
        // A text field is never a ticked box, and switching it does nothing.
        form.switch(0);
        assert!(!form.is_checked(0));
    }
}
