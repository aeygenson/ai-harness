//! The rows of the steps list: who worked, the verdict, who is next, how
//! long the step took and its summary.

use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;

use harness_core::task::handoff::{NextStep, Role};
use harness_core::task::store::Step;

use super::tab::{TaskView, TasksTab};
use super::times::duration_label;
use crate::tabs::tasks::labels::{next_name, verdict_span};
use crate::ui::i18n::I18n;
use crate::ui::theme;

impl TasksTab {
    /// The rows of the steps list: each role's step with how long it took,
    /// when the commits tell. The user's own steps get no time: it would only be
    /// how long the task waited for them.
    pub(super) fn step_items(&self, task: &TaskView, tr: &I18n) -> Vec<ListItem<'static>> {
        let times = self.times_of(&task.id);
        let took: Vec<Option<String>> = task
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                if step.handoff.role == Role::Human {
                    return None;
                }
                let took = times?.step_took(index)?;
                Some(duration_label(took, tr))
            })
            .collect();
        // One column, as wide as the longest time.
        let width = took.iter().flatten().map(|t| t.chars().count()).max();
        task.steps
            .iter()
            .zip(&took)
            .map(|(step, took)| {
                let column = width.map(|width| (took.as_deref().unwrap_or_default(), width));
                step_item(step, column, tr)
            })
            .collect()
    }
}

/// One row of the steps list. `took` is how long the step took and the
/// width of that column; `None` when no step of the task has a time.
fn step_item(step: &Step, took: Option<(&str, usize)>, tr: &I18n) -> ListItem<'static> {
    let h = &step.handoff;
    let next = match h.next_role {
        NextStep::To(role) => theme::role(role),
        NextStep::Done => theme::ok(),
    };
    let mut spans = vec![
        Span::raw(format!("r{} ", h.round)),
        Span::styled(format!("{:<10} ", h.role.as_str()), theme::role(h.role)),
        verdict_span(h.verdict, tr),
        Span::styled(" → ", theme::dim()),
        Span::styled(format!("{:<10}", next_name(h.next_role, tr)), next),
    ];
    if let Some((took, width)) = took {
        spans.push(Span::styled(format!(" {took:>width$}"), theme::dim()));
    }
    spans.push(Span::raw(format!(" {}", h.summary)));
    ListItem::new(Line::from(spans))
}
