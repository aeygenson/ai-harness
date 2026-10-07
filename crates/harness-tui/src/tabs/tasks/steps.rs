//! The steps of a task as the tab shows them: list rows, the step's text and its files.

use std::fs;
use std::path::Path;

use anyhow::Result;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::ListItem;

use harness_core::config::AgentKind;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::retro::usage::{self, Usage};
use harness_core::task::handoff::{FileAction, NextStep, Role};
use harness_core::task::store::{Step, TaskStore};
use harness_core::task::{Stage, WaitReason};

use super::tab::{Artifact, TaskView, TasksTab};
use crate::tabs::tasks::labels::{next_name, severity_label, usage_label, verdict_span};
use crate::ui::i18n::I18n;
use crate::ui::theme;

impl TasksTab {
    /// The files of `step`, as links in its «Files»: what its commit
    /// created, changed or deleted (outside the run records), then its notes
    /// and log. Before the commit is there, what its handoff lists.
    pub fn artifacts(&self, step: &Step) -> Vec<Artifact> {
        let runs = format!("{HARNESS_DIR}/runs/");
        let dir = self.root.join(&step.dir);
        let known = self.commits.borrow().get(&dir).cloned();
        let commit = known.or_else(|| {
            let handoff = dir.join("handoff.json");
            let relative = handoff.strip_prefix(&self.root).ok()?.to_str()?.to_string();
            let files = Repo::open(&self.root)
                .ok()?
                .files_of_commit_adding(&relative)?;
            self.commits.borrow_mut().insert(dir.clone(), files.clone());
            Some(files)
        });
        let changed: Vec<(char, String)> = match commit {
            Some(files) => files
                .into_iter()
                .filter(|(_, path)| !path.starts_with(&runs))
                .map(|(status, path)| {
                    let mark = match status {
                        'A' => '+',
                        'D' => '−',
                        _ => '~',
                    };
                    (mark, path)
                })
                .collect(),
            None => step
                .handoff
                .files
                .iter()
                .filter_map(|file| {
                    let mark = match file.action {
                        FileAction::Created => '+',
                        FileAction::Modified => '~',
                        FileAction::Deleted => '−',
                        FileAction::Read => return None,
                    };
                    Some((mark, file.path.clone()))
                })
                .collect(),
        };
        let mut files: Vec<Artifact> = Vec::new();
        for (mark, shown) in changed {
            let path = self.root.join(&shown);
            if !files.iter().any(|f| f.path == path) {
                files.push(Artifact { mark, shown, path });
            }
        }
        // The step's own files: their name is enough, the folder is long.
        for name in ["notes.md", "agent.log"] {
            let path = dir.join(name);
            if path.is_file() {
                files.push(Artifact {
                    mark: '·',
                    shown: name.to_string(),
                    path,
                });
            }
        }
        files
    }

    /// The tokens and cost the agent of `step` printed in its `agent.log`;
    /// `None` if it printed none or there is no log yet.
    pub(super) fn step_usage(&self, step: &Step) -> Option<Usage> {
        let dir = self.root.join(&step.dir);
        if let Some(known) = self.usages.borrow().get(&dir) {
            return *known;
        }
        // No log yet is not remembered: it may still be written.
        let log = fs::read_to_string(dir.join("agent.log")).ok()?;
        let found = usage::from_log(&log);
        self.usages.borrow_mut().insert(dir, found);
        found
    }

    /// What all steps of `task` spent, for its title: « · $1.20 · 155k
    /// tokens», or nothing when no step's agent reported it.
    pub(super) fn task_spent(&self, task: &TaskView, tr: &I18n) -> String {
        let mut total: Option<Usage> = None;
        for found in task.steps.iter().filter_map(|step| self.step_usage(step)) {
            total.get_or_insert_with(Usage::default).add(found);
        }
        total
            .map(|usage| format!(" · {}", usage_label(&usage, tr)))
            .unwrap_or_default()
    }
}

