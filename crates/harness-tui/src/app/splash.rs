//! The window shown when the harness starts (and on `?` or F1): the version,
//! whether a newer one is out with its update button, and the first steps
//! and keys. Any key or click closes it.

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use harness_agents::install::update::VERSION;

use crate::ui::{buttons, clear, keys, panel, theme, wrapped_lines, ButtonId, Target};
use crate::App;

/// Whether the start window is over the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Splash {
    Open,
    Closed,
}

/// The window's widest size: the steps read well in this many columns.
const SPLASH_WIDTH: u16 = 76;

impl App {
    /// A key while the window is open: `u` updates (when offered), any other key only closes it.
    pub(crate) fn splash_key(&mut self, code: KeyCode) {
        self.splash = Splash::Closed;
        if keys::latin(code) == KeyCode::Char('u') {
            self.press(ButtonId::HarnessUpdate);
        }
    }

    /// A click while the window is open: its update button updates, any other place only closes it.
    pub(crate) fn splash_click(&mut self, hit: Option<(Target, u16)>) {
        self.splash = Splash::Closed;
        if let Some((Target::Button(ButtonId::HarnessUpdate), _)) = hit {
            self.press(ButtonId::HarnessUpdate);
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
