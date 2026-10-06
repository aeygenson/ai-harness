//! «From catalog» and «Catalogs» on the Plugins tab.
//!
//! ```text
//!  Plugins in the catalogs · search: review          Agent: [ claude ] [ codex ] [ all ]
//! ┌ Found 12 ───────────────────────────┐┌ code-review · claude-plugins-official ─┐
//! │> code-review      claude  added     ││ Reviews the diff for bugs and style.    │
//! │  pr-review        claude            ││ From: a folder in the catalog           │
//! │  npm-thing        claude  cannot    ││                                         │
//! └─────────────────────────────────────┘└─────────────────────────────────────────┘
//!  [ Search ] [ Add ] [ Add and give to the developer ] [ Catalogs ] [ Back ]
//! ```
//!
//! The list is every plugin of every added catalog, read from their copies
//! in `~/.harness/marketplaces/`, so searching is instant. Adding downloads
//! the plugin (in the background) and copies it into the project; nothing
//! of a catalog reaches an agent until then. The «Catalogs» screen adds,
//! updates and removes catalogs; the official Claude catalog is added by
//! itself the first time the catalog opens with none.

use std::path::Path;

use harness_core::config::AgentKind;
use harness_core::plugins::catalog::{Entry, Source};
use harness_core::plugins::ops::{self, CatalogInfo};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::plugins::PluginCatalogButton;
use crate::tabs::roles::RolesTab;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

/// The catalog anyone starts with: Anthropic's plugins for Claude Code.
pub const OFFICIAL: &str = "anthropics/claude-plugins-official";

/// The agent filter: index into these; `None` (all agents) last.
pub const FILTERS: [Option<AgentKind>; 3] = [Some(AgentKind::Claude), Some(AgentKind::Codex), None];

/// The plugins of all catalogs, while «From catalog» is open.
#[derive(Debug, Default)]
pub struct CatalogView {
    pub entries: Vec<Entry>,
    /// Catalogs that cannot be read.
    pub errors: Vec<String>,
    /// Words to look for in the name and description.
    pub query: String,
    /// Index into `FILTERS`.
    pub filter: usize,
    pub row: usize,
}

impl CatalogView {
    /// Reads every added catalog; `agent` sets the filter (all agents when
    /// it has no plugins).
    pub fn load(home: Option<&Path>, agent: Option<AgentKind>) -> Self {
        let mut view = Self {
            filter: FILTERS
                .iter()
                .position(|f| *f == agent)
                .unwrap_or(FILTERS.len() - 1),
            ..Self::default()
        };
        view.reload(home);
        view
    }

    pub fn reload(&mut self, home: Option<&Path>) {
        self.entries.clear();
        self.errors.clear();
        let Some(home) = home else {
            return;
        };
        match ops::catalogs(home) {
            Ok(catalogs) => {
                for catalog in catalogs {
                    match catalog.entries {
                        Ok(entries) => self.entries.extend(entries),
                        Err(error) => self.errors.push(format!("{}: {error}", catalog.name)),
                    }
                }
            }
            Err(error) => self.errors.push(error.to_string()),
        }
        self.row = self.row.min(self.shown().len().saturating_sub(1));
    }

    /// The entries that match the filter and the search, those that can be
    /// added first.
    pub fn shown(&self) -> Vec<&Entry> {
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let agent = FILTERS[self.filter.min(FILTERS.len() - 1)];
        let mut shown: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| agent.is_none_or(|agent| e.agent == agent))
            .filter(|e| {
                let text = format!("{} {}", e.name, e.description).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
        shown.sort_by_key(|e| unusable(e).is_some());
        shown
    }

    pub fn current(&self) -> Option<&Entry> {
        self.shown().get(self.row).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.shown().len();
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    pub fn select(&mut self, index: usize) {
        if index < self.shown().len() {
            self.row = index;
        }
    }

    pub fn choose_filter(&mut self, index: usize) {
        if index < FILTERS.len() {
            self.filter = index;
            self.row = 0;
        }
    }
}

/// Why a catalog plugin cannot be added yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// It comes from a kind of source the harness cannot download.
    Source(String),
    /// Its name could not be a folder name.
    Name,
}

impl Unusable {
    /// What is not supported, in Lisa's language.
    pub fn text(&self, tr: &I18n) -> String {
        match self {
            Unusable::Source(kind) => tr.f("plugins.unusable_source", &[("kind", kind)]),
            Unusable::Name => tr.t("plugins.unusable_name").to_string(),
        }
    }
}

/// Why an entry cannot be added, if it cannot.
pub fn unusable(entry: &Entry) -> Option<Unusable> {
    match &entry.source {
        Source::Unsupported(kind) => Some(Unusable::Source(kind.to_string())),
        _ if !harness_core::mcp::is_simple_name(&entry.name) => Some(Unusable::Name),
        _ => None,
    }
}

/// The added catalogs, while «Catalogs» is open.
#[derive(Debug, Default)]
pub struct CatalogsView {
    pub list: Vec<CatalogInfo>,
    pub row: usize,
    pub error: Option<String>,
}

impl CatalogsView {
    pub fn load(home: Option<&Path>) -> Self {
        let mut view = Self::default();
        view.reload(home);
        view
    }

