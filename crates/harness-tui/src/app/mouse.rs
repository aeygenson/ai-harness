//! Mouse input: what a click, double click or wheel turn on the screen does.
//! Every clickable place is found through the hit map (`Hits`), which already
//! accounts for scrolling.

use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};

use crate::app::splash::Splash;
use crate::tabs::mcp::{McpButton, McpCatalogButton};
use crate::tabs::plugins::{PluginButton, PluginCatalogButton};
use crate::tabs::retro::{self, RetroButton};
use crate::tabs::skills::SkillButton;
use crate::ui::message::Message;
use crate::ui::{ButtonId, Hits, ListId, Target};
use crate::{App, Tab, DOUBLE_CLICK, TABS};

impl App {
    /// A mouse event: a click (two in a row are a double click) or a wheel turn.
    pub(crate) fn on_mouse(&mut self, mouse: MouseEvent) {
        let hit = self.hits.at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if self.splash == Splash::Open => {
                self.splash_click(hit);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let now = Instant::now();
                let double = matches!(
                    (self.last_click, hit),
                    (Some((at, target, row)), Some((t, r)))
                        if target == t && row == r && now - at < DOUBLE_CLICK
                );
                self.last_click = hit.map(|(target, row)| (now, target, row));
                self.click(hit, double);
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                self.wheel(hit, mouse.kind == MouseEventKind::ScrollDown);
            }
            _ => {}
        }
    }

    /// A wheel turn over `hit`: scrolls the list or text under the mouse.
    fn wheel(&mut self, hit: Option<(Target, u16)>, down: bool) {
        if let Some((_, browser)) = &mut self.browser {
            browser.move_by(if down { 1 } else { -1 });
            return;
        }
        if self.form.is_some() {
            return;
        }
        match (self.tab, hit.and_then(|(t, r)| Hits::row(t, r))) {
            (Tab::Tasks, Some((list, _))) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.on_click_list(list);
                    tasks.on_key(if down { KeyCode::Down } else { KeyCode::Up });
                }
            }
            (Tab::Tasks, None) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.on_wheel(down);
                }
            }
            (Tab::Projects, _) => {
                self.projects
                    .on_key(if down { KeyCode::Down } else { KeyCode::Up });
            }
            (Tab::Agents, _) => self.agents.move_by(if down { 1 } else { -1 }),
            (Tab::Roles, Some((ListId::Roles, _))) => {
                if let Some(roles) = &mut self.roles {
                    let next = if down {
                        roles.selected + 1
                    } else {
                        roles.selected.saturating_sub(1)
                    };
                    roles.select(next);
                }
            }
            (Tab::Roles, _) => {
                if let Some(roles) = &mut self.roles {
                    roles.on_wheel(down);
                }
            }
            (Tab::Skills, Some((ListId::Skills, _))) => {
                if let Some(skills) = &mut self.skills {
                    skills.move_by(if down { 1 } else { -1 });
                }
            }
            (Tab::Skills, _) => {
                if let Some(skills) = &mut self.skills {
                    skills.on_wheel(down);
                }
            }
            (Tab::Mcp, _) => {
                if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                    if mcp.in_catalog() {
                        mcp.move_found(if down { 1 } else { -1 });
                    } else {
                        mcp.move_by(if down { 1 } else { -1 }, roles);
                    }
                }
            }
            (Tab::Retro, Some((list @ (ListId::Retros | ListId::RetroProposals), _))) => {
                if let Some(retro) = &mut self.retro {
                    retro.focus = if list == ListId::Retros {
                        retro::Focus::Retros
                    } else {
                        retro::Focus::Proposals
                    };
                    retro.move_by(if down { 1 } else { -1 });
                }
            }
            (Tab::Retro, _) => {
                if let Some(retro) = &mut self.retro {
                    retro.on_wheel(down);
                }
            }
            (Tab::Plugins, _) => {
                let delta = if down { 1 } else { -1 };
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                    if let Some(view) = &mut plugins.catalogs {
                        view.move_by(delta);
                    } else if let Some(view) = &mut plugins.catalog {
                        view.move_by(delta);
                    } else {
                        plugins.move_by(delta, roles);
                    }
                }
            }
        }
    }

    /// What a click on `hit` (a place from the hit map and its row) does.
    pub(crate) fn click(&mut self, hit: Option<(Target, u16)>, double: bool) {
        // An open list of the message box closes with any click; a click on
        // it chooses.
        if let Some((tasks, menu)) = self
            .tasks
            .as_mut()
            .and_then(|t| t.menu.take().map(|menu| (t, menu)))
        {
            if let Some((
                target @ Target::List {
                    list: ListId::Choices,
                    ..
                },
                row,
            )) = hit
            {
                if let Some((_, index)) = Hits::row(target, row) {
                    if !tasks.choose(menu, index) {
                        self.message = Some(Message::error(self.tr.t("tasks.choice_unavailable")));
                    }
                }
            }
            if !matches!(
                hit,
                Some((Target::Button(ButtonId::Input | ButtonId::Send), _))
            ) {
                return;
            }
        }
        if let Some((_, browser)) = &mut self.browser {
            match hit {
                Some((Target::Button(id), _)) => self.press(id),
                Some((target @ Target::List { .. }, row)) => {
                    if let Some((_, index)) = Hits::row(target, row) {
                        browser.mark(index);
                        if double {
                            browser.open_selected();
                        }
                    }
                }
                Some((Target::Window, _)) => {}
                // A click outside the window closes it.
                _ => self.browser = None,
            }
            return;
        }
        if let Some((_, form)) = &mut self.form {
            match hit {
                Some((Target::Button(ButtonId::Ok), _)) => self.submit(),
                Some((Target::Field(i), _)) if i < form.fields.len() => {
                    form.focus = i;
                    form.switch(i);
                }
                Some((Target::Field(_) | Target::Window, _)) => {}
                // «Cancel», or a click outside the window, closes it.
                _ => self.close_form(),
            }
            return;
        }
        let Some((target, row)) = hit else {
            return;
        };
        match target {
            Target::Tab(index) => self.show(TABS[index].0),
            Target::Button(id) => self.press(id),
            Target::List { .. } => {
                if let Some((list, index)) = Hits::row(target, row) {
                    self.click_list(list, index, double);
                }
            }
            Target::Row(index) => {
                if let Some(roles) = &mut self.roles {
                    let action = roles.activate(index, &self.tr);
                    self.act(action);
                }
            }
            Target::Field(_) | Target::Window => {}
        }
    }

    /// A click on row `index` of `list`: selects it; a double click also opens or toggles it.
    fn click_list(&mut self, list: ListId, index: usize, double: bool) {
        match list {
            ListId::Projects => {
                if index < self.projects.list.projects.len() {
                    self.projects.selected = index;
                    if double {
                        self.press(ButtonId::UseProject);
                    }
                }
            }
            ListId::Roles => {
                if let Some(roles) = &mut self.roles {
                    roles.select(index);
                }
            }
            ListId::Agents => self.agents.select(index),
            ListId::Skills => {
                if let Some(skills) = &mut self.skills {
                    skills.select(index);
                }
                if double {
                    self.press(ButtonId::Skill(SkillButton::Edit));
                }
            }
            ListId::Mcp => {
                if let (Some(mcp), Some(roles)) = (&mut self.mcp, &self.roles) {
                    mcp.select(index, roles);
                }
                if double {
                    self.press(ButtonId::Mcp(McpButton::Toggle));
                }
            }
            ListId::Plugins => {
                if let (Some(plugins), Some(roles)) = (&mut self.plugins, &self.roles) {
                    plugins.select(index, roles);
                }
                if double {
                    self.press(ButtonId::Plugin(PluginButton::Toggle));
                }
            }
            ListId::PluginCatalog => {
                if let Some(view) = self.plugins.as_mut().and_then(|p| p.catalog.as_mut()) {
                    view.select(index);
                }
                if double {
                    self.press(ButtonId::PluginCatalog(PluginCatalogButton::Add));
                }
            }
            ListId::PluginCatalogs => {
                if let Some(view) = self.plugins.as_mut().and_then(|p| p.catalogs.as_mut()) {
                    view.select(index);
                }
            }
            ListId::Retros => {
                if let Some(retro) = &mut self.retro {
                    retro.select(index);
                }
            }
            ListId::RetroProposals => {
                if let Some(retro) = &mut self.retro {
                    retro.select_proposal(index);
                }
                if double {
                    self.press(ButtonId::Retro(RetroButton::Toggle));
                }
            }
            ListId::McpCatalog => {
                if let Some(mcp) = &mut self.mcp {
                    mcp.select_found(index);
                }
                if double {
                    self.press(ButtonId::McpCatalog(McpCatalogButton::Use));
                }
            }
            ListId::RoleFilter => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.toggle_filter(index);
                }
            }
            list @ (ListId::Tasks | ListId::Steps) => {
                if let Some(tasks) = &mut self.tasks {
                    tasks.on_click(list, index);
                }
            }
            // Clicks in the folder browser and the «To» menu are handled in
            // `click`, while they are open.
            ListId::Folders | ListId::Choices => {}
        }
    }
}
