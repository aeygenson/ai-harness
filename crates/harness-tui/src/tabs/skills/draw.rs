//! Drawing the Skills tab: the skill list, marks and the chosen skill's text.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use harness_core::skills::{self, LibrarySkill, Source};

use super::tab::{Row, SkillButton, SkillsTab, AGENT_NOTE, ROLES};
use crate::tabs::roles::RolesTab;
use crate::ui::i18n::I18n;
use crate::ui::theme;
use crate::ui::{buttons, panel, selected, selector, ButtonId, Hits, ListId, Target};

impl SkillsTab {
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
    pub(super) fn mark(&self, name: &str, roles: &RolesTab) -> &'static str {
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
            |index| ButtonId::Skill(SkillButton::Role(index)),
        );

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let items = self.list_items(roles, tr);
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
                    Target::Button(ButtonId::Skill(SkillButton::Mark(index))),
                );
            }
        }

        self.draw_skill_text(frame, right, tr);

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
                    ButtonId::Skill(SkillButton::Edit),
                    self.current().is_some(),
                ),
                (tr.t("skills.new"), ButtonId::Skill(SkillButton::New), true),
                (
                    tr.t("skills.restore"),
                    ButtonId::Skill(SkillButton::Restore),
                    restore,
                ),
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

    /// The rows of the skill list: headings, and skills with their mark and status.
    fn list_items(&self, roles: &RolesTab, tr: &I18n) -> Vec<ListItem<'static>> {
        let dim = theme::dim();
        self.rows()
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
            .collect()
    }

    /// The selected skill's text on the right, with a hint on how the role uses it.
    fn draw_skill_text(&self, frame: &mut Frame, right: Rect, tr: &I18n) {
        let dim = theme::dim();
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
