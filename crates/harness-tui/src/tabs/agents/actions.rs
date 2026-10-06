//! The Agents tab's work: signing in, installing, updating and removing agents,
//! and asking them which models they offer.

use std::sync::mpsc;

use harness_agents::install::credentials;
use harness_core::models::{self};

use crate::ui::message::Message;
use crate::ui::Form;
use crate::{Answers, App, Purpose};

impl App {
    /// Back from `harness login`: say how it went and look at the logins again.
    pub(crate) fn finish_sign_in(&mut self, name: &str, result: Result<(), String>) {
        self.message = Some(match result {
            Ok(()) => Message::info(self.tr.f("agents.signed_in", &[("name", &name)])),
            Err(error) => Message::error(self.tr.f(
                "agents.sign_in_failed",
                &[("name", &name), ("error", &error)],
            )),
        });
        if let Some(dir) = self.home.as_ref().map(|h| h.join("credentials")) {
            self.agents.reload_logins(&dir);
        }
        if let Some(roles) = &mut self.roles {
            roles.set_ready(self.agents.ready());
        }
    }

    /// «Install», «Update» or «Remove»: first the command is shown to be confirmed.
    pub(crate) fn ask_to_run_agent_command(&mut self, remove: bool) {
        use harness_agents::install::catalog::Action;
        let Some((status, action, command)) = self.agents.next_step(remove) else {
            return;
        };
        let name = status.entry.name;
        let (title, ok, text) = match action {
            Action::Install => (
                "agents.confirm_install",
                "agents.run_install",
                "agents.confirm_text",
            ),
            Action::Update => (
                "agents.confirm_update",
                "agents.run_update",
                "agents.confirm_text",
            ),
            Action::Remove => (
                "agents.confirm_remove",
                "agents.run_remove",
                "agents.confirm_remove_text",
            ),
        };
        let tr = &self.tr;
        let mut text = tr.f(text, &[("command", &command)]);
        // Claude Code may also be what runs Claude's own sessions here.
        if action == Action::Remove && status.entry.id == "claude" {
            text = format!("{text}\n\n{}", tr.t("agents.remove_claude_note"));
        }
        if action == Action::Install && !status.entry.runs() {
            text = format!("{}\n\n{text}", tr.t("agents.confirm_not_run"));
        }
        let form = Form::new(&tr.f(title, &[("name", &name)]), &text, tr.t(ok));
        self.form = Some((
            Purpose::RunAgentCommand {
                name,
                action,
                command,
            },
            form,
        ));
    }

    /// «Refresh models»: the agents are asked in the background.
    pub(crate) fn ask_for_models(&mut self) {
        if self.asking.is_some() {
            return;
        }
        let Some(dir) = credentials::default_dir() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let asker = self.asker;
        std::thread::spawn(move || {
            let _ = tx.send(asker(&dir));
        });
        self.asking = Some(rx);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = true;
        }
        self.message = Some(Message::info(self.tr.t("roles.refreshing")));
    }

    /// The agents answered: keep the lists and say what came back.
    pub(crate) fn models_answered(&mut self, answers: Answers) {
        let mut got = Vec::new();
        let mut failed = Vec::new();
        for (agent, answer) in answers {
            let saved = answer.and_then(|list| {
                let count = list.models.len();
                match &self.home {
                    Some(home) => models::save(home, &list).map_err(|e| e.to_string()),
                    None => Err("HOME is not set".into()),
                }
                .map(|()| count)
            });
            match saved {
                Ok(count) => got.push(format!("{agent} {count}")),
                Err(error) => failed.push(format!("{agent}: {error}")),
            }
        }
        let text = match (got.is_empty(), failed.is_empty()) {
            (true, true) => Message::error(self.tr.t("roles.no_logins")),
            (_, true) => Message::info(self.tr.f("roles.refreshed", &[("lists", &got.join(", "))])),
            _ => {
                let mut text = failed.join("; ");
                if !got.is_empty() {
                    text = format!(
                        "{}; {text}",
                        self.tr.f("roles.refreshed", &[("lists", &got.join(", "))])
                    );
                }
                Message::error(text)
            }
        };
        self.message = Some(text);
        if let Some(roles) = &mut self.roles {
            roles.refreshing = false;
            roles.reload_models();
        }
    }
}
