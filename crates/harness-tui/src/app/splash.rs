//! The window shown when the harness starts (and on H, `?` or F1): the version,
//! whether a newer one is out with its update button, and the first steps
//! and keys. Any key or click closes it; Space or a click on its box decides
//! whether it opens at the next start (kept as `splash` in `tui.toml`).

use std::path::Path;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use harness_agents::install::update::VERSION;

use crate::ui::message::Message;
use crate::ui::{buttons, clear, i18n, keys, panel, theme, wrapped_lines, ButtonId, Target};
use crate::App;

/// The start window: open now, and whether it opens when the harness starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Splash {
    pub(crate) open: bool,
    pub(crate) at_start: bool,
}

impl Default for Splash {
    fn default() -> Self {
        Self {
            open: false,
            at_start: true,
        }
    }
}

/// The `tui.toml` setting that turns the window off at start: `splash = "off"`.
const SETTING: &str = "splash";

/// Does the window open at start? Yes, unless it was switched off in `home`.
pub(crate) fn shown_at_start(home: Option<&Path>) -> bool {
    home.and_then(|home| i18n::saved_setting(home, SETTING))
        .is_none_or(|value| value != "off")
}

/// The window's widest size: the steps read well in this many columns.
const SPLASH_WIDTH: u16 = 76;

impl App {
    /// A key while the window is open: Space ticks its box, `u` updates
    /// (when offered), any other key only closes it.
    pub(crate) fn splash_key(&mut self, code: KeyCode) {
        match keys::latin(code) {
            KeyCode::Char(' ') => self.toggle_splash_at_start(),
            KeyCode::Char('u') => {
                self.splash.open = false;
                self.press(ButtonId::HarnessUpdate);
            }
            _ => self.splash.open = false,
        }
    }

    /// A click while the window is open: its box and update button do their
    /// work, any other place only closes it.
    pub(crate) fn splash_click(&mut self, hit: Option<(Target, u16)>) {
        match hit {
            Some((Target::Button(ButtonId::SplashAtStart), _)) => self.toggle_splash_at_start(),
            Some((Target::Button(ButtonId::HarnessUpdate), _)) => {
                self.splash.open = false;
                self.press(ButtonId::HarnessUpdate);
            }
            _ => self.splash.open = false,
        }
    }

    /// Switches whether the window opens at start, and remembers it in `tui.toml`.
    pub(crate) fn toggle_splash_at_start(&mut self) {
        self.splash.at_start = !self.splash.at_start;
        let value = if self.splash.at_start { "on" } else { "off" };
        if let Some(home) = &self.home {
            if let Err(error) = i18n::save_setting(home, SETTING, value) {
                self.message = Some(Message::error(error));
            }
        }
    }

    /// Draws the window over the screen.
    pub(crate) fn draw_splash(&mut self, frame: &mut Frame) {
        let lines = self.splash_lines();
        let width = frame.area().width.saturating_sub(4).min(SPLASH_WIDTH);
        // Long lines wrap inside the border, so they take more than one row.
        let inner_width = usize::from(width.saturating_sub(2)).max(1);
        let rows: usize = lines
            .iter()
            .map(|line| wrapped_lines(&line.to_string(), inner_width))
            .sum();
        // The text, an empty row and the button row, inside the border.
        let height = u16::try_from(rows + 4).unwrap_or(u16::MAX);
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        clear(frame, area);
        let block = panel(&format!(" ◆ AI Harness {VERSION} "), true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let text_height = inner.height.saturating_sub(2);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            Rect::new(inner.x, inner.y, inner.width, text_height),
        );
        let update = self
            .agents
            .harness_update()
            .map(|version| self.tr.f("tabs.update_harness", &[("version", &version)]));
        let mut items = Vec::with_capacity(2);
        if let Some(label) = &update {
            items.push((label.as_str(), ButtonId::HarnessUpdate, true));
        }
        items.push((self.tr.t("splash.start"), ButtonId::CloseSplash, true));
        let row = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
        buttons(frame, row, &mut self.hits, &items);
        // The box, at the right of the buttons' row.
        let mark = if self.splash.at_start { "[x]" } else { "[ ]" };
        let label = format!("{mark} {}", self.tr.t("splash.at_start"));
        let width = u16::try_from(label.chars().count())
            .unwrap_or(0)
            .min(row.width);
        let rect = Rect::new(row.right().saturating_sub(width), row.y, width, 1);
        frame.render_widget(Span::styled(label, theme::dim()), rect);
        self.hits.add(rect, Target::Button(ButtonId::SplashAtStart));
    }

    /// The window's text: what the harness is, its version, first steps and keys.
    fn splash_lines(&self) -> Vec<Line<'static>> {
        let tr = &self.tr;
        let release = &self.agents.release;
        let (state, style) = if let Some(version) = &release.installed {
            let text = tr.f("agents.harness_installed", &[("version", version)]);
            (text, theme::ok())
        } else if release.from_source {
            (tr.t("agents.harness_source").to_string(), theme::dim())
        } else {
            match &release.newer {
                None => (tr.t("splash.checking").to_string(), theme::dim()),
                Some(Ok(None)) => (tr.t("agents.harness_newest").to_string(), theme::ok()),
                Some(Ok(Some(version))) => {
                    let text = tr.f("splash.newer", &[("version", version)]);
                    (text, theme::warn())
                }
                Some(Err(_)) => (tr.t("agents.harness_no_check").to_string(), theme::dim()),
            }
        };
        let mut lines = vec![
            Line::from(tr.t("splash.about").to_string()),
            Line::default(),
            Line::from(vec![
                Span::raw(tr.f("splash.version", &[("version", &VERSION)])),
                Span::styled(format!(" · {state}"), style),
            ]),
            Line::default(),
            Line::styled(tr.t("splash.steps_title").to_string(), theme::accent()),
        ];
        lines.extend(
            tr.t("splash.steps")
                .lines()
                .map(|l| Line::from(l.to_string())),
        );
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("splash.keys").to_string(), theme::dim()));
        lines
    }
}
