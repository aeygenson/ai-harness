//! Drawing the Retro tab: the retrospectives, the chosen one's text and its proposals.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::config::projects::name_of;
use harness_core::git::HARNESS_DIR;
use harness_core::retro::ops::RetroInfo;
use harness_core::retro::Scope;

use super::tab::{Focus, RetroButton, RetroTab};
use crate::tabs::tasks::draw_list;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId};

impl RetroTab {
    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [main, bottom] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let [retros_area, proposals_area] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(left);
        let dim = theme::dim();

        let items: Vec<ListItem> = self
            .list
            .iter()
            .map(|retro| {
                let scope = scope_text(retro, tr);
                let mut spans = vec![Span::raw(format!("{}  ", retro.number))];
                spans.push(Span::raw(format!(
                    "{:<11}",
                    retro.date.clone().unwrap_or_default()
                )));
                spans.push(Span::styled(scope, dim));
                if retro.retro.is_none() {
                    spans.push(Span::styled(
                        format!(" · {}", tr.t("retro.stats_only")),
                        dim,
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        draw_list(
            frame,
            hits,
            retros_area,
            ListId::Retros,
            &tr.f("retro.list", &[("name", &name_of(&self.root))]),
            items,
            self.row,
            self.focus == Focus::Retros,
        );

        self.draw_proposals(frame, proposals_area, hits, tr);

        let (title, lines) = self.text(tr);
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(&title, false))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            right,
        );

        self.draw_buttons(frame, bottom, hits, tr);
    }

    /// The proposals of the selected retrospective, or why there are none.
    fn draw_proposals(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let dim = theme::dim();
        let green = theme::ok();
        let proposals = self.proposals();
        let items: Vec<ListItem> = proposals
            .iter()
            .map(|p| {
                let (mark, style) = if self.applied(p.id) {
                    ("✓  ", green)
                } else if self.chosen.contains(&p.id) {
                    ("[x]", Style::new())
                } else {
                    ("[ ]", Style::new())
                };
                ListItem::new(Line::styled(
                    format!("{mark} {} {}", p.id, p.summary),
                    style,
                ))
            })
            .collect();
        let empty = items.is_empty();
        draw_list(
            frame,
            hits,
            area,
            ListId::RetroProposals,
            tr.t("retro.proposals"),
            items,
            self.proposal,
            self.focus == Focus::Proposals,
        );
        if empty {
            let note = match self.current().and_then(|r| r.proposals.as_ref()) {
                Some(Err(error)) => error.clone(),
                None if self.current().is_some_and(|r| r.retro.is_none()) => {
                    tr.t("retro.no_agent").to_string()
                }
                Some(Ok(_)) | None => tr.t("retro.no_proposals").to_string(),
            };
            let inner = panel("", false).inner(area);
            frame.render_widget(
                Paragraph::new(Line::styled(note, dim)).wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    /// «Generate», «Open», «Choose» and «Apply».
    fn draw_buttons(&self, frame: &mut Frame, bottom: Rect, hits: &mut Hits, tr: &I18n) {
        let generating = self.generating.is_some();
        let has_text = self.current().is_some_and(|r| r.retro.is_some());
        let can_choose = self.current_proposal().is_some_and(|p| !self.applied(p.id));
        let choose = match self.current_proposal() {
            Some(p) if self.chosen.contains(&p.id) => tr.t("retro.unchoose"),
            _ => tr.t("retro.choose"),
        };
        let apply = tr.f("retro.apply", &[("count", &self.chosen.len())]);
        buttons(
            frame,
            bottom,
            hits,
            &[
                (
                    tr.t("retro.generate"),
                    ButtonId::Retro(RetroButton::Generate),
                    !generating,
                ),
                (
                    tr.t("retro.open"),
                    ButtonId::Retro(RetroButton::Open),
                    has_text,
                ),
                (choose, ButtonId::Retro(RetroButton::Toggle), can_choose),
                (
                    &apply,
                    ButtonId::Retro(RetroButton::Apply),
                    !self.chosen.is_empty(),
                ),
            ],
        );
    }

    /// The right side: the live log while generating, a proposal with what
    /// it changes, or the retrospective with its statistics.
    pub(super) fn text(&self, tr: &I18n) -> (String, Vec<Line<'static>>) {
        let dim = theme::dim();
        let bold = Style::new().add_modifier(Modifier::BOLD);
        if self.generating.is_some() || (self.list.is_empty() && !self.log.is_empty()) {
            let mut lines = vec![Line::styled(tr.t("retro.generating").to_string(), dim)];
            lines.extend(self.log.iter().map(|l| Line::from(l.clone())));
            return (format!(" {} ", tr.t("retro.agent")), lines);
        }
        let Some(retro) = self.current() else {
            let lines = tr
                .t("retro.empty")
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect();
            return (String::new(), lines);
        };
        if self.focus == Focus::Proposals {
            if let Some(proposal) = self.current_proposal() {
                let harness_dir = self.root.join(HARNESS_DIR);
                // After applying, the files already look like the proposal:
                // a diff would only say "already like this".
                let text = match &self.config {
                    _ if self.applied(proposal.id) => format!(
                        "{}\n{}\n\n✓ {}\n",
                        proposal.summary,
                        proposal.reason,
                        tr.f("retro.was_applied", &[("name", &proposal.skill)])
                    ),
                    Some(config) => proposal.describe(&harness_dir, config),
                    None => format!("{}\n{}\n", proposal.summary, proposal.reason),
                };
                let lines = text
                    .lines()
                    .map(|line| {
                        let style = if line.starts_with("+ ") {
                            theme::ok()
                        } else if line.starts_with("- ") {
                            theme::bad()
                        } else {
                            Style::new()
                        };
                        Line::styled(line.to_string(), style)
                    })
                    .collect();
                return (
                    format!(" {} {} ", tr.t("retro.proposal"), proposal.id),
                    lines,
                );
            }
        }
        let scope = scope_text(retro, tr);
        let title = match &retro.date {
            Some(date) => format!(" {} · {scope} · {date} ", retro.number),
            None => format!(" {} · {scope} ", retro.number),
        };
        let mut lines: Vec<Line<'static>> = match &retro.retro {
            Some(text) => text.lines().map(|l| Line::from(l.to_string())).collect(),
            None => vec![Line::styled(tr.t("retro.no_agent").to_string(), dim)],
        };
        if let Some(stats) = &retro.stats {
            lines.push(Line::default());
            lines.push(Line::styled(format!("── {} ──", tr.t("retro.stats")), bold));
            lines.extend(stats.lines().map(|l| Line::styled(l.to_string(), dim)));
        }
        (title, lines)
    }
}

/// What `retro` looked at, in Lisa's language: «all tasks» or the task id.
fn scope_text(retro: &RetroInfo, tr: &I18n) -> String {
    match &retro.scope {
        Some(Scope::All) => tr.t("retro.all").to_string(),
        Some(Scope::Task(id)) => id.clone(),
        None => String::new(),
    }
}
