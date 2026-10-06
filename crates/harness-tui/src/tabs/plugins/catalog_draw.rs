//! Drawing «From catalog» on the Plugins tab: the plugins found and the chosen one.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::plugins::catalog::{Entry, Source};

use super::catalog::{unusable, CatalogView, FILTERS};
use crate::tabs::plugins::PluginCatalogButton;
use crate::tabs::roles::RolesTab;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selector, ButtonId, Hits, ListId};

/// «From catalog»: the plugins found on the left, the chosen one on the right.
#[expect(
    clippy::too_many_arguments,
    reason = "the catalog view draws with the same inputs as the rest of the Plugins tab"
)]
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
    let items = catalog_items(&shown, roles, tr);
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

    let (title, lines) = catalog_details(view, roles, tr);
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

/// The rows of «From catalog»: name, agent, and whether it is added or cannot be.
fn catalog_items(shown: &[&Entry], roles: &RolesTab, tr: &I18n) -> Vec<ListItem<'static>> {
    let dim = theme::dim();
    shown
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
        .collect()
}

/// The title and text of the right panel of «From catalog».
fn catalog_details(
    view: &CatalogView,
    roles: &RolesTab,
    tr: &I18n,
) -> (String, Vec<Line<'static>>) {
    let red = theme::bad();
    if let Some(entry) = view.current() {
        (
            format!(" {} · {} ", entry.name, entry.catalog),
            entry_details(entry, roles, tr),
        )
    } else {
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
