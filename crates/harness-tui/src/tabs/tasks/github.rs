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
                ("files", &files.join(", ")),
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
