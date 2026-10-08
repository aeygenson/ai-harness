//! Pieces every tab uses: clickable areas, panels, buttons and a text form
//! (the form is in `form.rs`).

pub(crate) mod i18n;
pub(crate) mod keys;
pub(crate) mod message;
pub(crate) mod theme;

mod form;

pub(crate) use form::{wrapped_lines, Form};

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Clear};
use ratatui::Frame;

use crate::tabs::mcp::{McpButton, McpCatalogButton};
use crate::tabs::plugins::{PluginButton, PluginCatalogButton};
use crate::tabs::projects::picker::FolderButton;
use crate::tabs::retro::RetroButton;
use crate::tabs::skills::SkillButton;

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
    /// The rest of an open window: a click there does nothing, but it does
    /// not close the window either.
    Window,
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
    /// The catalog of the Agents tab.
    Agents,
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
    /// A button of the folder browser.
    Folder(crate::tabs::projects::picker::FolderButton),
    /// The message field of the Tasks tab.
    Input,
    /// «To ▾», «Model ▾» and «Level ▾» on the Tasks tab; model and level
    /// are for the next launch only.
    Menu(crate::tabs::tasks::Menu),
    Send,
    /// A button of the Skills tab.
    Skill(crate::tabs::skills::SkillButton),
    /// A link in «Files» of the step on the Tasks tab.
    TaskFile(usize),
    /// The title of a window of the Tasks tab: over the whole tab, or back.
    TaskZoom(crate::tabs::tasks::Zoom),
    /// A button of the MCP tab's server list.
    Mcp(crate::tabs::mcp::McpButton),
    /// A button of the MCP registry's catalog.
    McpCatalog(crate::tabs::mcp::McpCatalogButton),
    /// A button of the Plugins tab's plugin list.
    Plugin(crate::tabs::plugins::PluginButton),
    /// A button of «From catalog» or «Catalogs» on the Plugins tab.
    PluginCatalog(crate::tabs::plugins::PluginCatalogButton),
    /// A button of the Retro tab.
    Retro(crate::tabs::retro::RetroButton),
    /// «Check again» on the Agents tab.
    AgentsCheck,
    /// «Install» or «Update» for the agent selected on the Agents tab.
    AgentRun,
    /// «Remove» for the agent selected on the Agents tab.
    AgentRemove,
    /// «Sign in» for the agent selected on the Agents tab.
    AgentSignIn,
    /// «Update Harness», on the Agents tab, in the top bar and in the start window.
    HarnessUpdate,
    /// «Start»: closes the start window.
    CloseSplash,
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
            | ButtonId::HarnessUpdate
            | ButtonId::Send
            | ButtonId::Save
            | ButtonId::UseProject
            | ButtonId::Folder(FolderButton::Choose | FolderButton::CreateFolder)
            | ButtonId::Retro(RetroButton::Generate | RetroButton::Apply)
            | ButtonId::Plugin(PluginButton::OpenCatalog)
            | ButtonId::PluginCatalog(PluginCatalogButton::Add)
            | ButtonId::Mcp(McpButton::OpenCatalog)
            | ButtonId::McpCatalog(McpCatalogButton::Use)
            | ButtonId::Skill(SkillButton::Edit)
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
}
