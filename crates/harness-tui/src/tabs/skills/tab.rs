//! The Skills tab: what each role is told, and the skills it can choose.
//!
//! ```text
//!  Role: [ architect ] [ developer ] [ tester ] [ security ]
//! ┌ Skills of the developer ──────────┐┌ developer · built-in ────────────────────┐
//! │ Always in the prompt              ││ ---                                      │
//! │> common          built-in         ││ description: How the developer ...       │
//! │  developer       changed          ││ ---                                      │
//! │ Agent notes (● in the prompt)     ││ # Developer                              │
//! │  ● agent-claude  built-in         ││ ...                                      │
//! │  ○ agent-codex   built-in         ││                                          │
//! │ Optional: click the mark          ││                                          │
//! │  [x] crash-recovery  built-in     ││                                          │
//! │  [■] rust-errors     own          ││                                          │
//! └───────────────────────────────────┘└──────────────────────────────────────────┘
//!  Edit in Zed   New skill   Restore built-in   Save   Undo changes
//! ```
//!
//! Skills are edited in Zed (see `editor`). Editing a built-in skill makes a
//! copy in `.harness/skills/`, which replaces it in this project; «Restore
//! built-in» deletes the copy. A click on the mark of an optional skill (or
//! Space) changes it for the chosen role: `[ ]` not used, `[x]` read when
//! needed, `[■]` always in the prompt. Like on the Roles tab, the marks are
//! kept in harness.toml by «Save».

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::KeyCode;

use harness_core::config::{AgentKind, Config};
use harness_core::git::HARNESS_DIR;
use harness_core::skills::{self, LibrarySkill, Source};
use harness_core::task::handoff::Role;

/// The roles the selector offers, in the order of the flow.
pub const ROLES: [Role; 4] = [
    Role::Architect,
    Role::Developer,
    Role::Tester,
    Role::Security,
];

/// How the names of the agents' notes start.
pub(super) const AGENT_NOTE: &str = "agent-";

/// A button of the Skills tab, clicked or chosen with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillButton {
    /// A role of the selector, an index into `ROLES`.
    Role(usize),
    Edit,
    New,
    /// Go back to the built-in text of a changed skill.
    Restore,
    /// The `[ ]` mark of the skill on this row of the list.
    Mark(usize),
}

/// What the tab asks the App to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Open this skill in the editor.
    Edit(String),
    New,
    /// Ask before deleting the project's copy of this built-in skill.
    Restore(String),
    /// The next mark of this optional skill for the role.
    Cycle(Role, String),
}

/// A line of the list: a heading, or a skill by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    Heading(&'static str),
    Skill(String),
}

#[derive(Debug)]
pub struct SkillsTab {
    root: PathBuf,
    /// The role chosen at the top, an index into `ROLES`.
    pub(crate) role: usize,
    pub(super) library: Vec<LibrarySkill>,
    /// Each role's agent.
    pub(super) roles: BTreeMap<Role, AgentKind>,
    /// The selected line of the list.
    pub(crate) row: usize,
    pub(crate) scroll: u16,
    pub problem: Option<String>,
}

