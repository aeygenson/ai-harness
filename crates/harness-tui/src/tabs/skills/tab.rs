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

use harness_core::config::{AgentKind, Config};
use harness_core::git::HARNESS_DIR;
use harness_core::skills::{self, LibrarySkill, Source};
use harness_core::task::handoff::Role;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::tabs::roles::RolesTab;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selected, selector, ButtonId, Hits, ListId, Target};

/// The roles the selector offers, in the order of the flow.
pub const ROLES: [Role; 4] = [
    Role::Architect,
    Role::Developer,
    Role::Tester,
    Role::Security,
];

/// How the names of the agents' notes start.
const AGENT_NOTE: &str = "agent-";

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
enum Row {
    Heading(&'static str),
    Skill(String),
}

#[derive(Debug)]
pub struct SkillsTab {
    root: PathBuf,
    /// The role chosen at the top, an index into `ROLES`.
    pub(crate) role: usize,
    library: Vec<LibrarySkill>,
    /// Each role's agent.
    roles: BTreeMap<Role, AgentKind>,
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

    fn role(&self) -> Role {
        ROLES[self.role.min(ROLES.len() - 1)]
    }

    /// The agent of the chosen role, `claude` if harness.toml has no such role.
    fn agent(&self) -> AgentKind {
        self.roles
            .get(&self.role())
            .copied()
            .unwrap_or(AgentKind::Claude)
    }

    fn rows(&self) -> Vec<Row> {
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
    fn optional_at(&self, index: usize) -> Option<String> {
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
            KeyCode::Enter | KeyCode::Char('e') => return self.press(ButtonId::SkillEdit),
            KeyCode::Char(' ') => return self.press(ButtonId::SkillMark(self.row)),
            KeyCode::Char('n') => return self.press(ButtonId::SkillNew),
            KeyCode::Delete => return self.press(ButtonId::SkillRestore),
            _ => {}
        }
        Action::None
    }

