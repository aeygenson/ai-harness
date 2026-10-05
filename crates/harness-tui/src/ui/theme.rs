//! The colours of the TUI: five themes, switched with the ◐ button or T.
//!
//! «Night», «Day», «Arctic» and «Contrast» paint their own background, so
//! they look the same in any terminal; «Terminal» uses the terminal's own
//! palette and background. The chosen one is remembered in
//! `~/.harness/tui.toml` like the language.
//!
//! The theme is kept per thread: the TUI draws from one thread, and tests
//! running side by side do not disturb each other.

use std::cell::Cell;

use harness_core::handoff::Role;
use ratatui::style::{Color, Modifier, Style};

/// One theme. `bg: None` keeps the terminal's background and text colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Saved in tui.toml.
    pub code: &'static str,
    /// The key of its name in the translations.
    pub name: &'static str,
    pub bg: Option<Color>,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    /// Text on the accent colour.
    pub on_accent: Color,
    /// The selected row; `None` reverses its colours instead.
    pub selected: Option<Color>,
    pub ok: Color,
    pub bad: Color,
    pub warn: Color,
    pub running: Color,
    /// Buttons and keys.
    pub chip: Color,
    pub on_chip: Color,
    /// The field being typed in: always its own colours, readable anywhere.
    pub input: Color,
    pub on_input: Color,
    /// architect, developer, tester, security, human, retro.
    pub roles: [Color; 6],
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub const THEMES: [Theme; 5] = [
    Theme {
        code: "night",
        name: "theme.night",
        bg: Some(rgb(27, 29, 38)),
        fg: rgb(215, 218, 230),
        dim: rgb(124, 130, 154),
        border: rgb(58, 63, 85),
        accent: rgb(138, 164, 255),
        on_accent: rgb(17, 19, 26),
        selected: Some(rgb(44, 51, 80)),
        ok: rgb(127, 214, 160),
        bad: rgb(242, 139, 139),
        warn: rgb(242, 198, 109),
        running: rgb(121, 184, 255),
        chip: rgb(42, 46, 61),
        on_chip: rgb(215, 218, 230),
        input: rgb(42, 58, 110),
        on_input: rgb(255, 255, 255),
        roles: [
            rgb(196, 155, 255),
            rgb(121, 184, 255),
            rgb(242, 198, 109),
            rgb(242, 139, 139),
            rgb(127, 214, 160),
            rgb(111, 211, 211),
        ],
    },
    Theme {
        code: "day",
        name: "theme.day",
        bg: Some(rgb(247, 247, 251)),
        fg: rgb(35, 37, 47),
        dim: rgb(107, 112, 133),
        border: rgb(195, 199, 214),
        accent: rgb(61, 90, 254),
        on_accent: rgb(255, 255, 255),
        selected: Some(rgb(222, 228, 255)),
        ok: rgb(29, 122, 68),
        bad: rgb(198, 40, 40),
        warn: rgb(154, 98, 0),
        running: rgb(31, 95, 191),
        chip: rgb(228, 230, 239),
        on_chip: rgb(35, 37, 47),
        input: rgb(31, 63, 191),
        on_input: rgb(255, 255, 255),
        roles: [
            rgb(123, 63, 228),
            rgb(31, 95, 191),
            rgb(154, 98, 0),
            rgb(198, 40, 40),
            rgb(29, 122, 68),
            rgb(0, 128, 140),
        ],
    },
    Theme {
        code: "arctic",
        name: "theme.arctic",
        bg: Some(rgb(46, 52, 64)),
        fg: rgb(216, 222, 233),
        dim: rgb(144, 153, 171),
        border: rgb(76, 86, 106),
        accent: rgb(136, 192, 208),
        on_accent: rgb(46, 52, 64),
        selected: Some(rgb(67, 76, 94)),
        ok: rgb(163, 190, 140),
        bad: rgb(208, 112, 120),
        warn: rgb(235, 203, 139),
        running: rgb(129, 161, 193),
        chip: rgb(59, 66, 82),
        on_chip: rgb(216, 222, 233),
        input: rgb(94, 129, 172),
        on_input: rgb(255, 255, 255),
        roles: [
            rgb(180, 142, 173),
            rgb(129, 161, 193),
            rgb(235, 203, 139),
            rgb(208, 112, 120),
            rgb(163, 190, 140),
            rgb(143, 188, 187),
        ],
    },
    Theme {
        code: "contrast",
        name: "theme.contrast",
        bg: Some(rgb(0, 0, 0)),
        fg: rgb(255, 255, 255),
        dim: rgb(175, 175, 175),
        border: rgb(200, 200, 200),
        accent: rgb(255, 215, 0),
        on_accent: rgb(0, 0, 0),
        selected: Some(rgb(0, 62, 145)),
        ok: rgb(0, 255, 120),
        bad: rgb(255, 95, 95),
        warn: rgb(255, 215, 0),
        running: rgb(95, 205, 255),
        chip: rgb(64, 64, 64),
        on_chip: rgb(255, 255, 255),
        input: rgb(0, 0, 205),
        on_input: rgb(255, 255, 255),
        roles: [
            rgb(225, 145, 255),
            rgb(95, 205, 255),
            rgb(255, 215, 0),
            rgb(255, 95, 95),
            rgb(0, 255, 120),
            rgb(0, 235, 235),
        ],
    },
    Theme {
        code: "terminal",
        name: "theme.terminal",
        bg: None,
        fg: Color::Reset,
        dim: Color::DarkGray,
        border: Color::Reset,
        accent: Color::Cyan,
        on_accent: Color::Black,
        selected: None,
        ok: Color::Green,
        bad: Color::LightRed,
        warn: Color::Yellow,
        running: Color::Blue,
        chip: Color::DarkGray,
        on_chip: Color::White,
        input: Color::Blue,
        on_input: Color::White,
        roles: [
            Color::Magenta,
            Color::Blue,
            Color::Yellow,
            Color::Red,
            Color::Green,
            Color::Cyan,
        ],
    },
];