    pub fn reload(&mut self, home: Option<&Path>) {
        match home.map(ops::catalogs) {
            Some(Ok(list)) => {
                self.list = list;
                self.error = None;
            }
            Some(Err(error)) => self.error = Some(error.to_string()),
            None => self.list.clear(),
        }
        self.row = self.row.min(self.list.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&CatalogInfo> {
        self.list.get(self.row)
    }

    pub fn move_by(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.list.len().saturating_sub(1));
    }

    pub fn select(&mut self, index: usize) {
        if index < self.list.len() {
            self.row = index;
        }
    }
}

/// «From catalog»: the plugins found on the left, the chosen one on the right.
#[allow(clippy::too_many_arguments)]
pub fn draw_catalog(
    view: &CatalogView,
    frame: &mut Frame,
    area: Rect,
    hits: &mut Hits,
    tr: &I18n,
    roles: &RolesTab,
    role: &str,
    busy: Option<&str>,
) {
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let [title_area, filter_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(44)]).areas(top);
    let status = match busy {
        Some(what) => Span::styled(what.to_string(), dim),
        None if view.query.is_empty() => Span::raw(""),
        None => Span::styled(tr.f("plugins.searched", &[("query", &view.query)]), dim),
    };
    frame.render_widget(
        Line::from(vec![
            Span::styled(format!(" {} ", tr.t("plugins.catalog_title")), bold),
            status,
        ]),
        title_area,
    );
    let labels: Vec<&str> = FILTERS
        .iter()
        .map(|f| f.map_or(tr.t("plugins.all"), |agent| agent.as_str()))
        .collect();
    selector(
        frame,
        filter_area,
        hits,
        tr.t("plugins.agent"),
        &labels,
        view.filter,
        |index| ButtonId::PluginCatalog(PluginCatalogButton::Filter(index)),
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main);
    let shown = view.shown();
    let items: Vec<ListItem> = shown
        .iter()
        .map(|entry| {
            let added = roles
                .plugins()
                .values()
                .any(|p| p.source.as_deref() == Some(entry.id().as_str()));
            let style = if unusable(entry).is_some() {
                dim
            } else {
                Style::new()
            };
            let note = if added {
                tr.t("plugins.added_mark")
            } else if unusable(entry).is_some() {
                tr.t("plugins.cannot_mark")
            } else {
                ""
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<20} ", entry.name), style),
                Span::styled(format!("{:<7}", entry.agent), dim),
                Span::styled(note.to_string(), dim),
            ]))
        })
        .collect();
    draw_list(
        frame,
        hits,
        left,
        ListId::PluginCatalog,
        &format!(" {} ", tr.f("plugins.found", &[("count", &shown.len())])),
        items,
        view.row,
        true,
    );

    let (title, lines) = match view.current() {
        Some(entry) => (
            format!(" {} · {} ", entry.name, entry.catalog),
            entry_details(entry, roles, tr),
        ),
        None => {
            let mut lines: Vec<Line> = if view.entries.is_empty() {
                tr.t("plugins.no_catalogs")
            } else {
                tr.t("plugins.nothing_found")
            }
            .lines()
            .map(|l| Line::from(l.to_string()))
            .collect();
            for error in &view.errors {
                lines.push(Line::styled(error.clone(), red));
            }
            (String::new(), lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&title, false))
            .wrap(Wrap { trim: false }),
        right,
    );

    let can_add = busy.is_none() && view.current().is_some_and(|e| unusable(e).is_none());
    let give = tr.f("plugins.add_give", &[("role", &role)]);
    buttons(
        frame,
        bottom,
        hits,
        &[
            (
                tr.t("plugins.search"),
                ButtonId::PluginCatalog(PluginCatalogButton::Search),
                true,
            ),
            (
                tr.t("plugins.add"),
                ButtonId::PluginCatalog(PluginCatalogButton::Add),
                can_add,
            ),
            (
                &give,
                ButtonId::PluginCatalog(PluginCatalogButton::AddGive),
                can_add,
            ),
            (
                tr.t("plugins.catalogs"),
                ButtonId::PluginCatalog(PluginCatalogButton::OpenCatalogs),
                true,
            ),
            (
                tr.t("plugins.back"),
                ButtonId::PluginCatalog(PluginCatalogButton::Back),
                true,
            ),
        ],
    );
}

