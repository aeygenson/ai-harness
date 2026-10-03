//! Pieces every tab uses: clickable areas, panels, buttons and a text form.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme;

/// Something on the screen that reacts to a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tab(usize),
    /// A row of a list: `first` is the index of the top visible row.
    List {
        list: ListId,
        first: usize,
    },
    Button(ButtonId),
    /// A field of the open form.
    Field(usize),
    /// A line of a tab's details that can be chosen, by its index.
    Row(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListId {
    Tasks,
    Steps,
    Projects,
    Roles,
    Folders,
    /// The «To» list of the Tasks tab.
    Choices,
    /// «Roles» on the Tasks tab: a click filters the tasks.
    RoleFilter,
    /// The skills of the Skills tab.
    Skills,
    /// The servers of the MCP tab.
    Mcp,
    /// The servers found in the MCP registry.
    McpCatalog,
    /// The plugins of the Plugins tab.
    Plugins,
    /// The plugins of all catalogs, in «From catalog».
    PluginCatalog,
    /// The added catalogs, in «Catalogs».
    PluginCatalogs,
    /// The saved retrospectives of the Retro tab.
    Retros,
    /// The proposals of the selected retrospective.
    RetroProposals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonId {
    NewProject,
    OpenFolder,
    RemoveProject,
    UseProject,
    Ok,
    Cancel,
    Language,
    /// The next colour theme.
    Theme,
    Save,
    Undo,
    /// «Refresh models» on the Roles tab.
    RefreshModels,
    Choose,
    Up,
    NewFolder,
    ToggleHidden,
    /// The message field of the Tasks tab.
    Input,
    /// «To ▾» on the Tasks tab.
    To,
    /// «Model ▾» and «Level ▾» on the Tasks tab: for the next launch only.
    Model,
    Level,
    Send,
    /// A role of the Skills tab's selector, an index into its `ROLES`.
    SkillRole(usize),
    SkillEdit,
    SkillNew,
    SkillRestore,
    /// The `[ ]` mark of the skill on this row of the Skills tab's list.
    SkillMark(usize),
    /// A link in «Files» of the step on the Tasks tab.
    TaskFile(usize),
    /// The title of a window of the Tasks tab: over the whole tab, or back.
    TaskZoom(crate::tasks::Zoom),
    /// A role of the MCP tab's selector, an index into `skills_tab::ROLES`.
    McpRole(usize),
    /// Gives the selected MCP server to the role, or takes it away.
    McpToggle,
    McpNew,
    McpEdit,
    McpRemove,
    /// Saves a secret the selected MCP server needs.
    McpSecret,
    /// Starts the selected MCP server and asks it for its tools.
    McpCheck,
    /// Signs in to the selected web MCP server in the browser.
    McpSignIn,
    /// Opens the catalog of the MCP registry.
    McpCatalog,
    /// In the catalog: a new search, add the chosen server, back to the list.
    McpSearch,
    McpUse,
    McpBack,
    /// A role of the Plugins tab's selector, an index into its `ROLES`.
    PluginRole(usize),
    /// Give the selected plugin to the role, or take it away.
    PluginToggle,
    /// Allow or forbid the plugin's hooks, its own servers.
    PluginHooks,
    PluginServers,
    PluginRemove,
    /// Open the plugin's folder in the editor.
    PluginOpen,
    /// Download the plugin's newest version from its catalog.
    PluginUpdate,
    /// Open «From catalog».
    PluginCatalog,
    /// In «From catalog»: the agent filter, an index into its `FILTERS`.
    PluginFilter(usize),
    PluginSearch,
    PluginAdd,
    /// Add and give to the role chosen on the tab.
    PluginAddGive,
    /// Open «Catalogs».
    PluginCatalogs,
    PluginBack,
    CatalogAdd,
    CatalogUpdate,
    CatalogRemove,
    /// The Retro tab: a new retrospective of the whole project.
    RetroGenerate,
    /// Open the retrospective's text in the editor.
    RetroOpen,
    /// Choose the selected proposal, or take it out.
    RetroToggle,
    /// Apply the chosen proposals.
    RetroApply,
}

/// Where the clickable things were drawn in the last frame. Drawing fills it,
/// a mouse click looks it up.
#[derive(Debug, Default)]
pub struct Hits {
    areas: Vec<(Rect, Target)>,
}

impl Hits {
    pub fn clear(&mut self) {
        self.areas.clear();
    }

    pub fn add(&mut self, area: Rect, target: Target) {
        self.areas.push((area, target));
    }

    /// What is at column `x`, row `y`, with the row inside it (0 = top row).
    /// Things drawn later (a form over a tab) win.
    pub fn at(&self, x: u16, y: u16) -> Option<(Target, u16)> {
        self.areas
            .iter()
            .rev()
            .find(|(area, _)| area.contains((x, y).into()))
            .map(|(area, target)| (*target, y - area.y))
    }

    /// The row index a click at `row` in a list means.
    pub fn row(target: Target, row: u16) -> Option<(ListId, usize)> {
        match target {
            Target::List { list, first } => Some((list, first + usize::from(row))),
            _ => None,
        }
    }
}

/// A panel with rounded corners; the one with the focus is in the accent colour.
pub fn panel(title: &str, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(title.to_string(), theme::title(focused)))
        .border_style(theme::border(focused))
}

pub fn selected() -> Style {
    theme::selected()
}

/// Clears `area` for a window drawn over the screen, in the theme's colours.
pub fn clear(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(theme::base()), area);
}

