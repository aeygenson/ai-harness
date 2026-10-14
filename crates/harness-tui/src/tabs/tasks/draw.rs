//! Drawing the Tasks tab: the tasks, the steps, one step, the roles and the live log.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::task::Stage;

use super::message_box::input_height;
use super::steps::{step_item, step_text};
use super::tab::{Focus, TaskView, TasksTab, Zoom};
use crate::tabs::tasks::labels::{short_stage, stage_label};
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{panel, selected, ButtonId, Hits, ListId, Target};

/// The selection mark every list puts before its rows (see [`draw_list`]).
const LIST_MARK: &str = "▶ ";

/// Its width on the screen: every row of a list starts this far to the right.
const LIST_MARK_WIDTH: usize = 2;

impl TasksTab {
    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [area, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(input_height(area))])
                .areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(area);
        let roles_height = u16::try_from(self.roles.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2);
        // The role filter above the tasks it filters.
        let [roles_area, tasks_area] =
            Layout::vertical([Constraint::Length(roles_height), Constraint::Min(3)]).areas(left);
        // The live log takes the bottom of the right side while there is one.
        let [right, log_area] = if self.running.is_some() || !self.log.is_empty() {
            Layout::vertical([Constraint::Percentage(65), Constraint::Percentage(35)]).areas(right)
        } else {
            [right, Rect::default()]
        };
        let [steps_area, detail_area] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(right);
        // One window over the whole tab: the others are not drawn.
        let zoomed = self.zoomed();
        let only = |zoom: Zoom, normal: Rect| match zoomed {
            None => normal,
            Some(z) if z == zoom => area,
            Some(_) => Rect::default(),
        };
        let (steps_area, detail_area, log_area) = (
            only(Zoom::Steps, steps_area),
            only(Zoom::Step, detail_area),
            only(Zoom::Log, log_area),
        );
        let (roles_area, tasks_area) = if zoomed.is_some() {
            (Rect::default(), Rect::default())
        } else {
            (roles_area, tasks_area)
        };
        self.draw_log(frame, log_area, hits, tr);

        self.draw_tasks(frame, tasks_area, hits, tr);
        self.draw_roles(frame, roles_area, hits, tr);

        let Some(task) = self.current() else {
            let empty = match self.filter {
                Some(role) => tr.f("tasks.empty_filtered", &[("role", &role)]),
                None => tr.t("tasks.empty").to_string(),
            };
            let empty = Paragraph::new(empty)
                .block(panel(tr.t("tasks.title"), false))
                .wrap(Wrap { trim: false });
            frame.render_widget(empty, if zoomed.is_some() { area } else { right });
            self.draw_input(frame, input_area, hits, tr);
            return;
        };

        let failures = match task.failures {
            0 => String::new(),
            n => tr.f("tasks.failures", &[("count", &n)]),
        };
        let spent = self.task_spent(task, tr);
        let title = tr.f(
            "tasks.header",
            &[
                ("task", &task.id),
                ("round", &task.state.round),
                ("max", &task.state.max_rounds),
                ("stage", &stage_label(task.state.stage, tr)),
                ("failures", &failures),
                ("usage", &spent),
            ],
        );
        let title = self.zoom_title(Zoom::Steps, &title, steps_area, hits);
        let items: Vec<ListItem> = task.steps.iter().map(|s| step_item(s, tr)).collect();
        draw_list(
            frame,
            hits,
            steps_area,
            ListId::Steps,
            &title,
            items,
            self.step,
            self.focus == Focus::Steps,
        );

        self.draw_step(frame, task, detail_area, hits, tr);
        self.draw_input(frame, input_area, hits, tr);
    }

    /// The list of tasks, each with a mark for where it stands and its pull
    /// request, which a click (or o) opens in the browser.
    fn draw_tasks(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let lines: Vec<Line> = self
            .tasks
            .iter()
            .map(|t| {
                let (mark, style) = match t.state.stage {
                    Stage::Done => ("✓", theme::ok()),
                    Stage::Working(_) => ("●", theme::running()),
                    Stage::WaitingForHuman(_) => ("◆", theme::warn()),
                };
                let mut spans = vec![
                    Span::raw(format!("{}  ", t.id)),
                    Span::styled(format!("{mark} {}", short_stage(t.state.stage, tr)), style),
                ];
                if let Some(pull_request) = &t.state.pull_request {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("PR #{}", pull_request.number),
                        theme::accent(),
                    ));
                }
                Line::from(spans)
            })
            .collect();
        let items: Vec<ListItem> = lines.iter().cloned().map(ListItem::new).collect();
        let title = match self.filter {
            Some(role) => tr.f("tasks.title_filtered", &[("role", &role)]),
            None => tr.t("tasks.title").to_string(),
        };
        let first = draw_list(
            frame,
            hits,
            area,
            ListId::Tasks,
            &title,
            items,
            self.task,
            self.focus == Focus::Tasks,
        );
        self.pull_request_hits(&lines, first, area, hits);
    }

    /// Makes the «PR #7» at the end of the visible task rows clickable.
    fn pull_request_hits(&self, lines: &[Line], first: usize, area: Rect, hits: &mut Hits) {
        let inner = panel("", false).inner(area);
        let visible = lines
            .iter()
            .enumerate()
            .skip(first)
            .take(usize::from(inner.height));
        for (row, (index, line)) in visible.enumerate() {
            if self.tasks[index].state.pull_request.is_none() {
                continue;
            }
            // The list puts the selection mark («▶ ») before every row.
            let width = u16::try_from(line.width() + LIST_MARK_WIDTH).unwrap_or(u16::MAX);
            let link = u16::try_from(line.spans.last().map_or(0, Span::width)).unwrap_or(0);
            let x = inner.x.saturating_add(width.saturating_sub(link));
            let y = inner
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            let link = link.min(inner.right().saturating_sub(x));
            if link > 0 {
                hits.add(
                    Rect::new(x, y, link, 1),
                    Target::Button(ButtonId::TaskPullRequest(index)),
                );
            }
        }
    }

    /// The selected step (or the task description), with its file links clickable.
    fn draw_step(
        &self,
        frame: &mut Frame,
        task: &TaskView,
        area: Rect,
        hits: &mut Hits,
        tr: &I18n,
    ) {
        let (title, text, links) = match task.steps.get(self.step) {
            Some(step) => {
                let usage = self.step_usage(step);
                let (text, links) = step_text(step, &self.artifacts(step), usage, tr);
                let title = tr.f(
                    "tasks.step",
                    &[("round", &step.handoff.round), ("role", &step.handoff.role)],
                );
                (title, text, links)
            }
            None => (
                " task.md ".to_string(),
                Text::from(task.description.clone()),
                Vec::new(),
            ),
        };
        let title = self.zoom_title(Zoom::Step, &title, area, hits);
        let block = panel(&title, false);
        let inner = block.inner(area);
        // Where each link is on the screen, after wrapping and scrolling.
        for (index, &line) in links.iter().enumerate() {
            let before = Paragraph::new(Text::from(text.lines[..line].to_vec()))
                .wrap(Wrap { trim: false })
                .line_count(inner.width);
            let row = u16::try_from(before)
                .unwrap_or(u16::MAX)
                .checked_sub(self.scroll);
            if let Some(row) = row.filter(|r| *r < inner.height) {
                let width = u16::try_from(text.lines[line].width()).unwrap_or(u16::MAX);
                hits.add(
                    Rect::new(inner.x, inner.y + row, width.min(inner.width), 1),
                    Target::Button(ButtonId::TaskFile(index)),
                );
            }
        }
        frame.render_widget(
            Paragraph::new(text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    /// The agents of the roles; a click on a role filters the tasks.
    fn draw_roles(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let block = panel(tr.t("tasks.roles"), false);
        let inner = block.inner(area);
        let items: Vec<ListItem> = self
            .roles
            .iter()
            .map(|(role, line)| {
                let style = if role.is_some() && *role == self.filter {
                    selected()
                } else {
                    Style::new()
                };
                let mark = role.map_or_else(theme::retro, theme::role);
                ListItem::new(Line::from(vec![
                    Span::styled("■ ", mark),
                    Span::raw(line.clone()),
                ]))
                .style(style)
            })
            .collect();
        frame.render_widget(List::new(items).block(block), area);
        hits.add(
            inner,
            Target::List {
                list: ListId::RoleFilter,
                first: 0,
            },
        );
    }

    /// What the agent prints, the latest lines at the bottom.
    /// `title` with ⤢ (or ⤡ when shown over the whole tab); a click on it
    /// toggles that.
    fn zoom_title(&self, zoom: Zoom, title: &str, area: Rect, hits: &mut Hits) -> String {
        let mark = if self.zoomed() == Some(zoom) {
            '⤡'
        } else {
            '⤢'
        };
        let title = format!(" {mark} {} ", title.trim());
        if area.height > 0 {
            let width = u16::try_from(title.chars().count()).unwrap_or(u16::MAX);
            hits.add(
                Rect::new(
                    area.x + 1,
                    area.y,
                    width.min(area.width.saturating_sub(2)),
                    1,
                ),
                Target::Button(ButtonId::TaskZoom(zoom)),
            );
        }
        title
    }

    fn draw_log(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        if area.height == 0 {
            return;
        }
        let title = match (
            &self.running,
            self.running.as_ref().and_then(|r| r.task.as_ref()),
        ) {
            (Some(_), Some(task)) => tr.f("tasks.log_running", &[("task", task)]),
            (Some(_), None) => tr.f("tasks.log_running", &[("task", &"…")]),
            (None, _) => tr.t("tasks.log_title").to_string(),
        };
        let title = self.zoom_title(Zoom::Log, &title, area, hits);
        let rows = usize::from(area.height.saturating_sub(2));
        let lines: Vec<Line> = self
            .log
            .iter()
            .skip(self.log.len().saturating_sub(rows))
            .map(|l| Line::from(l.clone()))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).block(panel(&title, self.running.is_some())),
            area,
        );
    }
}

/// A bordered list whose rows can be clicked.
#[expect(
    clippy::too_many_arguments,
    reason = "every list on every tab is drawn by this one function with these inputs"
)]
pub fn draw_list(
    frame: &mut Frame,
    hits: &mut Hits,
    area: Rect,
    id: ListId,
    title: &str,
    items: Vec<ListItem>,
    selected_row: usize,
    focused: bool,
) -> usize {
    let block = panel(title, focused);
    let inner = block.inner(area);
    let mut state = ListState::default().with_selected(Some(selected_row));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(selected())
            .highlight_symbol(LIST_MARK),
        area,
        &mut state,
    );
    hits.add(
        inner,
        Target::List {
            list: id,
            first: state.offset(),
        },
    );
    state.offset()
}
