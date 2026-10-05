//! What the buttons do: one place for every `ButtonId`, whether it was clicked
//! or chosen with a key.

use harness_agents::credentials;

use crate::projects_tab::has_config;
use crate::roles_tab::Action;
use crate::tasks::Menu;
use crate::ui::{ButtonId, Form};
use crate::{i18n, plugins_tab, theme, App, Pick, Purpose};

impl App {
    /// A button, clicked or chosen with its key.
    pub(crate) fn press(&mut self, id: ButtonId) {
        match id {
            ButtonId::AgentsCheck => self.check_agents(),
            ButtonId::AgentRun => self.ask_to_run_agent_command(false),
            ButtonId::AgentRemove => self.ask_to_run_agent_command(true),
            ButtonId::AgentSignIn => {
                if let Some(status) = self.agents.sign_in_target() {
                    let entry = status.entry;
                    self.sign_in = Some((credentials::login_name(entry.id), entry.name));
                }
            }
            ButtonId::RefreshModels => self.ask_for_models(),
            ButtonId::McpRole(index) => {
                if let Some(mcp) = &mut self.mcp {
                    mcp.choose_role(index);
                }
            }
            ButtonId::McpCheck => self.check_mcp(),
            ButtonId::McpCatalog => {
                if let Some(mcp) = &mut self.mcp {
                    let action = mcp.open_catalog();
                    self.mcp_action(action);
                }
            }
            ButtonId::McpSearch | ButtonId::McpUse | ButtonId::McpBack => {
                if let Some(mcp) = &mut self.mcp {
                    let action = mcp.catalog_press(id);
                    self.mcp_action(action);
                }
            }
            ButtonId::McpNew
            | ButtonId::McpEdit
            | ButtonId::McpRemove
            | ButtonId::McpSecret
            | ButtonId::McpSignIn => {
                if let (Some(mcp), Some(roles)) = (&self.mcp, &self.roles) {
                    let action = mcp.press(id, roles);
                    self.mcp_action(action);
                }
            }
            ButtonId::McpToggle => {
                if let (Some(mcp), Some(roles)) = (&self.mcp, &mut self.roles) {
                    if let Err(key) = mcp.toggle(roles) {
                        self.message = Some((self.tr.t(key).to_string(), true));
                    }
                }
            }
            ButtonId::PluginRole(index) => {
                if let Some(plugins) = &mut self.plugins {
                    plugins.choose_role(index);
                }
            }
            ButtonId::PluginToggle => {
                if let (Some(plugins), Some(roles)) = (&self.plugins, &mut self.roles) {
                    if let Err(key) = plugins.toggle(roles) {
                        self.message = Some((self.tr.t(key).to_string(), true));
                    }
                }
            }
            ButtonId::PluginCatalog => self.plugin_action(plugins_tab::Action::OpenCatalog),
            ButtonId::PluginFilter(_)
            | ButtonId::PluginSearch
            | ButtonId::PluginAdd
            | ButtonId::PluginAddGive
            | ButtonId::PluginCatalogs
            | ButtonId::PluginBack
            | ButtonId::CatalogAdd
            | ButtonId::CatalogUpdate
            | ButtonId::CatalogRemove => {
                if let Some(plugins) = &mut self.plugins {
                    let action = plugins.catalog_press(id);
                    self.plugin_action(action);
                }
            }
            ButtonId::PluginHooks
            | ButtonId::PluginServers
            | ButtonId::PluginUpdate
            | ButtonId::PluginRemove
            | ButtonId::PluginOpen => {
                if let (Some(plugins), Some(roles)) = (&self.plugins, &self.roles) {
                    let action = plugins.press(id, roles);
                    self.plugin_action(action);
                }
            }
            ButtonId::RetroGenerate
            | ButtonId::RetroOpen
            | ButtonId::RetroToggle
            | ButtonId::RetroApply => {
                if let Some(retro) = &mut self.retro {
                    let action = retro.press(id);
                    self.retro_action(action);
                }
            }
            ButtonId::SkillRole(_)
            | ButtonId::SkillEdit
            | ButtonId::SkillNew
            | ButtonId::SkillRestore
            | ButtonId::SkillMark(_) => {
                if let Some(skills) = &mut self.skills {
                    let action = skills.press(id);
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
            ButtonId::To | ButtonId::Model | ButtonId::Level => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.toggle_menu(match id {
                        ButtonId::Model => Menu::Model,
                        ButtonId::Level => Menu::Level,
                        _ => Menu::To,
                    });
                }
            }
            ButtonId::Send if self.generating() => {
                self.message = Some((self.tr.t("retro.busy").to_string(), true));
            }
            ButtonId::Send => {
                if let Some(tasks) = &mut self.tasks {
                    self.message = Some(match tasks.send(self.builder, &self.tr) {
                        Ok(text) => (text, false),
                        Err(error) => (error, true),
                    });
                }
            }
            ButtonId::UseProject => {
                if let Some(project) = self.projects.current() {
                    let path = project.path.clone();
                    if has_config(&path) {
                        self.open(&path);
                    } else if path.is_dir() {
                        self.form = Some(self.init_form(&path));
                    } else {
                        let text = self.tr.f("projects.gone", &[("path", &path.display())]);
                        self.message = Some((text, true));
                    }
                }
            }
            ButtonId::NewProject => self.pick(Pick::NewProject),
            ButtonId::OpenFolder => self.pick(Pick::Open),
            ButtonId::Choose => {
                if let Some((pick, browser)) = self.browser.take() {
                    self.picked(pick, browser.chosen());
                }
            }
            ButtonId::Up => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.up();
                }
            }
            ButtonId::NewFolder => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.naming = Some(String::new());
                }
            }
            ButtonId::ToggleHidden => {
                if let Some((_, browser)) = &mut self.browser {
                    browser.toggle_hidden();
                }
            }
            ButtonId::Theme => {
                theme::next();
                if let Some(home) = &self.home {
                    if let Err(error) = i18n::save_setting(home, "theme", theme::current().code) {
                        self.message = Some((error, true));
                    }
                }
            }
            ButtonId::Language => {
                self.tr.next();
                // The last message was in the old language.
                self.message = None;
                if let Some(home) = &self.home {
                    if let Err(error) = self.tr.save(home) {
                        self.message = Some((error, true));
                    }
                }
            }
            ButtonId::RemoveProject => {
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
            ButtonId::Save => {
                if let Some(roles) = &mut self.roles {
                    self.message = Some(match roles.save() {
                        Ok(()) => (self.tr.t("roles.saved").to_string(), false),
                        Err(error) => (error, true),
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

    /// What the Roles tab asks for after a click or a key.
    pub(crate) fn act(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Say(text) => self.message = Some((text, false)),
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