/// Is it `role`'s turn in the task, or is Lisa's answer for `role` awaited?
pub(super) fn waits_on(task: &TaskView, role: Role) -> bool {
    match task.state.stage {
        Stage::Working(working) => working == role,
        Stage::WaitingForHuman(WaitReason::RoleAskedForHelp(asking)) => asking == role,
        Stage::WaitingForHuman(WaitReason::ApproveDesign) => role == Role::Architect,
        Stage::WaitingForHuman(WaitReason::RoundLimitReached) => task
            .steps
            .last()
            .is_some_and(|s| s.handoff.next_role == NextStep::To(role)),
        Stage::Done => false,
    }
}

/// `target (812 MB), data.bin (60 MB)`
pub(super) fn sized_list(files: &[(String, u64)]) -> String {
    files
        .iter()
        .map(|(path, bytes)| format!("{path} ({} MB)", bytes.div_ceil(1024 * 1024)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn load_task(runs: &Path, id: &str) -> Result<TaskView> {
    let (store, state) = TaskStore::open(runs, id)?;
    Ok(TaskView {
        id: id.to_string(),
        state,
        description: store.description().unwrap_or_default(),
        steps: store.steps()?,
        failures: store.failures()?.len(),
    })
}

pub(super) fn agent_line(who: &str, agent: AgentKind, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{who:<10} {agent} ({model})"),
        None => format!("{who:<10} {agent}"),
    }
}

pub(super) fn step_item(step: &Step, tr: &I18n) -> ListItem<'static> {
    let h = &step.handoff;
    let next = match h.next_role {
        NextStep::To(role) => theme::role(role),
        NextStep::Done => theme::ok(),
    };
    ListItem::new(Line::from(vec![
        Span::raw(format!("r{} ", h.round)),
        Span::styled(format!("{:<10} ", h.role.as_str()), theme::role(h.role)),
        verdict_span(h.verdict, tr),
        Span::styled(" → ", theme::dim()),
        Span::styled(format!("{:<10}", next_name(h.next_role, tr)), next),
        Span::raw(format!(" {}", h.summary)),
    ]))
}

/// The step in full; `files` become links. Also the line of each link.
/// `usage` is what the step's agent reported spending, if anything.
pub(super) fn step_text(
    step: &Step,
    files: &[Artifact],
    usage: Option<Usage>,
    tr: &I18n,
) -> (Text<'static>, Vec<usize>) {
    let h = &step.handoff;
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(vec![
            Span::raw(tr.t("tasks.verdict").to_string()),
            verdict_span(h.verdict, tr),
            Span::raw(format!(" → {}", next_name(h.next_role, tr))),
        ]),
        Line::from(h.summary.clone()),
    ];
    if let Some(usage) = usage {
        let spent = tr.f("tasks.spent", &[("usage", &usage_label(&usage, tr))]);
        lines.push(Line::styled(spent, theme::dim()));
    }
    if !h.issues.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.issues").to_string(), bold));
        for issue in &h.issues {
            let (name, color) = severity_label(issue.severity, tr);
            let location = issue
                .location
                .as_deref()
                .map(|l| format!("{l}  "))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {name:<9} "),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{location}{}", issue.description)),
            ]));
        }
    }
    let mut link_rows = Vec::new();
    if !files.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.files").to_string(), bold));
        for file in files {
            // A deleted file cannot be opened: grey, not a link.
            let look = if file.mark == '−' || !file.path.is_file() {
                theme::dim()
            } else {
                theme::accent().add_modifier(Modifier::UNDERLINED)
            };
            link_rows.push(lines.len());
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", file.mark), theme::dim()),
                Span::styled(file.shown.clone(), look),
            ]));
        }
    }
    if !h.skills_used.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(tr.f(
            "tasks.skills_used",
            &[("skills", &h.skills_used.join(", "))],
        )));
    }
    if !step.notes.trim().is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(tr.t("tasks.notes").to_string(), bold));
        lines.extend(step.notes.lines().map(|l| Line::from(l.to_string())));
    }
    (Text::from(lines), link_rows)
}
