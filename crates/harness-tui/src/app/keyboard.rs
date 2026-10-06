//! Keyboard input: hot keys, typing into forms, pasting, and the folder browser.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tabs::mcp::McpTab;
use crate::tabs::plugins::PluginsTab;
use crate::tabs::roles::RolesTab;
use crate::tabs::tasks::TasksTab;
use crate::ui::keys;
use crate::ui::message::Message;
use crate::ui::ButtonId;
use crate::{App, Tab, TABS};

impl App {
    /// Pasted text goes to the open form, the new folder name, or the message box.
    pub(crate) fn on_paste(&mut self, text: &str) {
        if let Some((_, form)) = &mut self.form {
            text.chars()
                .filter(|c| !c.is_control())
                .for_each(|c| form.type_char(c));
        } else if let Some(name) = self.browser.as_mut().and_then(|(_, b)| b.naming.as_mut()) {
            name.extend(text.chars().filter(|c| !c.is_control()));
        } else if self.tab == Tab::Tasks && self.browser.is_none() {
            if let Some(tasks) = &mut self.tasks {
                tasks.paste(text);
            }
        }
    }

    /// One key press: Ctrl+C quits, the folder browser or a form gets it first.
    pub(crate) fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        let quit_warned = std::mem::take(&mut self.quit_warned);
        if self.browser.is_some() {
            self.browser_key(key.code);
            return;
        }
        if let Some((_, form)) = &mut self.form {
            match key.code {
                KeyCode::Esc => self.close_form(),
                KeyCode::Enter => self.submit(),
                KeyCode::Tab | KeyCode::Down => form.next_field(),
                KeyCode::Backspace => form.backspace(),
                KeyCode::Char(c) => form.type_char(c),
                _ => {}
            }
            return;
        }
        // Writing the message: every key is text, except these.
        if self.tab == Tab::Tasks {
            if let Some(tasks) = self.tasks.as_mut().filter(|t| t.typing()) {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                match (key.code, keys::latin(key.code)) {
                    // Enter is a new line; these send.
                    (_, KeyCode::Char('s')) if control => self.press(ButtonId::Send),
                    (KeyCode::Enter, _) if control || alt => self.press(ButtonId::Send),
                    // Other Ctrl and Alt keys are not text.
                    _ if control || alt => {}
                    (code, _) => tasks.on_key(code),
                }
                return;
            }
        }
        // Hot keys work with a Russian keyboard layout too: «й» is q.
        let code = keys::latin(key.code);
        let unsaved = self.roles.as_ref().is_some_and(RolesTab::changed);
        let running = self.tasks.as_ref().is_some_and(TasksTab::is_running) || self.generating();
        // A message written but not sent is not thrown away by one key.
        let unsent = self
            .tasks
            .as_ref()
            .is_some_and(|t| !t.input.trim().is_empty());
        match code {
            KeyCode::Esc if self.tasks.as_ref().is_some_and(|t| t.menu.is_some()) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.menu = None;
                }
            }
            KeyCode::Esc
                if self.tab == Tab::Tasks
                    && self.tasks.as_ref().is_some_and(|t| t.zoom.is_some()) =>
            {
                if let Some(tasks) = &mut self.tasks {
                    tasks.zoom = None;
                }
            }
            KeyCode::Char('q') | KeyCode::Esc if running => {
                let key = if self.generating() {
                    "retro.quit_running"
                } else {
                    "tasks.quit_running"
                };
                self.message = Some(Message::error(self.tr.t(key)));
            }
            KeyCode::Esc
                if self.tab == Tab::Roles
                    && self.roles.as_ref().is_some_and(RolesTab::in_details) =>
            {
                if let Some(roles) = &mut self.roles {
                    roles.on_key(KeyCode::Esc, &self.tr);
                }
            }
            KeyCode::Esc
                if self.tab == Tab::Mcp && self.mcp.as_ref().is_some_and(McpTab::in_catalog) =>
            {
                if let Some(mcp) = &mut self.mcp {
                    mcp.catalog = None;
                }
            }
            KeyCode::Esc
                if self.tab == Tab::Plugins
                    && self.plugins.as_ref().is_some_and(PluginsTab::in_catalog) =>
            {
                if let Some(plugins) = &mut self.plugins {
                    plugins.catalog_key(KeyCode::Esc);
                }
            }
            KeyCode::Char('q') | KeyCode::Esc if unsaved && !quit_warned => {
                self.quit_warned = true;
                self.message = Some(Message::error(self.tr.t("roles.unsaved_quit")));
            }
            KeyCode::Char('q') | KeyCode::Esc if unsent && !quit_warned => {
                self.quit_warned = true;
                self.message = Some(Message::error(self.tr.t("tasks.unsent_quit")));
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('L') | KeyCode::F(2) => self.press(ButtonId::Language),
            KeyCode::Char('T') | KeyCode::F(3) => self.press(ButtonId::Theme),
            KeyCode::Char(c @ '1'..='8') => {
                let index = usize::from(c as u8 - b'1');
                self.show(TABS[index].0);
            }
            KeyCode::Char('r') | KeyCode::F(5) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.reload();
                }
                // Unsaved changes are not thrown away by a reload.
                if let Some(roles) = self.roles.as_mut().filter(|r| !r.changed()) {
                    roles.reload();
                }
                if let Some(skills) = &mut self.skills {
                    skills.reload();
                }
                if let Some(retro) = self.retro.as_mut().filter(|r| !r.is_generating()) {
                    retro.reload();
                }
                if let Some(mcp) = &mut self.mcp {
                    mcp.reload();
                }
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                    plugins.reload(roles);
                }
                self.reload_catalog_views();
                self.projects.reload();
            }
            code => match self.tab {
                Tab::Tasks => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_key(code);
                    }
                }
                Tab::Roles => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    code => {
                        if let Some(roles) = &mut self.roles {
                            let action = roles.on_key(code, &self.tr);
                            self.act(action);
                        }
                    }
                },
                Tab::Skills => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    code => {
                        if let Some(skills) = &mut self.skills {
                            let action = skills.on_key(code);
                            self.skill_action(action);
                        }
                    }
                },
                Tab::Mcp if self.mcp.as_ref().is_some_and(McpTab::in_catalog) => {
                    if let Some(mcp) = &mut self.mcp {
                        let action = mcp.catalog_key(code);
                        self.mcp_action(action);
                    }
                }
                Tab::Mcp => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    KeyCode::Char(' ') | KeyCode::Enter => self.press(ButtonId::McpToggle),
                    KeyCode::Char('c') => self.press(ButtonId::McpCheck),
                    code => {
                        if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                            let action = mcp.on_key(code, roles);
                            self.mcp_action(action);
                        }
                    }
                },
                Tab::Plugins if self.plugins.as_ref().is_some_and(PluginsTab::in_catalog) => {
                    if let Some(plugins) = &mut self.plugins {
                        let action = plugins.catalog_key(code);
                        self.plugin_action(action);
                    }
                }
                Tab::Plugins => match code {
                    KeyCode::Char('s') => self.press(ButtonId::Save),
                    KeyCode::Char('u') => self.press(ButtonId::Undo),
                    KeyCode::Char(' ') | KeyCode::Enter => self.press(ButtonId::PluginToggle),
                    code => {
                        if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                            let action = plugins.on_key(code, roles);
                            self.plugin_action(action);
                        }
                    }
                },
                Tab::Retro => {
                    if let Some(retro) = &mut self.retro {
                        let action = retro.on_key(code);
                        self.retro_action(action);
                    }
                }
                Tab::Agents => match code {
                    KeyCode::Char('c') => self.press(ButtonId::AgentsCheck),
                    KeyCode::Char('i') | KeyCode::Enter => self.press(ButtonId::AgentRun),
                    KeyCode::Delete => self.press(ButtonId::AgentRemove),
                    KeyCode::Char('l') => self.press(ButtonId::AgentSignIn),
                    code => self.agents.on_key(code),
                },
                Tab::Projects => match code {
                    KeyCode::Enter => self.press(ButtonId::UseProject),
                    KeyCode::Char('n') => self.press(ButtonId::NewProject),
                    KeyCode::Char('o') => self.press(ButtonId::OpenFolder),
                    KeyCode::Delete => self.press(ButtonId::RemoveProject),
                    code => self.projects.on_key(code),
                },
            },
        }
    }

    /// Keys while the folder browser is open.
    fn browser_key(&mut self, code: KeyCode) {
        let Some((_, browser)) = &mut self.browser else {
            return;
        };
        if let Some(name) = &mut browser.naming {
            match code {
                KeyCode::Esc => browser.naming = None,
                KeyCode::Enter => browser.create(&self.tr),
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c) => name.push(c),
                _ => {}
            }
            return;
        }
        match keys::latin(code) {
            KeyCode::Esc => self.browser = None,
            KeyCode::Up | KeyCode::Char('k') => browser.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => browser.move_by(1),
            KeyCode::PageUp => browser.move_by(-10),
            KeyCode::PageDown => browser.move_by(10),
            KeyCode::Enter | KeyCode::Right => browser.open_selected(),
            KeyCode::Backspace | KeyCode::Left => browser.up(),
            KeyCode::Char('n') => browser.naming = Some(String::new()),
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Char('c') => self.press(ButtonId::Choose),
            _ => {}
        }
    }
}