impl SkillsTab {
    pub fn load(root: &Path) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            role: 0,
            library: Vec::new(),
            roles: BTreeMap::new(),
            row: 1,
            scroll: 0,
            problem: None,
        };
        tab.reload();
        tab
    }

    /// Reads the skills and harness.toml again, keeping the selected skill.
    pub fn reload(&mut self) {
        let keep = self.current().map(|s| s.name.clone());
        let harness_dir = self.root.join(HARNESS_DIR);
        self.library = skills::library(&harness_dir);
        self.roles.clear();
        self.problem = None;
        match Config::load(&harness_dir) {
            Ok(config) => {
                for (role, settings) in config.roles {
                    self.roles.insert(role, settings.agent);
                }
            }
            Err(error) => self.problem = Some(error.to_string()),
        }
        self.select_named(keep.as_deref());
    }

    pub(super) fn role(&self) -> Role {
        ROLES[self.role.min(ROLES.len() - 1)]
    }

    /// The agent of the chosen role, `claude` if harness.toml has no such role.
    pub(super) fn agent(&self) -> AgentKind {
        self.roles
            .get(&self.role())
            .copied()
            .unwrap_or(AgentKind::Claude)
    }

    pub(super) fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Heading("skills.base")];
        rows.extend(
            skills::base_names(self.role(), self.agent())
                .into_iter()
                .filter(|n| !n.starts_with(AGENT_NOTE))
                .map(|n| Row::Skill(n.to_string())),
        );
        // Every agent's note: the one of this role's agent is in its prompt.
        rows.push(Row::Heading("skills.agents"));
        rows.extend(
            self.library
                .iter()
                .filter(|s| s.name.starts_with(AGENT_NOTE))
                .map(|s| Row::Skill(s.name.clone())),
        );
        rows.push(Row::Heading("skills.optional"));
        rows.extend(
            self.library
                .iter()
                .filter(|s| !skills::is_base(&s.name))
                .map(|s| Row::Skill(s.name.clone())),
        );
        rows
    }

    /// The selected skill.
    pub fn current(&self) -> Option<&LibrarySkill> {
        match self.rows().get(self.row) {
            Some(Row::Skill(name)) => self.library.iter().find(|s| s.name == *name),
            _ => None,
        }
    }

    /// Selects the skill `name`, or the first skill.
    pub fn select_named(&mut self, name: Option<&str>) {
        let rows = self.rows();
        let at = name.and_then(|n| rows.iter().position(|r| *r == Row::Skill(n.to_string())));
        self.row = at.unwrap_or(1);
        self.scroll = 0;
    }

    /// Chooses the role at `index` of the selector.
    pub fn choose_role(&mut self, index: usize) {
        if index < ROLES.len() && index != self.role {
            let keep = self.current().map(|s| s.name.clone());
            self.role = index;
            // The role's own skill follows the role; another skill stays.
            let keep =
                keep.filter(|n| !skills::is_base(n) || n == "common" || n.starts_with(AGENT_NOTE));
            self.select_named(keep.as_deref());
        }
    }

    /// The optional skill on row `index`: the one a mark can be set for.
    pub(super) fn optional_at(&self, index: usize) -> Option<String> {
        match self.rows().get(index) {
            Some(Row::Skill(name)) if !skills::is_base(name) && !name.starts_with(AGENT_NOTE) => {
                Some(name.clone())
            }
            _ => None,
        }
    }

    /// A click on a line of the list.
    pub fn select(&mut self, index: usize) {
        if matches!(self.rows().get(index), Some(Row::Skill(_))) {
            self.row = index;
            self.scroll = 0;
        }
    }

    /// The next or previous skill, over the headings.
    pub fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let mut at = self.row;
        loop {
            let Some(next) = at.checked_add_signed(delta).filter(|n| *n < rows.len()) else {
                return;
            };
            at = next;
            if matches!(rows[at], Row::Skill(_)) {
                self.row = at;
                self.scroll = 0;
                return;
            }
        }
    }

    pub fn on_wheel(&mut self, down: bool) {
        self.scroll = if down {
            self.scroll.saturating_add(3)
        } else {
            self.scroll.saturating_sub(3)
        };
    }

    pub fn on_key(&mut self, key: KeyCode) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Left | KeyCode::Char('h') => self.choose_role(self.role.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.choose_role(self.role + 1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Enter | KeyCode::Char('e') => return self.press(SkillButton::Edit),
            KeyCode::Char(' ') => return self.press(SkillButton::Mark(self.row)),
            KeyCode::Char('n') => return self.press(SkillButton::New),
            KeyCode::Delete => return self.press(SkillButton::Restore),
            _ => {}
        }
        Action::None
    }

    /// A button of the tab.
    pub fn press(&mut self, id: SkillButton) -> Action {
        match id {
            SkillButton::Role(index) => self.choose_role(index),
            SkillButton::Edit => {
                if let Some(skill) = self.current() {
                    return Action::Edit(skill.name.clone());
                }
            }
            SkillButton::New => return Action::New,
            SkillButton::Mark(index) => {
                if let Some(name) = self.optional_at(index) {
                    self.select(index);
                    return Action::Cycle(self.role(), name);
                }
            }
            SkillButton::Restore => {
                if let Some(skill) = self.current() {
                    if matches!(skill.source, Source::Changed { .. }) {
                        return Action::Restore(skill.name.clone());
                    }
                }
            }
        }
        Action::None
    }

    /// Is there a skill with this name (built-in or the project's)?
    pub fn exists(&self, name: &str) -> bool {
        self.library.iter().any(|s| s.name == name)
    }
}
