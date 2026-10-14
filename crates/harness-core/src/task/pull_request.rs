//! The pull request a finished task becomes on GitHub: its title and text,
//! made from the task's description and handoffs.
//!
//! Everything here is text written by agents, so it is treated as untrusted:
//! terminal control characters and saved secrets are removed, and the
//! agents' words go into code blocks, where GitHub shows them as they are
//! (no links, pictures or `@` mentions that would notify people).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::secret;
use crate::task::handoff::{Handoff, NextStep, Severity, Verdict};
use crate::task::store::Step;
use crate::text;

/// A pull request on GitHub, as `state.json` remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    /// Its number in the repository, for example `12`.
    pub number: u64,
    /// Its page, for example `https://github.com/lisa/app/pull/12`.
    pub url: String,
}

impl PullRequest {
    /// The page to show and open: only when it is a pull request page on
    /// GitHub with this number. `state.json` may have come from anywhere (a
    /// merge, a clone), and the address is given to the browser.
    pub fn page(&self) -> Option<&str> {
        let rest = self.url.strip_prefix("https://github.com/")?;
        let parts: Vec<&str> = rest.split('/').collect();
        let name_part = |part: &str| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let valid = parts.len() == 4
            && name_part(parts[0])
            && name_part(parts[1])
            && parts[2] == "pull"
            && parts[3] == self.number.to_string();
        valid.then_some(self.url.as_str())
    }
}

/// GitHub cuts a title at 256 characters; a short one is easier to read.
const MAX_TITLE_CHARS: usize = 120;

/// GitHub refuses a pull request text longer than 65 536 characters; this
/// leaves room for the task, the last lines and the note that steps were left out.
const MAX_BODY_CHARS: usize = 55_000;

/// The task's description is Lisa's own text, but may be long: at most this
/// much of it goes into the pull request.
const MAX_TASK_CHARS: usize = 5_000;

/// The title: the first line of the task's description, or the task's id
/// when the description is empty.
pub fn title(task_id: &str, description: &str, secrets: &[&str]) -> String {
    let first = description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(task_id);
    text::safe_line(&secret::hide(&text::safe(first), secrets), MAX_TITLE_CHARS)
}

/// The text: the task, then what each role did and found, and where the
/// roles' notes are. `secrets` are replaced with `***`. When it would be too
/// long for GitHub, the last steps are left out and the text says so.
pub fn body(task_id: &str, description: &str, steps: &[Step], secrets: &[&str]) -> String {
    let task: String = description.chars().take(MAX_TASK_CHARS).collect();
    let mut out = format!("Task `{task_id}`, led through the roles by AI Harness.\n\n## Task\n\n");
    out.push_str(&quoted(&task, secrets));
    out.push_str("\n## Steps\n");
    let mut length = out.chars().count();
    for step in steps {
        let text = step_text(&step.handoff, secrets);
        length += text.chars().count();
        if length > MAX_BODY_CHARS {
            out.push_str("\n(The text got too long: the later steps are left out.)\n");
            break;
        }
        out.push_str(&text);
    }
    // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
    let _ = writeln!(
        out,
        "\nEach role's notes are in `.harness/runs/{task_id}/` on this branch."
    );
    out
}

/// One step: who, what they decided and where the work went, then their
/// summary and issues in a code block.
fn step_text(handoff: &Handoff, secrets: &[&str]) -> String {
    let next = match handoff.next_role {
        NextStep::To(role) => role.as_str(),
        NextStep::Done => "done",
    };
    let mut words = handoff.summary.clone();
    for issue in &handoff.issues {
        let place = issue
            .location
            .as_deref()
            .map(|place| format!(" {place}:"))
            .unwrap_or_default();
        let _ = write!(
            words,
            "\n- [{}]{place} {}",
            severity(issue.severity),
            issue.description
        );
    }
    format!(
        "\n### Round {}: {}, {} → {next}\n\n{}",
        handoff.round,
        handoff.role.as_str(),
        verdict(handoff.verdict),
        quoted(&words, secrets)
    )
}

/// `text` without control characters and secrets, in a code block whose
/// fence is longer than any run of backticks in it, so it cannot end early.
fn quoted(text: &str, secrets: &[&str]) -> String {
    // Control characters go first: one hidden inside a secret would
    // otherwise keep `hide` from finding it, and vanish only afterwards.
    let clean = secret::hide(&text::safe(text), secrets);
    let longest = clean.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{}\n{fence}\n", clean.trim_end())
}

fn verdict(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Approved => "approved",
        Verdict::Rejected => "rejected",
        Verdict::NeedsHuman => "needs human",
    }
}

fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
        Severity::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::task::handoff::{Issue, Role, SCHEMA_VERSION};

    fn step(role: Role, verdict: Verdict, next: NextStep, summary: &str) -> Step {
        Step {
            dir: PathBuf::from("round-01/01-x"),
            handoff: Handoff {
                schema_version: SCHEMA_VERSION,
                task_id: "task-001".into(),
                round: 1,
                role,
                verdict,
                next_role: next,
                summary: summary.into(),
                skills_used: vec![],
                files: vec![],
                issues: vec![],
            },
            notes: String::new(),
        }
    }

    #[test]
    fn the_title_is_the_first_line_of_the_task() {
        assert_eq!(
            title("task-001", "\n  Add a parser  \nwith tests", &[]),
            "Add a parser"
        );
        assert_eq!(title("task-001", "  \n", &[]), "task-001");
        assert_eq!(title("task-001", &"x".repeat(300), &[]).len(), 120);
        assert_eq!(
            title("task-001", "use key sk-12345678", &["sk-12345678"]),
            "use key ***"
        );
    }

    #[test]
    fn the_body_tells_each_step_with_its_issues() {
        let mut tester = step(
            Role::Tester,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
            "Two tests fail",
        );
        tester.handoff.issues.push(Issue {
            severity: Severity::High,
            location: Some("src/parser.rs:42".into()),
            description: "Panics on empty input".into(),
        });
        let steps = [
            step(
                Role::Architect,
                Verdict::Approved,
                NextStep::To(Role::Human),
                "Design in docs/parser.md",
            ),
            tester,
            step(
                Role::Security,
                Verdict::Approved,
                NextStep::Done,
                "Nothing found",
            ),
        ];

        let body = body("task-001", "Add a parser", &steps, &[]);

        for part in [
            "Task `task-001`",
            "```text\nAdd a parser\n```",
            "### Round 1: architect, approved → human",
            "### Round 1: tester, rejected → developer",
            "Two tests fail\n- [high] src/parser.rs:42: Panics on empty input",
            "### Round 1: security, approved → done",
            "`.harness/runs/task-001/`",
        ] {
            assert!(body.contains(part), "missing {part:?} in:\n{body}");
        }
    }

    #[test]
    fn agent_text_cannot_leave_its_code_block_or_show_secrets() {
        let summary = "done\n````\n@lisa ![x](https://evil.example/p.png)\u{1b}[2J key sk-12345678";
        let steps = [step(
            Role::Developer,
            Verdict::Approved,
            NextStep::Done,
            summary,
        )];

        let body = body("task-001", "task", &steps, &["sk-12345678"]);

        // The fence is longer than the four backticks inside.
        assert!(body.contains("`````text\ndone\n````\n@lisa"), "{body}");
        assert!(body.contains("key ***\n`````\n"), "{body}");
        assert!(!body.contains("sk-12345678"), "{body}");
        assert!(!body.contains('\u{1b}'), "{body}");
    }

    #[test]
    fn a_very_long_body_leaves_out_the_last_steps() {
        let steps: Vec<Step> = (0..40)
            .map(|_| {
                step(
                    Role::Developer,
                    Verdict::Approved,
                    NextStep::Done,
                    &"x".repeat(2_000),
                )
            })
            .collect();

        let body = body("task-001", &"t".repeat(9_000), &steps, &[]);

        assert!(body.chars().count() < 65_536);
        assert!(body.contains("the later steps are left out"), "{body}");
        assert!(body.ends_with("on this branch.\n"), "{body}");
        // Every code block that was opened is closed.
        assert_eq!(
            body.matches("```text").count() * 2,
            body.matches("```").count()
        );
    }

    #[test]
    fn a_control_character_inside_a_secret_does_not_let_it_through() {
        let summary = "key sk-1234\u{0}5678 and sk-1234\u{1b}[0m5678";
        let steps = [step(
            Role::Developer,
            Verdict::Approved,
            NextStep::Done,
            summary,
        )];

        let body = body("task-001", "task", &steps, &["sk-12345678"]);
        let title = title("task-001", "sk-1234\u{202e}5678", &["sk-12345678"]);

        assert!(body.contains("key *** and ***"), "{body}");
        assert_eq!(title, "***");
    }

    #[test]
    fn only_a_github_pull_request_page_is_opened() {
        let page = |number, url: &str| {
            PullRequest {
                number,
                url: url.into(),
            }
            .page()
            .map(str::to_string)
        };

        assert_eq!(
            page(7, "https://github.com/lisa/app/pull/7").as_deref(),
            Some("https://github.com/lisa/app/pull/7")
        );
        for wrong in [
            "file:///tmp/x.exe",
            "https://github.com/lisa/app/pull/8",
            "https://github.com.evil.example/lisa/app/pull/7",
            "https://github.com/lisa/app/pull/7\u{1b}[2J",
            "https://github.com/lisa/../pull/7",
            "https://github.com/lisa/app/pull/7/files",
        ] {
            assert_eq!(page(7, wrong), None, "{wrong}");
        }
    }

    #[test]
    fn state_json_without_a_pull_request_still_reads() {
        let old = r#"{"task_id":"task-001","round":1,"max_rounds":5,"stage":"done"}"#;
        let state: crate::task::TaskState = serde_json::from_str(old).unwrap();
        assert_eq!(state.pull_request, None);
        // And none is written while there is none.
        assert!(!serde_json::to_string(&state)
            .unwrap()
            .contains("pull_request"));
    }
}
