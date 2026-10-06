//! Writing the statistics as Markdown, for the terminal and `stats.md`.

use std::fmt::Write as _;

use super::{SkillSetting, Stats};
use crate::task::handoff::Severity;

impl Stats {
    /// The statistics as Markdown, for the terminal and `stats.md`.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "# Retrospective: {}\n", self.scope);
        self.tasks_markdown(&mut md);
        self.roles_markdown(&mut md);
        self.returns_markdown(&mut md);
        self.issues_markdown(&mut md);
        self.skills_markdown(&mut md);
        md
    }

    /// The «Tasks» table: one row per task.
    fn tasks_markdown(&self, md: &mut String) {
        md.push_str("## Tasks\n\n");
        md.push_str("| Task | Rounds | Stage | Steps | Lisa's decisions | Failed attempts |\n");
        md.push_str("|---|---|---|---|---|---|\n");
        for t in &self.tasks {
            let _ = writeln!(
                md,
                "| {} | {} of {} | {} | {} | {} | {} |",
                t.task_id,
                t.rounds,
                t.max_rounds,
                t.stage,
                t.steps,
                t.human_decisions,
                t.failed_attempts
            );
        }
    }

    /// The «Roles» table: steps and verdicts of each role.
    fn roles_markdown(&self, md: &mut String) {
        md.push_str("\n## Roles\n\n");
        if self.roles.is_empty() {
            md.push_str("No role has finished a step yet.\n");
        } else {
            md.push_str("| Role | Steps | Approved | Rejected | Needs human | Failed attempts | Issues found |\n");
            md.push_str("|---|---|---|---|---|---|---|\n");
            for (role, r) in &self.roles {
                let _ = writeln!(
                    md,
                    "| {} | {} | {} | {} | {} | {} | {} |",
                    role.as_str(),
                    r.steps,
                    r.approved,
                    r.rejected,
                    r.needs_human,
                    r.failed_attempts,
                    r.issues_found
                );
            }
        }
    }

    /// The «Work sent back» list: which role sent work back to which.
    fn returns_markdown(&self, md: &mut String) {
        md.push_str("\n## Work sent back\n\n");
        if self.returns.is_empty() {
            md.push_str("Nothing was sent back.\n");
        }
        for r in &self.returns {
            let _ = writeln!(
                md,
                "- {} -> {}: {} {}",
                r.from.as_str(),
                r.to.as_str(),
                r.count,
                times(r.count)
            );
        }
    }

    /// The «Issues» section: counts by severity and the repeated issues.
    fn issues_markdown(&self, md: &mut String) {
        md.push_str("\n## Issues\n\n");
        let i = &self.issues;
        let _ = writeln!(
            md,
            "{} in total: critical {}, high {}, medium {}, low {}.",
            i.total(),
            i.critical,
            i.high,
            i.medium,
            i.low
        );
        if !self.repeated_issues.is_empty() {
            md.push_str("\nFound more than once:\n\n");
        }
        for issue in &self.repeated_issues {
            let roles: Vec<&str> = issue.roles.iter().map(|r| r.as_str()).collect();
            let _ = writeln!(
                md,
                "- {} {}, {}: {} (by {}; in {})",
                issue.count,
                times(issue.count),
                severity_name(issue.severity),
                issue.description,
                roles.join(", "),
                issue.tasks.join(", ")
            );
        }
    }

    /// The «Skills» table and the skills that were configured but never used.
    fn skills_markdown(&self, md: &mut String) {
        md.push_str("\n## Skills\n\n");
        if self.skills.is_empty() {
            md.push_str("No skills configured or used.\n");
        } else {
            md.push_str("| Role | Skill | Setting | Listed in skills_used |\n");
            md.push_str("|---|---|---|---|\n");
            for s in &self.skills {
                let setting = match s.setting {
                    SkillSetting::OnDemand => "skills",
                    SkillSetting::Always => "always_skills",
                    SkillSetting::Plugin => "plugin",
                    SkillSetting::NotConfigured => "not configured",
                };
                let _ = writeln!(
                    md,
                    "| {} | {} | {} | {} |",
                    s.role.as_str(),
                    s.skill,
                    setting,
                    s.used
                );
            }
            let unused: Vec<String> = self
                .unused_skills()
                .map(|s| format!("{} ({})", s.skill, s.role.as_str()))
                .collect();
            if !unused.is_empty() {
                let _ = writeln!(md, "\nConfigured but never used: {}.", unused.join(", "));
            }
        }
    }
}

pub(super) fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

pub(super) fn times(count: usize) -> &'static str {
    if count == 1 {
        "time"
    } else {
        "times"
    }
}