    /// A button of the tab.
    pub fn press(&mut self, id: ButtonId) -> Action {
        match id {
            ButtonId::SkillRole(index) => self.choose_role(index),
            ButtonId::SkillEdit => {
                if let Some(skill) = self.current() {
                    return Action::Edit(skill.name.clone());
                }
            }
            ButtonId::SkillNew => return Action::New,
            ButtonId::SkillMark(index) => {
                if let Some(name) = self.optional_at(index) {
                    self.select(index);
                    return Action::Cycle(self.role(), name);
                }
            }
            ButtonId::SkillRestore => {
                if let Some(skill) = self.current() {
                    if matches!(skill.source, Source::Changed { .. }) {
                        return Action::Restore(skill.name.clone());
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Is there a skill with this name (built-in or the project's)?
    pub fn exists(&self, name: &str) -> bool {
        self.library.iter().any(|s| s.name == name)
    }

    fn status(skill: &LibrarySkill, tr: &I18n) -> (String, Style) {
        match skill.source {
            Source::BuiltIn => (tr.t("skills.built_in").to_string(), Style::new()),
            Source::Changed { outdated: false } => {
                (tr.t("skills.changed").to_string(), theme::warn())
            }
            Source::Changed { outdated: true } => {
                (tr.t("skills.outdated").to_string(), theme::bad())
            }
            Source::Own => (tr.t("skills.own").to_string(), theme::accent()),
        }
    }

    /// `[■]` always in the prompt, `[x]` read when needed, `[ ]` not chosen.
    /// The marks come from the Roles tab, with its changes not saved yet.
    fn mark(&self, name: &str, roles: &RolesTab) -> &'static str {
        match roles.settings(self.role()) {
            Some(s) if s.always_skills.iter().any(|n| n == name) => "[■]",
            Some(s) if s.skills.iter().any(|n| n == name) => "[x]",
            _ => "[ ]",
        }
    }

    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        hits: &mut Hits,
        tr: &I18n,
        roles: &RolesTab,
    ) {
        let [top, main, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

        let names: Vec<&str> = ROLES.iter().map(|r| r.as_str()).collect();
        selector(
            frame,
            top,
            hits,
            tr.t("skills.role"),
            &names,
            self.role,
            ButtonId::SkillRole,
        );

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let dim = theme::dim();
        let rows = self.rows();
        let items: Vec<ListItem> = rows
            .iter()
            .map(|row| match row {
                Row::Heading(key) => ListItem::new(Line::styled(
                    tr.t(key).to_string(),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Row::Skill(name) => {
                    let Some(skill) = self.library.iter().find(|s| s.name == *name) else {
                        return ListItem::new(Line::styled(
                            format!("{name:<22} {}", tr.t("skills.missing")),
                            theme::bad(),
                        ));
                    };
                    let (status, style) = Self::status(skill, tr);
                    if name.starts_with(AGENT_NOTE) {
                        // ● in this role's prompt, ○ the note of another agent.
                        let used = skills::agent_note(self.agent()) == name;
                        let (mark, look) = if used {
                            ("●", Style::new())
                        } else {
                            ("○", dim)
                        };
                        return ListItem::new(Line::from(vec![
                            Span::styled(format!("{mark} {name:<20} "), look),
                            Span::styled(status, if used { style } else { dim }),
                        ]));
                    }
                    let mark = if skills::is_base(name) {
                        String::new()
                    } else {
                        format!("{} ", self.mark(name, roles))
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{mark}{name:<22} ")),
                        Span::styled(status, style),
                    ]))
                }
            })
            .collect();
        let title = tr.f("skills.title", &[("role", &self.role())]);
        let block = panel(&title, true);
        let inner = block.inner(left);
        let mut state = ListState::default().with_selected(Some(self.row));
        frame.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(selected())
                .highlight_symbol("▶ "),
            left,
            &mut state,
        );
        hits.add(
            inner,
            Target::List {
                list: ListId::Skills,
                first: state.offset(),
            },
        );
        // The marks are buttons; they come after «▶ ».
        for line in 0..inner.height {
            let index = state.offset() + usize::from(line);
            if self.optional_at(index).is_some() && inner.width > 5 {
                hits.add(
                    Rect::new(inner.x + 2, inner.y + line, 3, 1),
                    Target::Button(ButtonId::SkillMark(index)),
                );
            }
        }

        match self.current() {
            Some(skill) => {
                let (status, _) = Self::status(skill, tr);
                let title = format!(" {} · {status} ", skill.name);
                let mut lines: Vec<Line> = Vec::new();
                if skill.description.is_none() {
                    lines.push(Line::styled(
                        tr.t("skills.no_description").to_string(),
                        theme::bad(),
                    ));
                }
                if skill.name.starts_with(AGENT_NOTE) {
                    let key = if skills::agent_note(self.agent()) == skill.name {
                        "skills.agent_used"
                    } else {
                        "skills.agent_unused"
                    };
                    let text = tr.f(key, &[("role", &self.role()), ("agent", &self.agent())]);
                    lines.push(Line::styled(text, dim));
                    lines.push(Line::default());
                } else if skills::is_base(&skill.name) {
                    lines.push(Line::styled(tr.t("skills.base_hint").to_string(), dim));
                    lines.push(Line::default());
                } else {
                    lines.push(Line::styled(tr.t("roles.skills_hint").to_string(), dim));
                    lines.push(Line::default());
                }
                lines.extend(reflow(&skill.text).into_iter().map(Line::from));
                frame.render_widget(
                    Paragraph::new(lines)
                        .block(panel(&title, false))
                        .wrap(Wrap { trim: false })
                        .scroll((self.scroll, 0)),
                    right,
                );
            }
            None => frame.render_widget(Paragraph::new("").block(panel("", false)), right),
        }

        let restore = self
            .current()
            .is_some_and(|s| matches!(s.source, Source::Changed { .. }));
        buttons(
            frame,
            bottom,
            hits,
            &[
                (
                    tr.t("skills.edit"),
                    ButtonId::SkillEdit,
                    self.current().is_some(),
                ),
                (tr.t("skills.new"), ButtonId::SkillNew, true),
                (tr.t("skills.restore"), ButtonId::SkillRestore, restore),
                (tr.t("roles.save"), ButtonId::Save, roles.changed()),
                (tr.t("roles.undo"), ButtonId::Undo, roles.changed()),
            ],
        );
        if roles.changed() {
            let note = tr.t("roles.unsaved");
            let width = u16::try_from(note.chars().count()).unwrap_or(0);
            let x = bottom.right().saturating_sub(width);
            frame.render_widget(
                Span::styled(note.to_string(), theme::warn()),
                Rect::new(x.max(bottom.x), bottom.y, width.min(bottom.width), 1),
            );
        }
    }
}

/// The text with the lines of each paragraph joined, so the panel wraps
/// them at its own width. Headings, lists, tables, quotes and code stay as
/// they are.
fn reflow(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut code = false;
    let mut joinable = false;
    // The header stays as it is.
    let mut header = text.starts_with("---");
    for (index, line) in text.lines().enumerate() {
        if header {
            lines.push(line.to_string());
            header = index == 0 || line.trim() != "---";
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            code = !code;
        }
        let item = trimmed.starts_with(['-', '*', '>', '+'])
            || trimmed.starts_with(|c: char| c.is_ascii_digit());
        let marker = item || trimmed.starts_with(['#', '|', '`']);
        let text_line = !code && !trimmed.is_empty() && !marker;
        match lines.last_mut() {
            Some(last) if text_line && joinable => {
                last.push(' ');
                last.push_str(trimmed);
            }
            _ => lines.push(line.to_string()),
        }
        // A paragraph or a list item goes on in its next text line.
        joinable = !code && (text_line || (item && !trimmed.starts_with("---")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::reflow;

    #[test]
    fn paragraphs_are_joined_but_headings_lists_and_code_stay() {
        let text = "---\ndescription: x\nbuiltin: 1\n---\n# Title\nOne line\nand more.\n\n- item\n  goes on\n```\na\nb\n```\n";
        assert_eq!(
            reflow(text),
            [
                "---",
                "description: x",
                "builtin: 1",
                "---",
                "# Title",
                "One line and more.",
                "",
                "- item goes on",
                "```",
                "a",
                "b",
                "```",
            ]
        );
    }
}
