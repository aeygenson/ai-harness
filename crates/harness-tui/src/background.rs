//! Work that runs in background threads: checking and installing agents, and
//! [`App::tick`], which takes whatever those threads sent since the last look.
//!
//! Each job hands its answer back through a channel (`mpsc`), so the screen
//! never waits for an agent, a download or a server.

use std::sync::mpsc;

use crate::agents_tab::{self, JobEvent};
use crate::{App, Tab};

/// What came from a background thread since the last look.
#[derive(Debug, PartialEq)]
enum Arrived<T> {
    /// Nothing yet: the thread is still working (or no job is running).
    Nothing,
    /// The thread's answer.
    Answer(T),
    /// The thread ended without answering, for example because it panicked.
    Stopped,
}

impl<T> Arrived<T> {
    /// The answer, or `stopped` when the thread ended without one; `None` while
    /// it still works.
    fn answer_or(self, stopped: T) -> Option<T> {
        match self {
            Arrived::Nothing => None,
            Arrived::Answer(answer) => Some(answer),
            Arrived::Stopped => Some(stopped),
        }
    }
}

/// Looks without waiting whether the job behind `rx` has answered.
fn look<T>(rx: Option<&mpsc::Receiver<T>>) -> Arrived<T> {
    let Some(rx) = rx else {
        return Arrived::Nothing;
    };
    match rx.try_recv() {
        Ok(answer) => Arrived::Answer(answer),
        Err(mpsc::TryRecvError::Empty) => Arrived::Nothing,
        Err(mpsc::TryRecvError::Disconnected) => Arrived::Stopped,
    }
}

impl App {
    /// Looks in the background which agents are installed on this computer.
    pub(crate) fn check_agents(&mut self) {
        if self.agent_check.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let checker = self.agent_checker;
        let credentials = self.home.as_ref().map(|h| h.join("credentials"));
        std::thread::spawn(move || {
            let _ = tx.send(checker(credentials.as_deref()));
        });
        self.agents.checking = true;
        self.agent_check = Some(rx);
    }

    /// Runs a confirmed install or update in the background; its lines come in `tick`.
    pub(crate) fn run_agent_command(
        &mut self,
        name: &'static str,
        action: harness_agents::catalog::Action,
        command: String,
    ) {
        if self.install_events.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (installer, line) = (self.installer, command.clone());
        std::thread::spawn(move || installer(&line, tx));
        self.agents.job = Some(agents_tab::Job {
            name,
            action,
            command,
            lines: Vec::new(),
            done: None,
        });
        self.install_events = Some(rx);
        self.message = Some((self.tr.f("agents.job_running", &[("name", &name)]), false));
    }

    /// Takes the lines of a running install or update, and its end.
    fn take_install_events(&mut self) {
        use harness_agents::catalog::Action;
        let mut finished = None;
        if let Some(rx) = &self.install_events {
            loop {
                match rx.try_recv() {
                    Ok(JobEvent::Line(line)) => {
                        if let Some(job) = &mut self.agents.job {
                            job.push(line);
                        }
                    }
                    Ok(JobEvent::Done(result)) => {
                        finished = Some(result);
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = Some(Err("the command stopped".into()));
                        break;
                    }
                }
            }
        }
        let Some(result) = finished else {
            return;
        };
        self.install_events = None;
        if let Some(job) = &mut self.agents.job {
            let key = match (&result, job.action) {
                (Err(_), _) => "agents.job_failed",
                (Ok(()), Action::Install) => "agents.job_installed",
                (Ok(()), Action::Update) => "agents.job_updated",
                (Ok(()), Action::Remove) => "agents.job_removed",
            };
            self.message = Some((self.tr.f(key, &[("name", &job.name)]), result.is_err()));
            job.done = Some(result);
        }
        // See what is installed now.
        self.check_agents();
    }

