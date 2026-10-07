//! Counting the statistics from the saved history of the tasks.

use std::collections::{BTreeMap, BTreeSet};

use super::usage::Usage;
use super::{
    stage_text, RepeatedIssue, Return, RoleStats, Scope, SeverityCounts, SkillSetting, SkillUse,
    Stats, TaskHistory, TaskSummary,
};
use crate::config::Config;
use crate::task::handoff::{NextStep, Role, Severity, Verdict};

impl Stats {
    /// Counts everything in `tasks`. `config` is the current harness.toml,
    /// used to find skills that were configured but never used.
    pub fn collect(scope: Scope, tasks: &[TaskHistory], config: Option<&Config>) -> Self {
        let mut roles: BTreeMap<Role, RoleStats> = BTreeMap::new();
        let mut returns: BTreeMap<(Role, Role), usize> = BTreeMap::new();
        let mut issues = SeverityCounts::default();
        let mut repeated: BTreeMap<String, RepeatedIssue> = BTreeMap::new();
        let mut used: BTreeMap<(Role, String), usize> = BTreeMap::new();
        let mut usage: BTreeMap<Role, Usage> = BTreeMap::new();
        let mut summaries = Vec::new();

        for task in tasks {
            for handoff in &task.handoffs {
                let role = roles.entry(handoff.role).or_default();
                role.steps += 1;
                match handoff.verdict {
                    Verdict::Approved => role.approved += 1,
                    Verdict::Rejected => role.rejected += 1,
                    Verdict::NeedsHuman => role.needs_human += 1,
                }
                role.issues_found += handoff.issues.len();

                if let (Verdict::Rejected, NextStep::To(to)) = (handoff.verdict, handoff.next_role)
                {
                    if to != handoff.role {
                        *returns.entry((handoff.role, to)).or_default() += 1;
                    }
                }

                for issue in &handoff.issues {
                    issues.add(issue.severity);
                    let entry = repeated
                        .entry(normalize(&issue.description))
                        .or_insert_with(|| RepeatedIssue {
                            description: issue.description.trim().to_string(),
                            count: 0,
                            severity: issue.severity,
                            roles: Vec::new(),
                            tasks: Vec::new(),
                        });
                    entry.count += 1;
                    if rank(issue.severity) > rank(entry.severity) {
                        entry.severity = issue.severity;
                    }
                    push_new(&mut entry.roles, handoff.role);
                    push_new(&mut entry.tasks, task.state.task_id.clone());
                }

                // A skill listed twice in one handoff is one use.
                let skills: BTreeSet<&str> = handoff.skills_used.iter().map(|s| s.trim()).collect();
                for skill in skills.into_iter().filter(|s| !s.is_empty()) {
                    *used.entry((handoff.role, skill.to_string())).or_default() += 1;
                }
            }
            for (&role, &found) in &task.usage {
                usage.entry(role).or_default().add(found);
            }
            for &(_, role) in &task.failures {
                roles.entry(role).or_default().failed_attempts += 1;
            }
            summaries.push(TaskSummary {
                task_id: task.state.task_id.clone(),
                rounds: task.state.round,
                max_rounds: task.state.max_rounds,
                stage: stage_text(task.state.stage),
                steps: task.handoffs.len(),
                human_decisions: task
                    .handoffs
                    .iter()
                    .filter(|h| h.role == Role::Human)
                    .count(),
                failed_attempts: task.failures.len(),
            });
        }

        let mut returns: Vec<Return> = returns
            .into_iter()
            .map(|((from, to), count)| Return { from, to, count })
            .collect();
        returns.sort_by_key(|a| std::cmp::Reverse(a.count));

        let mut repeated: Vec<RepeatedIssue> =
            repeated.into_values().filter(|i| i.count > 1).collect();
        repeated.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(rank(b.severity).cmp(&rank(a.severity)))
        });

        Self {
            scope,
            tasks: summaries,
            roles,
            returns,
            issues,
            repeated_issues: repeated,
            skills: config.map(|c| skill_uses(c, &used)).unwrap_or_default(),
            usage,
        }
    }
}

/// One line per (role, skill): every configured skill and every used one.
pub(super) fn skill_uses(config: &Config, used: &BTreeMap<(Role, String), usize>) -> Vec<SkillUse> {
    let mut rows: BTreeMap<(Role, String), SkillSetting> = BTreeMap::new();
    for (&role, settings) in &config.roles {
        for skill in &settings.skills {
            rows.insert((role, skill.clone()), SkillSetting::OnDemand);
        }
        for skill in &settings.always_skills {
            rows.insert((role, skill.clone()), SkillSetting::Always);
        }
    }
    for (role, skill) in used.keys() {
        let from_plugin = skill.split_once(':').is_some_and(|(plugin, _)| {
            config
                .roles
                .get(role)
                .is_some_and(|r| r.plugins.iter().any(|p| p == plugin))
        });
        rows.entry((*role, skill.clone()))
            .or_insert(if from_plugin {
                SkillSetting::Plugin
            } else {
                SkillSetting::NotConfigured
            });
    }
    rows.into_iter()
        .map(|((role, skill), setting)| {
            let used = used.get(&(role, skill.clone())).copied().unwrap_or(0);
            SkillUse {
                role,
                skill,
                setting,
                used,
            }
        })
        .collect()
}

/// Descriptions that differ only in case, spaces or a final dot are the same.
pub(super) fn normalize(description: &str) -> String {
    description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .to_lowercase()
}

pub(super) fn push_new<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

pub(super) fn rank(severity: Severity) -> u8 {
    match severity {
        Severity::Low => 0,
        Severity::Medium => 1,
        Severity::High => 2,
        Severity::Critical => 3,
    }
}
