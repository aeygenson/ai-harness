//! What the buttons do: one place for every `ButtonId`, whether it was clicked
//! or chosen with a key. A tab's own buttons go on to that tab's `press`.

use harness_agents::install::credentials;

use crate::tabs::projects::has_config;
use crate::tabs::projects::picker::FolderButton;
use crate::tabs::roles::Action;
use crate::ui::message::Message;
use crate::ui::{i18n, theme};
use crate::ui::{ButtonId, Form};
use crate::{App, Pick, Purpose};

impl App {
    /// A button, clicked or chosen with its key.
    pub(crate) fn press(&mut self, id: ButtonId) {
        match id {
            ButtonId::AgentsCheck => self.check_agents(),
            ButtonId::AgentRun => self.ask_to_run_agent_command(false),
            ButtonId::AgentRemove => self.ask_to_run_agent_command(true),
            ButtonId::AgentSignIn => self.sign_in_agent(),
            ButtonId::HarnessUpdate => self.ask_to_update_harness(),
            ButtonId::RefreshModels => self.ask_for_models(),
            ButtonId::Mcp(button) => {
                if let (Some(mcp), Some(roles)) = (&mut self.mcp, &mut self.roles) {
                    let action = mcp.press(button, roles);
                    self.mcp_action(action);
                }
            }
            ButtonId::McpCatalog(button) => {
                if let Some(mcp) = &mut self.mcp {
                    let action = mcp.catalog_press(button);
                    self.mcp_action(action);
                }
            }
            ButtonId::Plugin(button) => {
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &mut self.roles) {
                    let action = plugins.press(button, roles);
                    self.plugin_action(action);
                }
            }
            ButtonId::PluginCatalog(button) => {
                if let Some(plugins) = &mut self.plugins {
                    let action = plugins.catalog_press(button);
                    self.plugin_action(action);
                }
            }
            ButtonId::Retro(button) => {
                if let Some(retro) = &mut self.retro {
                    let action = retro.press(button);
                    self.retro_action(action);
                }
            }
            ButtonId::Skill(button) => {
                if let Some(skills) = &mut self.skills {
                    let action = skills.press(button);
                    self.skill_action(action);
                }
            }
            ButtonId::TaskZoom(zoom) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.toggle_zoom(zoom);
                }
            }
            ButtonId::TaskFile(index) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.open_file(index);
                }
            }
            ButtonId::Input => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.focus_input();
                }
            }
            ButtonId::Menu(menu) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.toggle_menu(menu);
                }
            }
            ButtonId::Send => self.send_message(),
            ButtonId::UseProject => self.use_project(),
            ButtonId::NewProject => self.pick(Pick::NewProject),
            ButtonId::OpenFolder => self.pick(Pick::Open),
            ButtonId::Folder(FolderButton::Choose) => {
                if let Some((pick, browser)) = self.browser.take() {
                    self.picked(pick, browser.chosen());
                }
            }
            ButtonId::Folder(button) => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.press(button, &self.tr);
                }
            }
            ButtonId::Theme => self.next_theme(),
            ButtonId::Language => self.next_language(),
            ButtonId::RemoveProject => self.ask_to_remove_project(),
            ButtonId::Save => self.save_roles(),
            ButtonId::Undo => {
                if let Some(roles) = &mut self.roles {
                    roles.undo();
                    self.message = None;
                }
            }
            ButtonId::Cancel => self.browser = None,
            ButtonId::Ok => {}
        }
    }

    /// Sends the message of the Tasks tab; not while a retrospective is being written.
    fn send_message(&mut self) {
        if self.generating() {
            self.message = Some(Message::error(self.tr.t("retro.busy")));
            return;
        }
        if let Some(tasks) = &mut self.tasks {
            self.message = Some(match tasks.send(self.builder, &self.tr) {
                Ok(text) => Message::info(text),
                Err(error) => Message::error(error),
            });
        }
    }

    /// Starts signing in to the agent selected on the Agents tab.
    fn sign_in_agent(&mut self) {
        if let Some(status) = self.agents.sign_in_target() {
            let entry = status.entry;
            self.sign_in = Some((credentials::login_name(entry.id), entry.name));
        }
    }

    /// Opens the chosen project, offers to set it up, or says the folder is gone.
    fn use_project(&mut self) {
        if let Some(project) = self.projects.current() {
            let path = project.path.clone();
            if has_config(&path) {
                self.open(&path);
            } else if path.is_dir() {
                self.form = Some(self.init_form(&path));
            } else {
                let text = self.tr.f("projects.gone", &[("path", &path.display())]);
                self.message = Some(Message::error(text));
            }
        }
    }

    /// Switches to the next colour theme and remembers it.
    fn next_theme(&mut self) {
        theme::next();
        if let Some(home) = &self.home {
            if let Err(error) = i18n::save_setting(home, "theme", theme::current().code) {
                self.message = Some(Message::error(error));
            }
        }
    }

    /// Switches to the next language and remembers it.
    fn next_language(&mut self) {
        self.tr.next();
        // The last message was in the old language.
        self.message = None;
        if let Some(home) = &self.home {
            if let Err(error) = self.tr.save(home) {
                self.message = Some(Message::error(error));
            }
        }
    }

    /// Asks whether to remove the chosen project from the list; not while it is busy.
    fn ask_to_remove_project(&mut self) {
        if self.project.as_deref() == self.projects.current().map(|p| p.path.as_path())
            && self.busy()
        {
            return;
        }
        if let Some(project) = self.projects.current() {
            let tr = &self.tr;
            let text = tr.f(
                "form.remove_text",
                &[("name", &project.name), ("path", &project.path.display())],
            );
            self.form = Some((
                Purpose::Remove(project.path.clone()),
                Form::new(tr.t("form.remove_title"), &text, tr.t("form.remove")),
            ));
        }
    }

    /// Saves the Roles tab and reloads the tabs that show its agents and skills.
    fn save_roles(&mut self) {
        if let Some(roles) = &mut self.roles {
            self.message = Some(match roles.save() {
                Ok(()) => Message::info(self.tr.t("roles.saved")),
                Err(error) => Message::error(error),
            });
            // The Tasks and Skills tabs show the agents and skills too.
            if let Some(tasks) = &mut self.tasks {
                tasks.reload();
            }
            if let Some(skills) = &mut self.skills {
                skills.reload();
            }
        }
    }

    /// What the Roles tab asks for after a click or a key.
    pub(crate) fn act(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Say(text) => self.message = Some(Message::info(text)),
            Action::EditModel(model) => {
                let tr = &self.tr;
                self.form = Some((
                    Purpose::Model,
                    Form::new(
                        tr.t("roles.model_title"),
                        tr.t("roles.model_text"),
                        tr.t("roles.model_ok"),
                    )
                    .field(tr.t("roles.model_field"), &model),
                ));
            }
        }
    }
}