thread_local! {
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

pub fn current() -> &'static Theme {
    &THEMES[CURRENT.with(Cell::get).min(THEMES.len() - 1)]
}

/// The theme before Lisa picks one: «Night», except in macOS's own
/// Terminal, which cannot show 24-bit colours (the RGB themes would come out
/// wrong there), so it starts with the terminal's own palette.
pub fn default_code() -> &'static str {
    default_for(std::env::var("TERM_PROGRAM").ok().as_deref())
}

fn default_for(term_program: Option<&str>) -> &'static str {
    match term_program {
        Some("Apple_Terminal") => "terminal",
        _ => THEMES[0].code,
    }
}

/// Switches to the theme `code`; an unknown code changes nothing.
pub fn select(code: &str) -> bool {
    match THEMES.iter().position(|t| t.code == code) {
        Some(index) => {
            CURRENT.with(|c| c.set(index));
            true
        }
        None => false,
    }
}

/// The next theme, round the list.
pub fn next() {
    CURRENT.with(|c| c.set((c.get() + 1) % THEMES.len()));
}

fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

/// The whole screen: the theme's background and text.
pub fn base() -> Style {
    let theme = current();
    let style = Style::new().fg(theme.fg);
    match theme.bg {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

pub fn dim() -> Style {
    fg(current().dim)
}

pub fn accent() -> Style {
    fg(current().accent).add_modifier(Modifier::BOLD)
}

pub fn ok() -> Style {
    fg(current().ok)
}

pub fn bad() -> Style {
    fg(current().bad)
}

pub fn warn() -> Style {
    fg(current().warn)
}

pub fn running() -> Style {
    fg(current().running)
}

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Each role has its own colour everywhere.
pub fn role(role: Role) -> Style {
    let roles = current().roles;
    fg(match role {
        Role::Architect => roles[0],
        Role::Developer => roles[1],
        Role::Tester => roles[2],
        Role::Security => roles[3],
        Role::Human => roles[4],
    })
}

/// The retrospective's colour.
pub fn retro() -> Style {
    fg(current().roles[5])
}

/// The selected row of a list.
pub fn selected() -> Style {
    match current().selected {
        Some(bg) => Style::new().bg(bg).add_modifier(Modifier::BOLD),
        None => Style::new().add_modifier(Modifier::REVERSED),
    }
}

/// The field being typed in.
pub fn input() -> Style {
    let theme = current();
    Style::new().fg(theme.on_input).bg(theme.input)
}

/// A button.
pub fn chip() -> Style {
    let theme = current();
    Style::new().fg(theme.on_chip).bg(theme.chip)
}

/// The main button of a tab, the chosen item of a selector.
pub fn primary() -> Style {
    let theme = current();
    Style::new()
        .fg(theme.on_accent)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD)
}

/// A panel's border: the accent colour when it has the focus.
pub fn border(focused: bool) -> Style {
    let theme = current();
    fg(if focused { theme.accent } else { theme.border })
}

/// A panel's title.
pub fn title(focused: bool) -> Style {
    if focused {
        accent()
    } else {
        bold()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_terminal_starts_with_its_own_colours() {
        assert_eq!(default_for(Some("Apple_Terminal")), "terminal");
        assert_eq!(default_for(Some("iTerm.app")), "night");
        assert_eq!(default_for(None), "night");
        assert!(THEMES.iter().any(|t| t.code == "terminal"));
    }

    #[test]
    fn themes_switch_round_the_list() {
        assert!(select("night"));
        assert_eq!(current().code, "night");
        assert!(!select("neon"));
        assert_eq!(current().code, "night");
        let codes: Vec<&str> = (0..THEMES.len())
            .map(|_| {
                next();
                current().code
            })
            .collect();
        assert_eq!(codes, ["day", "arctic", "contrast", "terminal", "night"]);
    }

    #[test]
    fn typed_text_always_has_its_own_colours() {
        for theme in &THEMES {
            assert_ne!(theme.input, theme.on_input, "{}", theme.code);
            assert!(theme.on_input != Color::Reset, "{}", theme.code);
            // A theme with its own background sets the text colour too.
            if theme.bg.is_some() {
                assert!(theme.fg != Color::Reset, "{}", theme.code);
            }
        }
    }
}
