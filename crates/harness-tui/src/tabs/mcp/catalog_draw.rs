//! Drawing the MCP registry catalog: the servers found and the chosen one in full.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::mcp::registry::Entry;

use super::catalog::Catalog;
use super::form::join_words;
use super::tab::{short_name, McpCatalogButton};
use crate::tabs::roles::RolesTab;
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId};

/// The catalog: the servers found on the left, the chosen one on the right.
pub(super) fn draw_catalog(
    catalog: &Catalog,
    frame: &mut Frame,
    area: Rect,
    hits: &mut Hits,
    tr: &I18n,
    roles: &RolesTab,
) {
    let [top, main, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let status = catalog_status(catalog, tr);
    frame.render_widget(
        Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.catalog_title")), bold),
            status,
        ]),
        top,
    );

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(main);
    let items = catalog_items(catalog, roles, tr);
    draw_list(
        frame,
        hits,
        left,
        ListId::McpCatalog,
        &format!(" {} ", tr.t("mcp.catalog_list")),
        items,
        catalog.row,
        true,
    );

    let (title, lines) = match catalog.current() {
        Some(entry) => (format!(" {} ", entry.name), entry_details(entry, tr)),
        None => (
            String::new(),
            tr.t("mcp.catalog_empty")
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect(),
        ),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&title, false))
            .wrap(Wrap { trim: false }),
        right,
    );

    let usable = catalog.current().is_some_and(|e| e.offer.is_some());
    buttons(
        frame,
        bottom,
        hits,
        &[
            (
                tr.t("mcp.search"),
                ButtonId::McpCatalog(McpCatalogButton::Search),
                !catalog.searching,
            ),
            (
                tr.t("mcp.use"),
                ButtonId::McpCatalog(McpCatalogButton::Use),
                usable,
            ),
            (
                tr.t("mcp.back"),
                ButtonId::McpCatalog(McpCatalogButton::Back),
                true,
            ),
        ],
    );
}

/// The line above the catalog: searching, the error, or how many servers were found.
fn catalog_status(catalog: &Catalog, tr: &I18n) -> Span<'static> {
    let dim = theme::dim();
    let red = theme::bad();
    if catalog.searching {
        Span::styled(tr.f("mcp.searching", &[("query", &catalog.query)]), dim)
    } else if let Some(error) = &catalog.error {
        Span::styled(tr.f("mcp.search_failed", &[("error", &error)]), red)
    } else if catalog.query.is_empty() {
        Span::styled(
            tr.f("mcp.found_all", &[("count", &catalog.entries.len())]),
            dim,
        )
    } else {
        Span::styled(
            tr.f(
                "mcp.found",
                &[("count", &catalog.entries.len()), ("query", &catalog.query)],
            ),
            dim,
        )
    }
}

/// The rows of the catalog: the name it would get, and its kind or why it cannot be used.
fn catalog_items(catalog: &Catalog, roles: &RolesTab, tr: &I18n) -> Vec<ListItem<'static>> {
    let dim = theme::dim();
    catalog
        .entries
        .iter()
        .map(|entry| {
            let shown = entry
                .offer
                .as_ref()
                .map_or_else(|| short_name(&entry.name), |o| o.name.clone());
            let added = entry
                .offer
                .as_ref()
                .is_some_and(|o| roles.servers().contains_key(&o.name));
            let style = if entry.offer.is_some() {
                Style::new()
            } else {
                dim
            };
            let mut spans = vec![Span::styled(format!("{shown:<20} "), style)];
            if added {
                spans.push(Span::styled(tr.t("mcp.already_added").to_string(), dim));
            } else if let Some(offer) = &entry.offer {
                spans.push(Span::styled(offer.kind.clone(), dim));
            } else {
                spans.push(Span::styled(tr.t("mcp.web_only").to_string(), dim));
            }
            ListItem::new(Line::from(spans))
        })
        .collect()
}

/// What the catalog shows about one registry server.
fn entry_details(entry: &Entry, tr: &I18n) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let red = theme::bad();
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    if let Some(title) = &entry.title {
        lines.push(Line::styled(title.clone(), bold));
    }
    if let Some(description) = &entry.description {
        lines.push(Line::from(description.clone()));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t("mcp.version")), bold),
        Span::raw(entry.version.clone()),
    ]));
    if let Some(repository) = &entry.repository {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", tr.t("mcp.repository")), bold),
            Span::raw(repository.clone()),
        ]));
    }
    lines.push(Line::default());
    let Some(offer) = &entry.offer else {
        let why = entry.unusable.clone().unwrap_or_default();
        lines.push(Line::styled(tr.f("mcp.cannot_use", &[("why", &why)]), red));
        return lines;
    };
    let (label, shown, values, none, title) = match &offer.server.url {
        Some(url) => (
            "mcp.address",
            url.clone(),
            &offer.server.headers,
            "mcp.no_headers",
            "mcp.headers",
        ),
        None => (
            "mcp.command",
            join_words(offer.server.command.iter().chain(&offer.server.args)),
            &offer.server.env,
            "mcp.no_variables",
            "mcp.variables",
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", tr.t(label)), bold),
        Span::raw(shown),
    ]));
    lines.push(Line::default());
    if offer.variables.is_empty() {
        lines.push(Line::styled(tr.t(none).to_string(), dim));
    } else {
        lines.push(Line::styled(tr.t(title).to_string(), bold));
        for (variable, description) in &offer.variables {
            let value = match values.get(variable) {
                Some(value) if value.is_empty() => tr.t("mcp.fill_in").to_string(),
                Some(value) => value.clone(),
                None => tr.t("mcp.optional").to_string(),
            };
            lines.push(Line::from(format!("  {variable} = {value}")));
            if !description.is_empty() {
                lines.push(Line::styled(format!("    {description}"), dim));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::styled(tr.t("mcp.use_hint").to_string(), dim));
    lines
}
