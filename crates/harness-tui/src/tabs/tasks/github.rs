//! The GitHub steps of a done task: sending it (its pull request) and
//! merging that pull request, and the bottom line's message about them.

use harness_agents::github::Folder;
use harness_core::config::Config;
use harness_core::git::Repo;
use harness_core::task::pull_request::PullRequest;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState};
use harness_core::text;

use super::runner::Outcome;
use crate::ui::i18n::I18n;
use crate::ui::message::Message;

/// How many characters of an error go into the bottom line.
const ERROR_CHARS: usize = 300;

/// Sends a done task to GitHub when the project is there (see
/// `harness_agents::github::publish`); `None` when nothing was sent.
pub(super) fn send_to_github(
    repo: &Repo,
    config: &Config,
    store: &TaskStore,
    state: &mut TaskState,
) -> Option<Result<PullRequest, String>> {
    if state.stage != Stage::Done {
        return None;
    }
    harness_agents::github::publish(repo, config, store, state).transpose()
}

/// The work when no role runs: merge the task's pull request, or send the
/// done task to GitHub.
pub(super) fn without_roles(
    merging: bool,
    repo: &Repo,
    config: &Config,
    store: &TaskStore,
    state: &mut TaskState,
) -> Outcome {
    let (published, merged) = if merging {
        (
            None,
            Some(harness_agents::github::merge(repo, config, state)),
        )
    } else {
        (send_to_github(repo, config, store, state), None)
    };
    Outcome {
        task: state.task_id.clone(),
        stop: None,
        published,
        merged,
    }
}

/// The message about sending or merging, when the work did one of them.
pub(super) fn github_text(outcome: &Outcome, tr: &I18n) -> Option<Message> {
    let task = &outcome.task;
    // Errors and file names may hold what an agent or GitHub wrote.
    let error = |e: &str| text::safe_line(e, ERROR_CHARS);
    if let Some(published) = &outcome.published {
        return Some(match published {
            Ok(pull_request) => Message::info(tr.f(
                "tasks.published",
                &[("task", task), ("url", &pull_request.url)],
            )),
            Err(e) => Message::error(tr.f(
                "tasks.publish_failed",
                &[("task", task), ("error", &error(e))],
            )),
        });
    }
    let merged = match outcome.merged.as_ref()? {
        Ok(merged) => merged,
        Err(e) => {
            return Some(Message::error(tr.f(
                "tasks.merge_failed",
                &[("task", task), ("error", &error(e))],
            )))
        }
    };
    let number = merged.number.to_string();
    let base = &merged.base;
    Some(match &merged.folder {
        Folder::Updated => Message::info(tr.f(
            "tasks.merged",
            &[("task", task), ("number", &number), ("base", base)],
        )),
        Folder::Uncommitted(files) => Message::error(tr.f(
            "tasks.merged_uncommitted",
            &[
                ("task", task),
                ("number", &number),
                // File names can be anything an agent created.
                ("files", &error(&files.join(", "))),
            ],
        )),
        Folder::Failed(e) => Message::error(tr.f(
            "tasks.merged_not_updated",
            &[
                ("task", task),
                ("number", &number),
                ("base", base),
                ("error", &error(e)),
            ],
        )),
    })
}

#[cfg(test)]
mod tests {
    use harness_agents::github::Merged;

    use super::*;

    fn merged(folder: Folder) -> Outcome {
        Outcome {
            task: "task-001".into(),
            stop: None,
            published: None,
            merged: Some(Ok(Merged {
                number: 7,
                base: "main".into(),
                folder,
            })),
        }
    }

    #[test]
    fn every_way_a_merge_ends_has_its_message() {
        let tr = I18n::load(None);

        let done = github_text(&merged(Folder::Updated), &tr).unwrap();
        assert!(
            done.text.contains("#7 merged; the project is on main"),
            "{}",
            done.text
        );

        let files = format!("evil\u{1b}[2J{}", "x".repeat(400));
        let kept = github_text(&merged(Folder::Uncommitted(vec![files])), &tr).unwrap();
        assert!(
            kept.text.contains("uncommitted changes in evilxxx"),
            "{}",
            kept.text
        );
        assert!(kept.text.chars().count() < 500, "{}", kept.text);

        let failed = github_text(&merged(Folder::Failed("offline".into())), &tr).unwrap();
        assert!(
            failed.text.contains("main here was not updated: offline"),
            "{}",
            failed.text
        );

        let mut refused = merged(Folder::Updated);
        refused.merged = Some(Err("checks failed".into()));
        let refused = github_text(&refused, &tr).unwrap();
        assert!(
            refused.text.contains("merging stopped: checks failed"),
            "{}",
            refused.text
        );

        // Work that neither sent nor merged has no GitHub message.
        let mut nothing = merged(Folder::Updated);
        nothing.merged = None;
        assert!(github_text(&nothing, &tr).is_none());
    }
}