    /// Takes what the background work sent.
    pub(crate) fn tick(&mut self) {
        self.take_plugin_job();
        self.take_install_events();
        self.take_agent_check();
        self.take_models();
        self.take_mcp_check();
        self.take_registry_search();
        self.take_mcp_sign_in();
        if let Some(tasks) = &mut self.tasks {
            if let Some(message) = tasks.tick(&self.tr) {
                self.message = Some(message);
            }
        }
        if let Some(retro) = &mut self.retro {
            if let Some(message) = retro.tick(&self.tr) {
                self.message = Some(message);
                // A failed attempt is committed too; the agent may have used skills.
                self.reload_skills(None);
            }
        }
    }

    /// A plugin download or update has finished.
    fn take_plugin_job(&mut self) {
        let done = match look(self.plugin_job.as_ref()) {
            Arrived::Nothing => return,
            Arrived::Answer(done) => Some(done),
            Arrived::Stopped => None,
        };
        self.plugin_job = None;
        if let Some(plugins) = &mut self.plugins {
            plugins.busy = None;
        }
        match done {
            Some(done) => self.plugin_job_done(done),
            None => self.message = Some(("the download stopped".into(), true)),
        }
    }

    /// The check of which agents are installed has finished.
    fn take_agent_check(&mut self) {
        // An empty list means the check stopped (the agent catalog is never empty).
        let Some(statuses) = look(self.agent_check.as_ref()).answer_or(Vec::new()) else {
            return;
        };
        self.agent_check = None;
        let failed = statuses.is_empty();
        self.agents.checked(statuses);
        if let Some(roles) = &mut self.roles {
            roles.set_ready(self.agents.ready());
        }
        if failed {
            self.message = Some((self.tr.t("agents.check_stopped").to_string(), true));
        } else if self.tab == Tab::Agents && self.agents.job.is_none() {
            let installed = self.agents.statuses.iter().filter(|s| s.installed());
            let text = self.tr.f(
                "agents.checked",
                &[
                    ("count", &installed.count()),
                    ("all", &self.agents.statuses.len()),
                ],
            );
            self.message = Some((text, false));
        }
    }

    /// The agents have said which models they offer.
    fn take_models(&mut self) {
        let Some(answers) = look(self.asking.as_ref()).answer_or(Vec::new()) else {
            return;
        };
        self.asking = None;
        self.models_answered(answers);
    }

    /// An MCP server has listed its tools (or failed to start).
    fn take_mcp_check(&mut self) {
        let rx = self.checking.as_ref().map(|(_, _, rx)| rx);
        let Some(answer) = look(rx).answer_or(Err("the check stopped".into())) else {
            return;
        };
        if let Some((name, server, _)) = self.checking.take() {
            self.mcp_checked(&name, &server, answer);
        }
    }

    /// The search in the MCP registry has answered.
    fn take_registry_search(&mut self) {
        let stopped = Err("the search stopped".into());
        let Some(answer) = look(self.searching.as_ref()).answer_or(stopped) else {
            return;
        };
        self.searching = None;
        if let Some(catalog) = self.mcp.as_mut().and_then(|m| m.catalog.as_mut()) {
            catalog.found(answer);
        }
    }

    /// The browser sign-in to an MCP server has finished.
    fn take_mcp_sign_in(&mut self) {
        let rx = self.signing.as_ref().map(|(_, rx)| rx);
        let Some(answer) = look(rx).answer_or(Err("the sign-in stopped".into())) else {
            return;
        };
        let Some((name, _)) = self.signing.take() else {
            return;
        };
        if let Some(mcp) = &mut self.mcp {
            mcp.signing = None;
        }
        self.message = Some(match answer {
            Ok(()) => (self.tr.f("mcp.signed_in_as", &[("name", &name)]), false),
            Err(error) => (error, true),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_tells_waiting_answered_and_stopped_apart() {
        let (tx, rx) = mpsc::channel();
        assert_eq!(look::<u8>(None), Arrived::Nothing);
        assert_eq!(look(Some(&rx)), Arrived::Nothing);

        tx.send(7).unwrap();
        assert_eq!(look(Some(&rx)), Arrived::Answer(7));

        drop(tx);
        assert_eq!(look(Some(&rx)), Arrived::Stopped);
        assert_eq!(Arrived::Stopped.answer_or(0), Some(0));
        assert_eq!(Arrived::<u8>::Nothing.answer_or(0), None);
    }
}
