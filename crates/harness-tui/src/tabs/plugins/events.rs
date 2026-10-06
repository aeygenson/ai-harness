//! Keys and buttons of the Plugins tab: the plugin list and the catalog.

use ratatui::crossterm::event::KeyCode;

use harness_core::config::AgentKind;

use super::tab::{Action, PluginButton, PluginCatalogButton, PluginsTab};
use crate::tabs::plugins::catalog::{unusable, CatalogView, CatalogsView, OFFICIAL};
use crate::tabs::roles::RolesTab;

impl PluginsTab {
    /// Can the role get the plugin now? `Err` is the key of the message why not.
    pub(super) fn can_give(&self, roles: &RolesTab, name: &str) -> Result<(), &'static str> {
        if roles.settings(self.role()).is_none() {
            return Err("plugins.no_role");
        }
        // A plugin the role has can always be taken away.
        if self.has(roles, name) {
            return Ok(());
        }
        let agent = self.agent(roles);
        match roles.plugins().get(name) {
            None => Err("plugins.not_described_short"),
            Some(_) if !agent.is_some_and(AgentKind::has_plugins) => Err("plugins.agent_has_none"),
            Some(plugin) if Some(plugin.agent) != agent => Err("plugins.other_agent"),
            Some(_) => Ok(()),
        }
    }

    /// «Give» or «Take away». `Err` is the key of the message why it cannot.
    pub fn toggle(&self, roles: &mut RolesTab) -> Result<(), &'static str> {
        let Some(name) = self.current(roles) else {
            return Ok(());
        };
        self.can_give(roles, &name)?;
        roles.toggle_plugin(self.role(), &name);
        Ok(())
    }

    pub fn on_key(&mut self, key: KeyCode, roles: &mut RolesTab) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1, roles),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1, roles),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::Char('e') => return self.press(PluginButton::Open, roles),
            KeyCode::Char('f') => return Action::OpenCatalog,
            KeyCode::Char('U') => return self.press(PluginButton::Update, roles),
            KeyCode::Delete => return self.press(PluginButton::Remove, roles),
            _ => {}
        }
        Action::None
    }

    /// A key while «From catalog» or «Catalogs» is open.
    pub fn catalog_key(&mut self, key: KeyCode) -> Action {
        if let Some(view) = &mut self.catalogs {
            match key {
                KeyCode::Up | KeyCode::Char('k') => view.move_by(-1),
                KeyCode::Down | KeyCode::Char('j') => view.move_by(1),
                KeyCode::Char('n') => return self.catalog_press(PluginCatalogButton::CatalogAdd),
                KeyCode::Char('U') => {
                    return self.catalog_press(PluginCatalogButton::CatalogUpdate)
                }
                KeyCode::Delete => return self.catalog_press(PluginCatalogButton::CatalogRemove),
                KeyCode::Esc | KeyCode::Backspace => self.catalogs = None,
                _ => {}
            }
            return Action::None;
        }
        let Some(view) = &mut self.catalog else {
            return Action::None;
        };
        match key {
            KeyCode::Up | KeyCode::Char('k') => view.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => view.move_by(1),
            KeyCode::Left | KeyCode::Char('h') => {
                view.choose_filter(view.filter.saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char('l') => view.choose_filter(view.filter + 1),
            KeyCode::Char('/' | 's' | 'f') => {
                return self.catalog_press(PluginCatalogButton::Search)
            }
            KeyCode::Enter | KeyCode::Char('a') => {
                return self.catalog_press(PluginCatalogButton::Add)
            }
            KeyCode::Char('g') => return self.catalog_press(PluginCatalogButton::AddGive),
            KeyCode::Char('c') => return self.catalog_press(PluginCatalogButton::OpenCatalogs),
            KeyCode::Esc | KeyCode::Backspace => self.catalog = None,
            _ => {}
        }
        Action::None
    }

    /// A button of «From catalog» or «Catalogs».
    pub fn catalog_press(&mut self, id: PluginCatalogButton) -> Action {
        // A download is running: nothing that starts another one.
        let busy = self.busy.is_some();
        match id {
            PluginCatalogButton::Back if self.catalogs.is_some() => self.catalogs = None,
            PluginCatalogButton::Back => self.catalog = None,
            PluginCatalogButton::OpenCatalogs => return Action::OpenCatalogs,
            PluginCatalogButton::Filter(index) => {
                if let Some(view) = &mut self.catalog {
                    view.choose_filter(index);
                }
            }
            PluginCatalogButton::Search => {
                if let Some(view) = &self.catalog {
                    return Action::Search(view.query.clone());
                }
            }
            PluginCatalogButton::Add | PluginCatalogButton::AddGive if !busy => {
                if let Some(entry) = self.catalog.as_ref().and_then(CatalogView::current) {
                    if let Some(why) = unusable(entry) {
                        return Action::Unusable(why);
                    }
                    return Action::Add {
                        entry: entry.clone(),
                        give: id == PluginCatalogButton::AddGive,
                    };
                }
            }
            PluginCatalogButton::CatalogAdd if !busy => {
                let empty = self.catalogs.as_ref().is_none_or(|v| v.list.is_empty());
                return Action::AddCatalog(if empty { OFFICIAL } else { "" }.to_string());
            }
            PluginCatalogButton::CatalogUpdate if !busy => {
                if let Some(c) = self.catalogs.as_ref().and_then(CatalogsView::current) {
                    return Action::UpdateCatalog(c.name.clone());
                }
            }
            PluginCatalogButton::CatalogRemove if !busy => {
                if let Some(c) = self.catalogs.as_ref().and_then(CatalogsView::current) {
                    return Action::RemoveCatalog(c.name.clone());
                }
            }
            PluginCatalogButton::Add
            | PluginCatalogButton::AddGive
            | PluginCatalogButton::CatalogAdd
            | PluginCatalogButton::CatalogUpdate
            | PluginCatalogButton::CatalogRemove => {}
        }
        Action::None
    }

    /// A button of the plugin list.
    pub fn press(&mut self, id: PluginButton, roles: &mut RolesTab) -> Action {
        match id {
            PluginButton::Role(index) => {
                self.choose_role(index);
                return Action::None;
            }
            PluginButton::OpenCatalog => return Action::OpenCatalog,
            PluginButton::Toggle => {
                return match self.toggle(roles) {
                    Ok(()) => Action::None,
                    Err(key) => Action::Refused(key),
                }
            }
            PluginButton::Hooks
            | PluginButton::Servers
            | PluginButton::Remove
            | PluginButton::Open
            | PluginButton::Update => {}
        }
        // The rest is about the selected plugin.
        let Some(name) = self.current(roles) else {
            return Action::None;
        };
        let Some(plugin) = roles.plugins().get(&name) else {
            return Action::None;
        };
        let (has_hooks, has_servers) = self.brings(&name);
        match id {
            PluginButton::Remove => Action::Remove(name),
            PluginButton::Open => Action::Open(name),
            PluginButton::Update if plugin.source.is_some() && self.busy.is_none() => {
                Action::Update(name)
            }
            PluginButton::Hooks if has_hooks || plugin.allow_hooks => Action::Allow {
                name,
                hooks: !plugin.allow_hooks,
                servers: plugin.allow_mcp,
            },
            PluginButton::Servers if has_servers || plugin.allow_mcp => Action::Allow {
                name,
                hooks: plugin.allow_hooks,
                servers: !plugin.allow_mcp,
            },
            // Not possible now, or handled above.
            PluginButton::Update
            | PluginButton::Hooks
            | PluginButton::Servers
            | PluginButton::Role(_)
            | PluginButton::Toggle
            | PluginButton::OpenCatalog => Action::None,
        }
    }
}
