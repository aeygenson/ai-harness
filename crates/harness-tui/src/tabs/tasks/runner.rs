//! Runs roles in the background, so the TUI keeps working while an agent
//! does. The same core functions as `harness task new`, `harness approve` and
//! `harness run`, in a thread with its own small tokio runtime. What the agent
//! prints comes back line by line for the live log.
//!
//! The user's stop buttons reach the run through a [`StopRequest`] the screen and
//! the run share (see `harness_core::task::stop_request`): the run loop reads
//! it before each role, the agent's process while the agent works. The work
//! holds the project's run lock (`harness_core::task::run_lock`), so a
//! `harness run` in a terminal cannot work in the same folder at once, and
//! `harness stop` from a terminal reaches this run too.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use harness_agents::build::BuildError;
use harness_agents::github::{Merged, Waiting, MERGE_WAIT_LIMIT, MERGE_WAIT_PAUSE};
use harness_agents::process;
use harness_agents::Team;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::skills::Skills;
use harness_core::task::branch;
use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{self, HumanDecision, Project, SameText, StopReason};
use harness_core::task::pull_request::PullRequest;
use harness_core::task::run_lock::RunLock;
use harness_core::task::stop_request::StopRequest;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState};
use tokio::sync::oneshot::{self, error::TryRecvError};

use super::agent_output::readable;
use super::github::{send_to_github, without_roles};
use super::live_log::LiveLog;

/// Builds the agents of the project: `build_team`, or mock agents in tests.
pub type Builder = fn(&Config, &Path) -> Result<Team, BuildError>;

/// What the user sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A new task with this description; the architect starts.
    New(String),
    /// The user's decision on a waiting task; then the roles run, unless it is done.
    Decide {
        task: String,
        /// Where the task stood when the user decided. The list is read from the
        /// branch checked out, which may hold an old copy of a task that has
        /// moved on in its own branch; then the decision is refused.
        seen: Stage,
        verdict: Verdict,
        next: NextStep,
        notes: String,
        /// Rounds added to the task's limit before the decision; 0 for none.
        extra_rounds: u32,
    },
    /// A role stopped part-way (a limit, a failure): run the task again.
    Continue(String),
    /// Send the done task to GitHub again (the last try failed).
    Publish(String),
    /// Merge the done task's pull request and bring the work into the folder.
    Merge(String),
}

/// The model and level the user chose for one role for this launch only;
/// `harness.toml` stays as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunChoice {
    pub role: Role,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// How the work ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub task: String,
    /// Why the roles stopped; `None` when nothing had to run.
    pub stop: Option<StopReason>,
    /// How sending the done task to GitHub went; `None` when nothing was
    /// sent (not done, or the project is not on GitHub).
    pub published: Option<Result<PullRequest, String>>,
    /// How merging the pull request went; `None` when it was not asked for.
    pub merged: Option<Result<Merged, String>>,
}

#[derive(Debug)]
enum Event {
    /// The task being worked on; a new task gets its id here.
    Started(String),
    /// The task's branch was checked out.
    SwitchedBranch,
    Finished(Result<Outcome, String>),
}