/// What the catalog shows about one plugin.
fn entry_details(entry: &Entry, roles: &RolesTab, tr: &I18n) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    if !entry.description.is_empty() {
        lines.push(Line::from(entry.description.clone()));
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t("plugins.agent")), bold),
        Span::raw(entry.agent.to_string()),
    ]));
    let from = match &entry.source {
        Source::InCatalog(path) => tr.f("plugins.in_catalog", &[("path", &path)]),
        Source::Git { url, path, .. } => match path {
            Some(path) => format!("{url} ({path})"),
            None => url.clone(),
        },
        Source::Unsupported(kind) => kind.clone(),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t("plugins.from")), bold),
        Span::raw(from),
    ]));
    lines.push(Line::default());
    if let Some(why) = unusable(entry) {
        lines.push(Line::styled(
            tr.f("plugins.cannot_add", &[("why", &why.text(tr))]),
            red,
        ));
        return lines;
    }
    if let Some((name, _)) = roles
        .plugins()
        .iter()
        .find(|(_, p)| p.source.as_deref() == Some(entry.id().as_str()))
    {
        lines.push(Line::styled(
            tr.f("plugins.already_added", &[("name", &name)]),
            dim,
        ));
    } else if roles.plugins().contains_key(&entry.name) {
        lines.push(Line::styled(
            tr.f("plugins.name_taken", &[("name", &entry.name)]),
            red,
        ));
    } else {
        lines.push(Line::styled(tr.t("plugins.add_hint").to_string(), dim));
    }
    lines
}

/// «Catalogs»: the added catalogs and what each holds.
pub fn draw_catalogs(
    view: &CatalogsView,
    frame: &mut Frame,
    area: Rect,
    hits: &mut Hits,
    tr: &I18n,
    busy: Option<&str>,
) {
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    frame.render_widget(
        Line::from(vec![
            Span::styled(format!(" {} ", tr.t("plugins.catalogs_title")), bold),
            Span::styled(busy.unwrap_or_default().to_string(), dim),
        ]),
        top,
    );
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main);
    let items: Vec<ListItem> = view
        .list
        .iter()
        .map(|catalog| {
            let count = match &catalog.entries {
                Ok(entries) => tr.f("plugins.count", &[("count", &entries.len())]),
                Err(_) => tr.t("plugins.unreadable").to_string(),
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{:<28} ", catalog.name)),
                Span::styled(count, dim),
            ]))
        })
        .collect();
    draw_list(
        frame,
        hits,
        left,
        ListId::PluginCatalogs,
        &format!(" {} ", tr.t("plugins.catalogs")),
        items,
        view.row,
        true,
    );
    let mut lines = Vec::new();
    match view.current() {
        Some(catalog) => {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", tr.t("plugins.address")), bold),
                Span::raw(catalog.config.source.clone()),
            ]));
            if let Some(commit) = &catalog.commit {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", tr.t("plugins.version")), bold),
                    Span::raw(commit[..commit.len().min(7)].to_string()),
                ]));
            }
            match &catalog.entries {
                Ok(entries) => {
                    let claude = entries
                        .iter()
                        .filter(|e| e.agent == AgentKind::Claude)
                        .count();
                    lines.push(Line::from(tr.f(
                        "plugins.catalog_counts",
                        &[("claude", &claude), ("codex", &(entries.len() - claude))],
                    )));
                }
                Err(error) => lines.push(Line::styled(error.clone(), red)),
            }
            lines.push(Line::default());
            lines.push(Line::styled(tr.t("plugins.catalogs_hint").to_string(), dim));
        }
        None => lines.extend(
            tr.t("plugins.catalogs_empty")
                .lines()
                .map(|l| Line::from(l.to_string())),
        ),
    }
    if let Some(error) = &view.error {
        lines.push(Line::styled(error.clone(), red));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(
                &view
                    .current()
                    .map(|c| format!(" {} ", c.name))
                    .unwrap_or_default(),
                false,
            ))
            .wrap(Wrap { trim: false }),
        right,
    );
    let free = busy.is_none();
    let chosen = free && view.current().is_some();
    buttons(
        frame,
        bottom,
        hits,
        &[
            (
                tr.t("plugins.add_catalog"),
                ButtonId::PluginCatalog(PluginCatalogButton::CatalogAdd),
                free,
            ),
            (
                tr.t("plugins.update_catalog"),
                ButtonId::PluginCatalog(PluginCatalogButton::CatalogUpdate),
                chosen,
            ),
            (
                tr.t("plugins.remove_catalog"),
                ButtonId::PluginCatalog(PluginCatalogButton::CatalogRemove),
                chosen,
            ),
            (
                tr.t("plugins.back"),
                ButtonId::PluginCatalog(PluginCatalogButton::Back),
                true,
            ),
        ],
    );
}
