//! Keys for the open tab: what each tab does with a key no hot key took.

use ratatui::crossterm::event::KeyCode;

use crate::tabs::mcp::{McpButton, McpTab};
use crate::tabs::plugins::{PluginButton, PluginsTab};
use crate::ui::ButtonId;
use crate::{App, Tab};

impl App {
    /// A key that no hot key took: the open tab handles it.
    pub(super) fn tab_key(&mut self, code: KeyCode) {
        match self.tab {
            Tab::Tasks => match code {
                KeyCode::Char('o') => {
                    if let Some(index) = self.tasks.as_ref().map(|t| t.task) {
                        self.press(ButtonId::TaskPullRequest(index));
                    }
                }
                KeyCode::Char('n') => self.press(ButtonId::TaskQuestions),
                KeyCode::Char('s') => self.press(ButtonId::StopAfterStep),
                KeyCode::Char('x') => self.press(ButtonId::StopNow),
                code => {
                    if let Some(tasks) = &mut self.tasks {
                        tasks.on_key(code);
                    }
                }
            },
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
                KeyCode::Char(' ') | KeyCode::Enter => {
                    self.press(ButtonId::Mcp(McpButton::Toggle));
                }
                KeyCode::Char('c') => self.press(ButtonId::Mcp(McpButton::Check)),
                code => {
                    if let (Some(mcp), Some(roles)) = (&mut self.mcp, &mut self.roles) {
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
                KeyCode::Char(' ') | KeyCode::Enter => {
                    self.press(ButtonId::Plugin(PluginButton::Toggle));
                }
                code => {
                    if let (Some(plugins), Some(roles)) = (&mut self.plugins, &mut self.roles) {
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
                KeyCode::Char('u') => self.press(ButtonId::HarnessUpdate),
                code => self.agents.on_key(code),
            },
            Tab::Projects => match code {
                KeyCode::Enter => self.press(ButtonId::UseProject),
                KeyCode::Char('n') => self.press(ButtonId::NewProject),
                KeyCode::Char('o') => self.press(ButtonId::OpenFolder),
                KeyCode::Delete => self.press(ButtonId::RemoveProject),
                KeyCode::Char('g') => self.press(ButtonId::ConnectGitHub),
                code => self.projects.on_key(code),
            },
        }
    }
}