/// A thread that runs agents. Dropping it (the TUI quits, the project is
/// removed) stops the agents with everything they started, then waits for the
/// thread, so no agent keeps working after the harness is gone.
#[derive(Debug)]
pub struct Background {
    /// Dropping this sender is the stop signal (see [`run_until_stopped`]).
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Background {
    /// Starts `job` on a new thread. `job` gets the stop signal and passes it
    /// to [`run_until_stopped`].
    pub fn start(job: impl FnOnce(oneshot::Receiver<()>) + Send + 'static) -> Self {
        let (stop, stop_rx) = oneshot::channel();
        let thread = thread::spawn(move || job(stop_rx));
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        // `take` moves the sender out, and it is dropped right here.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs `work` on a small tokio runtime of its own until it ends or `stop`
/// fires. Stopping drops `work`, and with it the running agent, which stops
/// everything the agent started (`harness_agents::process::run`).
pub fn run_until_stopped<F: Future>(
    work: F,
    stop: oneshot::Receiver<()>,
) -> Result<F::Output, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    runtime.block_on(async {
        // `select!` waits for whichever finishes first and drops the other.
        // The stop signal finishes when its sender is dropped.
        tokio::select! {
            output = work => Ok(output),
            _ = stop => Err("stopped".to_string()),
        }
    })
}

/// What one background run is asked to do.
struct Job {
    root: PathBuf,
    request: Request,
    choice: Option<RunChoice>,
    builder: Builder,
    /// The user's stop buttons, shared with the screen.
    stop: StopRequest,
}

/// Does `request` run roles? Finishing a task, sending it to GitHub and
/// merging it run none.
fn runs_roles(request: &Request) -> bool {
    !matches!(
        request,
        Request::Decide {
            next: NextStep::Done,
            ..
        } | Request::Publish(_)
            | Request::Merge(_)
    )
}

/// Work in the background.
#[derive(Debug)]
pub struct Running {
    events: Receiver<Event>,
    log: Receiver<String>,
    /// The task, once known.
    pub task: Option<String>,
    /// The work checked out the task's branch; reset by whoever reacts.
    pub switched_branch: bool,
    /// The user's stop buttons for this run; `None` when it runs no roles.
    pub stop: Option<StopRequest>,
    /// When the user started the run: its first role starts then.
    pub started: SystemTime,
    /// Kept to stop the agents when this is dropped.
    _background: Background,
}

impl Running {
    /// Starts `request` on a background thread; [`Running::poll`] collects the results.
    pub fn start(
        root: &Path,
        request: Request,
        choice: Option<RunChoice>,
        builder: Builder,
    ) -> Self {
        let (events, events_rx) = channel();
        let (log, log_rx) = channel();
        let stop = StopRequest::new();
        let buttons = runs_roles(&request).then(|| stop.clone());
        let job = Job {
            root: root.to_path_buf(),
            request,
            choice,
            builder,
            stop,
        };
        let background = Background::start(move |stop| {
            process::set_live_log(Some(log));
            let result = work(job, &events, stop);
            process::set_live_log(None);
            let _ = events.send(Event::Finished(result));
        });
        Self {
            events: events_rx,
            log: log_rx,
            task: None,
            switched_branch: false,
            stop: buttons,
            started: SystemTime::now(),
            _background: background,
        }
    }

    /// Takes what arrived: the agent's lines go to `log` (the latest
    /// [`LOG_LINES`] stay). `Some` once the work is over.
    pub fn poll(&mut self, log: &mut LiveLog) -> Option<Result<Outcome, String>> {
        for line in self.log.try_iter() {
            if let Some(line) = readable(&line) {
                log.push(line);
            }
        }
        for event in self.events.try_iter() {
            match event {
                Event::Started(task) => self.task = Some(task),
                Event::SwitchedBranch => self.switched_branch = true,
                Event::Finished(result) => return Some(result),
            }
        }
        None
    }
}

fn work(job: Job, events: &Sender<Event>, stop: oneshot::Receiver<()>) -> Result<Outcome, String> {
    let Job {
        root,
        request,
        choice,
        builder,
        stop: stop_request,
    } = job;
    let root = root.as_path();
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    // Held until the work ends, before anything is switched or saved.
    let mut lock = RunLock::hold(&repo).map_err(|e| e.to_string())?;
    if let Some(task) = task_of(&request) {
        lock.set_task(task).map_err(|e| e.to_string())?;
    }
    // An existing task's branch first: its settings are the ones to use, and
    // the user's decision and the roles' work are committed there. A merge stays
    // where the user is: it changes the folder only once everything is checked,
    // and then it moves to the main branch.
    if let Request::Decide { task, .. } | Request::Continue(task) | Request::Publish(task) =
        &request
    {
        if branch::enter(&repo, task).map_err(|e| e.to_string())? {
            let _ = events.send(Event::SwitchedBranch);
        }
    }
    let harness_dir = root.join(HARNESS_DIR);
    let config = load_config(&harness_dir, choice)?;
    let merging = matches!(request, Request::Merge(_));
    // Everything the agents need is checked before anything is saved: a
    // missing login leaves the task as it was.
    let agents = if runs_roles(&request) {
        let team = builder(&config, root).map_err(|e| e.to_string())?;
        let skills = Skills::load(&harness_dir, &config).map_err(|e| e.to_string())?;
        Some((team, skills))
    } else {
        None
    };

    let (store, mut state) = open_task(&repo, &config, request)?;
    let task = state.task_id.clone();
    lock.set_task(&task).map_err(|e| e.to_string())?;
    let _ = events.send(Event::Started(task.clone()));
    let Some((team, skills)) = agents else {
        let merge = merge_waiting(merging, stop);
        let (outcome, moved) = without_roles(merge, &repo, &config, &store, &mut state);
        if moved {
            let _ = events.send(Event::SwitchedBranch);
        }
        return Ok(outcome);
    };
    // Only work that runs roles can stop halfway, so only it gets a stop file.
    lock.connect(&stop_request).map_err(|e| e.to_string())?;
    let project = Project {
        config: &config,
        skills: &skills,
        agent_versions: team.versions(),
        stop: &stop_request,
    };
    let stop = run_until_stopped(
        orchestrator::run_project(&repo, &store, &mut state, &team, project),
        stop,
    )?
    .map_err(|e| e.to_string())?;
    let published = if stop == StopReason::Done {
        send_to_github(&repo, &config, &store, &mut state)
    } else {
        None
    };
    Ok(Outcome {
        task,
        stop: Some(stop),
        published,
        merged: None,
    })
}

/// The task the request is about; `None` for a new one, which has no id yet.
fn task_of(request: &Request) -> Option<&str> {
    match request {
        Request::New(_) => None,
        Request::Decide { task, .. }
        | Request::Continue(task)
        | Request::Publish(task)
        | Request::Merge(task) => Some(task),
    }
}

/// The task the request is about: a new one made, or the saved one opened
/// (with the user's decision recorded, or a stopped task set to go on).
fn open_task(
    repo: &Repo,
    config: &Config,
    request: Request,
) -> Result<(TaskStore, TaskState), String> {
    let runs = repo.runs_dir();
    let opened = match request {
        Request::New(description) => {
            let id = branch::next_task_id(repo).map_err(|e| e.to_string())?;
            orchestrator::create_project_task(repo, config, &id, &description, SameText::Refuse)
                .map_err(|e| e.to_string())?
        }
        Request::Decide {
            task,
            seen,
            verdict,
            next,
            notes,
            extra_rounds,
        } => {
            let (store, mut state) = TaskStore::open(&runs, &task).map_err(|e| e.to_string())?;
            if state.stage != seen {
                return Err(format!(
                    "{task} has moved on in its branch {}; look at it again and decide",
                    branch::task_branch(&task)
                ));
            }
            // The new limit is saved first, so the decision's commit carries it.
            if extra_rounds > 0 {
                store
                    .add_rounds(&mut state, extra_rounds)
                    .map_err(|e| e.to_string())?;
            }
            orchestrator::record_human_decision(
                repo,
                &store,
                &mut state,
                HumanDecision {
                    verdict,
                    next,
                    notes: &notes,
                },
            )
            .map_err(|e| e.to_string())?;
            (store, state)
        }
        Request::Continue(task) => {
            let (store, mut state) = TaskStore::open(&runs, &task).map_err(|e| e.to_string())?;
            // A task the user stopped goes on with the role they stopped.
            orchestrator::continue_stopped(repo, &store, &mut state).map_err(|e| e.to_string())?;
            (store, state)
        }
        Request::Publish(task) | Request::Merge(task) => {
            TaskStore::open(&runs, &task).map_err(|e| e.to_string())?
        }
    };
    Ok(opened)
}

/// How «Merge pull request» waits for GitHub in the TUI, when `merging`:
/// up to [`MERGE_WAIT_LIMIT`], saying in the live log what it waits for, and
/// only until `stop` fires (the TUI quits).
fn merge_waiting(merging: bool, mut stop: oneshot::Receiver<()>) -> Option<Waiting<'static>> {
    merging.then(|| Waiting {
        limit: MERGE_WAIT_LIMIT,
        pause: MERGE_WAIT_PAUSE,
        // `Empty`: the sender is still there, so nobody asked to stop. It is
        // dropped (`Closed`) to stop.
        stopped: Box::new(move || !matches!(stop.try_recv(), Err(TryRecvError::Empty))),
        note: Box::new(process::note_live),
    })
}

/// `harness.toml` with the model and level the user chose for this launch.
fn load_config(harness_dir: &Path, choice: Option<RunChoice>) -> Result<Config, String> {
    let mut config = Config::load(harness_dir).map_err(|e| e.to_string())?;
    if let Some(choice) = choice {
        if let Some(settings) = config.roles.get_mut(&choice.role) {
            settings.model = choice.model;
            settings.effort = choice.effort;
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_a_background_job_stops_its_work_and_waits() {
        let (sender, results) = channel();
        // Work that would never end by itself, like an agent that hangs.
        let background = Background::start(move |stop| {
            let result = run_until_stopped(std::future::pending::<()>(), stop);
            sender.send(result).unwrap();
        });

        drop(background);

        // `drop` waited for the thread, so its answer is already there.
        assert_eq!(results.try_recv().unwrap(), Err("stopped".to_string()));
    }

    #[test]
    fn work_that_ends_by_itself_gives_its_result() {
        let (_stop, stop_rx) = oneshot::channel::<()>();
        assert_eq!(run_until_stopped(async { 42 }, stop_rx), Ok(42));
    }

    #[test]
    fn only_work_that_runs_roles_gets_stop_buttons() {
        let decide = |next| Request::Decide {
            task: "task-001".into(),
            seen: harness_core::task::Stage::Done,
            verdict: Verdict::Approved,
            next,
            notes: String::new(),
            extra_rounds: 0,
        };
        assert!(runs_roles(&Request::New("Do it".into())));
        assert!(runs_roles(&Request::Continue("task-001".into())));
        assert!(runs_roles(&decide(NextStep::To(Role::Developer))));
        assert!(!runs_roles(&decide(NextStep::Done)));
        assert!(!runs_roles(&Request::Publish("task-001".into())));
        assert!(!runs_roles(&Request::Merge("task-001".into())));
    }
}
