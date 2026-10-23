//! How long a task and its steps took, in plain words («12 min», «2 h 05 min»).
//!
//! The times come from git, not from a new file: the harness commits a new
//! task (`task.md`) and then every step (`handoff.json` in the step folder),
//! so the time of the commit that added a file is when that happened. Commit
//! times stay right after a branch switch or a fresh clone, unlike the times
//! of the files on disk. One `git log` reads them for all tasks, and only
//! when a task has a step whose time is not known yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_core::git::{Repo, HARNESS_DIR};

use super::tab::{TaskView, TasksTab};
use crate::ui::i18n::I18n;

/// When a task was created and when each of its steps was committed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TaskTimes {
    /// The commit of the new task; `None` if it is not committed.
    created: Option<SystemTime>,
    /// The commit of each step, in the order of the steps.
    steps: Vec<Option<SystemTime>>,
}

impl TaskTimes {
    /// Every time is known: there is nothing more to read for this task.
    fn complete(&self, steps: usize) -> bool {
        self.steps.len() == steps
            && self.created.is_some()
            && self.steps.iter().all(Option::is_some)
    }

    /// How long the task has run: until `now`, or until its last step once it is done.
    pub(super) fn task_took(&self, done: bool, now: SystemTime) -> Option<Duration> {
        let created = self.created?;
        let end = if done { self.last()? } else { now };
        Some(between(created, end))
    }

    /// How long step `index` took: from the step before it (or the new task) to its commit.
    pub(super) fn step_took(&self, index: usize) -> Option<Duration> {
        let end = (*self.steps.get(index)?)?;
        let start = match index.checked_sub(1) {
            Some(before) => (*self.steps.get(before)?)?,
            None => self.created?,
        };
        Some(between(start, end))
    }

    /// The latest known time of the task: its last step, or its creation.
    pub(super) fn last(&self) -> Option<SystemTime> {
        self.steps
            .iter()
            .rev()
            .flatten()
            .next()
            .copied()
            .or(self.created)
    }
}

/// `end - start`; zero when the clock went back (a commit from another computer).
pub(super) fn between(start: SystemTime, end: SystemTime) -> Duration {
    end.duration_since(start).unwrap_or(Duration::ZERO)
}

impl TasksTab {
    /// Reads the times of the tasks again, if one has a step not known yet.
    pub(super) fn refresh_times(&mut self) {
        let known = self.all.iter().all(|task| {
            self.times
                .get(&task.id)
                .is_some_and(|times| times.complete(task.steps.len()))
        });
        if known {
            return;
        }
        let added = Repo::open(&self.root)
            .map(|repo| repo.added_times(&format!("{HARNESS_DIR}/runs")))
            .unwrap_or_default();
        self.times = task_times(&self.root, &self.all, &added);
    }

    /// The times of the task `id`, if they were read.
    pub(super) fn times_of(&self, id: &str) -> Option<&TaskTimes> {
        self.times.get(id)
    }
}

/// The times of `tasks` from `added`: files with the time their commit added
/// them (see `Repo::added_times`), paths relative to `root`.
fn task_times(
    root: &Path,
    tasks: &[TaskView],
    added: &[(String, u64)],
) -> HashMap<String, TaskTimes> {
    // `added` lists the newest commits first; the oldest time of a file
    // (added, deleted, added again) wins because it is inserted last.
    let mut when: HashMap<PathBuf, SystemTime> = HashMap::with_capacity(added.len());
    for (path, seconds) in added {
        when.insert(root.join(path), UNIX_EPOCH + Duration::from_secs(*seconds));
    }
    let runs = root.join(HARNESS_DIR).join("runs");
    tasks
        .iter()
        .map(|task| {
            let times = TaskTimes {
                created: when.get(&runs.join(&task.id).join("task.md")).copied(),
                steps: task
                    .steps
                    .iter()
                    .map(|step| {
                        when.get(&root.join(&step.dir).join("handoff.json"))
                            .copied()
                    })
                    .collect(),
            };
            (task.id.clone(), times)
        })
        .collect()
}

/// A duration in plain words: «45 s», «12 min» or «2 h 05 min».
pub(super) fn duration_label(took: Duration, tr: &I18n) -> String {
    let seconds = took.as_secs();
    let minutes = seconds / 60;
    if minutes == 0 {
        return tr.f("tasks.time_seconds", &[("count", &seconds)]);
    }
    if minutes < 60 {
        return tr.f("tasks.time_minutes", &[("count", &minutes)]);
    }
    // Minutes with two digits, so «2 h 05 min» reads like a clock.
    let rest = format!("{:02}", minutes % 60);
    tr.f(
        "tasks.time_hours",
        &[("hours", &(minutes / 60)), ("minutes", &rest)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn label(seconds: u64, tr: &I18n) -> String {
        duration_label(Duration::from_secs(seconds), tr)
    }

    #[test]
    fn durations_read_in_seconds_minutes_or_hours() {
        let tr = I18n::load(None);
        assert_eq!(label(0, &tr), "0 s");
        assert_eq!(label(59, &tr), "59 s");
        assert_eq!(label(60, &tr), "1 min");
        assert_eq!(label(3_599, &tr), "59 min");
        assert_eq!(label(3_600, &tr), "1 h 00 min");
        assert_eq!(label(2 * 3_600 + 5 * 60 + 59, &tr), "2 h 05 min");
        assert_eq!(label(30 * 3_600, &tr), "30 h 00 min");
        // Parts of a second do not count.
        assert_eq!(duration_label(Duration::from_millis(1_999), &tr), "1 s");
    }

    #[test]
    fn durations_are_translated() {
        let mut tr = I18n::load(None);
        assert!(tr.select("ru"));
        assert_eq!(label(7, &tr), "7 с");
        assert_eq!(label(12 * 60, &tr), "12 мин");
        assert_eq!(label(3_600 + 60, &tr), "1 ч 01 мин");
    }

    #[test]
    fn a_task_runs_until_now_or_until_its_last_step() {
        let times = TaskTimes {
            created: Some(at(1_000)),
            steps: vec![Some(at(1_600)), None, Some(at(4_000))],
        };
        assert_eq!(
            times.task_took(false, at(10_000)),
            Some(Duration::from_mins(150))
        );
        assert_eq!(
            times.task_took(true, at(10_000)),
            Some(Duration::from_mins(50))
        );
        assert_eq!(times.last(), Some(at(4_000)));
        // A clock that went back gives zero, not a panic.
        assert_eq!(times.task_took(false, at(10)), Some(Duration::ZERO));
        assert_eq!(TaskTimes::default().task_took(false, at(10)), None);
    }

    #[test]
    fn a_step_takes_from_the_step_before_it_to_its_own_commit() {
        let times = TaskTimes {
            created: Some(at(1_000)),
            steps: vec![Some(at(1_600)), None, Some(at(4_000)), Some(at(4_030))],
        };
        assert_eq!(times.step_took(0), Some(Duration::from_mins(10)));
        // Without its own time or the one before it: unknown.
        assert_eq!(times.step_took(1), None);
        assert_eq!(times.step_took(2), None);
        assert_eq!(times.step_took(3), Some(Duration::from_secs(30)));
        assert_eq!(times.step_took(4), None);
        assert!(!times.complete(4));
        let done = TaskTimes {
            created: Some(at(1)),
            steps: vec![Some(at(2))],
        };
        assert!(done.complete(1));
        // A new step makes the times incomplete again.
        assert!(!done.complete(2));
    }
}
