//! Keys and buttons of the MCP tab's server list.

use ratatui::crossterm::event::KeyCode;

use harness_core::mcp::{self};

use super::tab::{is_oauth, secret_names, Action, McpButton, McpTab};
use crate::tabs::roles::RolesTab;

impl McpTab {
    /// Can the role get the selected server now? `Err` says why not.
    pub(super) fn can_give(&self, roles: &RolesTab, name: &str) -> Result<(), &'static str> {
        let settings = roles.settings(self.role()).ok_or("mcp.no_role")?;
        let on = settings.mcp.iter().any(|n| n == name);
        match roles.servers().get(name) {
            // A server the role has can always be taken away.
            _ if on => Ok(()),
            None => Err("mcp.not_described"),
            Some(server) if !mcp::agent_runs(settings.agent, server) => Err("mcp.agent_cannot"),
            Some(_) => Ok(()),
        }
    }

    /// «Give» or «Take away»: changes the role's list of servers. `Err` is
    /// the key of the message why it cannot.
    pub fn toggle(&self, roles: &mut RolesTab) -> Result<(), &'static str> {
        let Some(name) = self.current(roles) else {
            return Ok(());
        };
        self.can_give(roles, &name)?;
        roles.toggle_mcp(self.role(), &name);
        Ok(())
    }

    pub fn on_key(&mut self, key: KeyCode, roles: &mut RolesTab) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::Char('n') => return self.press(McpButton::New, roles),
            KeyCode::Char('f') => return self.open_catalog(),
            KeyCode::Char('i') => return self.press(McpButton::SignIn, roles),
            KeyCode::Char('e') => return self.press(McpButton::Edit, roles),
            KeyCode::Delete => return self.press(McpButton::Remove, roles),
            _ => {}
        }
        Action::None
    }

    /// A button of the server list.
    pub fn press(&mut self, id: McpButton, roles: &mut RolesTab) -> Action {
        // The selected server; `None` when the list is empty.
        let current = self.current(roles);
        match id {
            McpButton::Role(index) => {
                self.choose_role(index);
                Action::None
            }
            McpButton::New => Action::New,
            McpButton::OpenCatalog => self.open_catalog(),
            McpButton::Check => Action::Check,
            McpButton::Toggle => match self.toggle(roles) {
                Ok(()) => Action::None,
                Err(key) => Action::Refused(key),
            },
            McpButton::Edit => current.map_or(Action::None, Action::Edit),
            McpButton::Remove => match current {
                Some(name) if roles.servers().contains_key(&name) => Action::Remove(name),
                _ => Action::None,
            },
            McpButton::SignIn => match current {
                Some(name)
                    if roles.servers().get(&name).is_some_and(is_oauth)
                        && self.signing.is_none() =>
                {
                    Action::SignIn(name)
                }
                _ => Action::None,
            },
            McpButton::Secret => match current {
                Some(name) => Action::Secret(self.secret_to_ask(&name, roles)),
                None => Action::None,
            },
        }
    }

    /// The secret of server `name` to ask for: the first one not saved yet,
    /// else the first one, else the server's own name.
    fn secret_to_ask(&self, name: &str, roles: &RolesTab) -> String {
        let wanted: Vec<String> = roles
            .servers()
            .get(name)
            .map(|s| secret_names(s).collect())
            .unwrap_or_default();
        wanted
            .iter()
            .find(|n| !self.secrets.contains(n))
            .or(wanted.first())
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}