/// The main button of a tab: filled with the accent colour.
fn is_primary(id: ButtonId) -> bool {
    matches!(
        id,
        ButtonId::Ok
            | ButtonId::Send
            | ButtonId::Save
            | ButtonId::UseProject
            | ButtonId::Choose
            | ButtonId::RetroGenerate
            | ButtonId::RetroApply
            | ButtonId::PluginCatalog
            | ButtonId::PluginAdd
            | ButtonId::McpCatalog
            | ButtonId::McpUse
            | ButtonId::SkillEdit
    )
}

/// How wide the button `label` is drawn.
pub fn button_width(label: &str) -> u16 {
    u16::try_from(label.chars().count() + 2).unwrap_or(u16::MAX)
}

/// Draws buttons ` label ` in a row starting at `area`, and makes them clickable.
pub fn buttons(frame: &mut Frame, area: Rect, hits: &mut Hits, items: &[(&str, ButtonId, bool)]) {
    let mut x = area.x;
    for (label, id, enabled) in items {
        let text = format!(" {label} ");
        let width = button_width(label);
        if x + width > area.right() {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        let style = match (*enabled, is_primary(*id)) {
            (false, _) => theme::dim(),
            (true, true) => theme::primary(),
            (true, false) => theme::chip(),
        };
        frame.render_widget(Span::styled(text, style), rect);
        if *enabled {
            hits.add(rect, Target::Button(*id));
        }
        x += width + 1;
    }
}

/// `Label: [ a ] [ b ] …` on one row; the chosen one is filled. Each one is
/// a button `id(index)`.
pub fn selector(
    frame: &mut Frame,
    area: Rect,
    hits: &mut Hits,
    label: &str,
    names: &[&str],
    chosen: usize,
    id: fn(usize) -> ButtonId,
) {
    let label = format!(" {label} ");
    let width = u16::try_from(label.chars().count()).unwrap_or(0);
    frame.render_widget(Span::raw(label), area);
    let mut x = area.x + width;
    for (index, name) in names.iter().enumerate() {
        let text = format!(" {name} ");
        let w = button_width(name);
        if x + w > area.right() {
            break;
        }
        let rect = Rect::new(x, area.y, w, 1);
        let style = if index == chosen {
            theme::primary()
        } else {
            theme::chip()
        };
        frame.render_widget(Span::styled(text, style), rect);
        hits.add(rect, Target::Button(id(index)));
        x += w + 1;
    }
}

/// The field being typed in. Its text colour is set too: with only a
/// background, a light terminal theme draws dark text on dark gray.
pub fn input() -> Style {
    theme::input()
}

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

#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub value: String,
    /// A secret: shown as dots, never printed.
    pub hidden: bool,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = if self.hidden { "***" } else { &self.value };
        f.debug_struct("Field")
            .field("label", &self.label)
            .field("value", &value)
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
            hidden: false,
        });
        self
    }

    /// A field for a secret: what is typed or pasted shows as dots.
    pub fn secret(mut self, label: &str) -> Self {
        self.fields.push(Field {
            label: label.into(),
            value: String::new(),
            hidden: true,
        });
        self
    }

    pub fn value(&self, index: usize) -> &str {
        self.fields.get(index).map_or("", |f| f.value.trim())
    }

    /// Typing goes into the focused field.
    pub fn type_char(&mut self, c: char) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            field.value.push(c);
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
        hits.add(area, Target::Field(usize::MAX));

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
            let shown = if field.hidden {
                "•".repeat(field.value.chars().count())
            } else {
                field.value.clone()
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
    fn hits_find_the_top_thing_and_the_row() {
        let mut hits = Hits::default();
        hits.add(
            Rect::new(0, 0, 10, 5),
            Target::List {
                list: ListId::Tasks,
                first: 3,
            },
        );
        hits.add(Rect::new(2, 2, 3, 1), Target::Button(ButtonId::Ok));
        assert_eq!(hits.at(2, 2), Some((Target::Button(ButtonId::Ok), 0)));
        let (target, row) = hits.at(0, 4).unwrap();
        assert_eq!(Hits::row(target, row), Some((ListId::Tasks, 7)));
        assert_eq!(hits.at(20, 20), None);
    }

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
}
