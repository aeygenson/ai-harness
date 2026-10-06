//! The words the Tasks tab shows for a stage, a verdict, a severity and the
//! next step, in Lisa's language.

use harness_core::task::handoff::{NextStep, Severity, Verdict};
use harness_core::task::{Stage, WaitReason};
use ratatui::style::Color;
use ratatui::text::Span;

use crate::ui::i18n::I18n;
use crate::ui::theme;

/// Where the task is, such as «waiting: approve the design».
pub fn stage_label(stage: Stage, tr: &I18n) -> String {
    match stage {
        Stage::Working(role) => tr.f("stage.working", &[("role", &role.as_str())]),
        Stage::WaitingForHuman(WaitReason::ApproveDesign) => tr.t("stage.approve_design").into(),
        Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(role)) => {
            tr.f("stage.asked_for_help", &[("role", &role.as_str())])
        }
        Stage::WaitingForHuman(WaitReason::RoundLimitReached) => tr.t("stage.round_limit").into(),
        Stage::Done => tr.t("stage.done").into(),
    }
}

/// «done», «working» or «waiting»: short enough for the task list.
pub fn short_stage(stage: Stage, tr: &I18n) -> &str {
    match stage {
        Stage::Working(_) => tr.t("stage.short_working"),
        Stage::WaitingForHuman(_) => tr.t("stage.short_waiting"),
        Stage::Done => tr.t("stage.done"),
    }
}

/// The verdict of a step, in its colour.
pub fn verdict_span(verdict: Verdict, tr: &I18n) -> Span<'static> {
    let (key, style) = match verdict {
        Verdict::Approved => ("verdict.approved", theme::ok()),
        Verdict::Rejected => ("verdict.rejected", theme::bad()),
        Verdict::NeedsHuman => ("verdict.needs_human", theme::warn()),
    };
    Span::styled(tr.t(key).to_string(), style)
}

/// How bad an issue is, and its colour.
pub fn severity_label(severity: Severity, tr: &I18n) -> (&str, Color) {
    let theme = theme::current();
    let (key, color) = match severity {
        Severity::Low => ("severity.low", theme.dim),
        Severity::Medium => ("severity.medium", theme.warn),
        Severity::High => ("severity.high", theme.bad),
        Severity::Critical => ("severity.critical", theme.bad),
    };
    (tr.t(key), color)
}

/// Who works next: a role name, or «done».
pub fn next_name(next: NextStep, tr: &I18n) -> &str {
    match next {
        NextStep::To(role) => role.as_str(),
        NextStep::Done => tr.t("stage.done"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::task::handoff::Role;

    #[test]
    fn stages_are_shown_in_the_chosen_language() {
        let mut tr = I18n::load(None);
        let stage = Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Tester));
        assert_eq!(stage_label(stage, &tr), "waiting: tester asked for help");
        assert_eq!(short_stage(stage, &tr), "waiting");
        tr.next();
        assert_eq!(stage_label(stage, &tr), "ждёт: tester просит помощи");
        assert_eq!(short_stage(Stage::Done, &tr), "готово");
        assert_eq!(next_name(NextStep::Done, &tr), "готово");
        assert_eq!(severity_label(Severity::High, &tr).0, "высокая");
    }
}
