//! Drawing «Catalogs» on the Plugins tab: the added catalogs and the chosen one.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::config::AgentKind;

use super::catalog::CatalogsView;
use crate::tabs::plugins::PluginCatalogButton;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, draw_list, panel, ButtonId, Hits, ListId, ListView};

impl CatalogsView {
    /// «Catalogs»: the added catalogs and what each holds.
    pub fn draw(
        &self,
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
        let bold = theme::bold();
        frame.render_widget(
            Line::from(vec![
                Span::styled(format!(" {} ", tr.t("plugins.catalogs_title")), bold),
                Span::styled(busy.unwrap_or_default().to_string(), dim),
            ]),
            top,
        );
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let items: Vec<ListItem> = self
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
            ListView {
                id: ListId::PluginCatalogs,
                title: &format!(" {} ", tr.t("plugins.catalogs")),
                items,
                selected_row: self.row,
                focused: true,
            },
        );
        let lines = catalogs_details(self, tr);
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(
                    &self
                        .current()
                        .map(|c| format!(" {} ", c.name))
                        .unwrap_or_default(),
                    false,
                ))
                .wrap(Wrap { trim: false }),
            right,
        );
        let free = busy.is_none();
        let chosen = free && self.current().is_some();
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
}

/// The right panel of «Catalogs»: the address, version and contents of the chosen one.
fn catalogs_details(view: &CatalogsView, tr: &I18n) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let red = theme::bad();
    let bold = theme::bold();
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
    lines
}
