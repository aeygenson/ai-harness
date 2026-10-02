//! The Retro tab: lessons learned from the whole history of the project.
//!
//! ```text
//! ┌ Retrospectives ───┐┌ 003 · all · 2026-10-02 ─────────────────────────────┐
//! │> 003  2026-10-02  ││ What went well ...                                  │
//! │  002  2026-09-30  ││                                                     │
//! ├ Proposals ────────┤│ ── Statistics ──                                    │
//! │> [x] 1 Teach ...  ││ # Retrospective: all                                │
//! │  ✓   2 Check ...  ││ ...                                                 │
//! └───────────────────┘└─────────────────────────────────────────────────────┘
//!  [ Generate ] [ Open in Zed ] [ Choose ] [ Apply chosen ]
//! ```
//!
//! «Generate» does what `harness retro --all --suggest` does, in the
//! background: the statistics of every task, then the `[retro]` agent reads
//! the whole history (every handoff, note and agent log) and writes its
//! lessons and its proposals for the skills. The text opens in Zed for
//! editing. Proposals Lisa chooses are applied after a confirmation.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use harness_agents::build::BuildError;
use harness_agents::{process, AnyAgent};
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::proposals::Proposal;
use harness_core::retro_ops::{self, RetroInfo};
use harness_core::suggest::{self, RETRO_MD};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::i18n::I18n;
use crate::runner::{push_line, readable};
use crate::tasks::draw_list;
use crate::ui::{buttons, panel, ButtonId, Hits, ListId};

/// Builds the agent of `[retro]`: `retro_agent`, or a mock in tests.
pub type RetroBuilder = fn(&Config) -> Result<AnyAgent, BuildError>;

/// What the tab asks the App to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Generate,
    /// Open the retrospective's text in the editor: its number and file.
    Open(String, PathBuf),
    /// Ask before applying these proposals of the retrospective in `dir`.
    Apply(PathBuf, Vec<u32>),
    /// Something cannot be done now; the key of the message why.
    Say(&'static str),
}

/// Which list the keys move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Retros,
    Proposals,
}

/// «Generate», while the agent works.
#[derive(Debug)]
struct Generating {
    log: Receiver<String>,
    done: Receiver<Result<String, String>>,
}

#[derive(Debug)]
pub struct RetroTab {
    root: PathBuf,
    pub(crate) list: Vec<RetroInfo>,
    /// The selected retrospective.
    pub(crate) row: usize,
    /// The selected proposal of it.
    pub(crate) proposal: usize,
    pub(crate) focus: Focus,
    /// The proposals chosen to apply, of the selected retrospective.
    pub(crate) chosen: BTreeSet<u32>,
    pub(crate) scroll: u16,
    /// harness.toml, to show what a proposal changes.
    config: Option<Config>,
    generating: Option<Generating>,
    /// What the agent printed during the last «Generate».
    log: VecDeque<String>,
}

impl RetroTab {
    pub fn load(root: &Path) -> Self {
        let mut tab = Self {
            root: root.to_path_buf(),
            list: Vec::new(),
            row: 0,
            proposal: 0,
            focus: Focus::Retros,
            chosen: BTreeSet::new(),
            scroll: 0,
            config: None,
            generating: None,
            log: VecDeque::new(),
        };
        tab.reload();
        tab
    }

    /// Reads the saved retrospectives again, keeping the selected one.
    pub fn reload(&mut self) {
        let keep = self.current().map(|r| r.number.clone());
        self.list = Repo::open(&self.root)
            .map(|repo| retro_ops::list(&repo))
            .unwrap_or_default();
        self.config = Config::load(&self.root.join(HARNESS_DIR)).ok();
        match keep {
            Some(number) => self.select_number(&number),
            None => self.row = 0,
        }
        let applied = self
            .current()
            .map(|r| r.applied.clone())
            .unwrap_or_default();
        self.chosen.retain(|id| !applied.contains(id));
    }

    /// Selects the retrospective `number`, if there is one.
    pub fn select_number(&mut self, number: &str) {
        if let Some(at) = self.list.iter().position(|r| r.number == number) {
            if at != self.row {
                self.row = at;
                self.changed_retro();
            }
        }
    }

    pub fn current(&self) -> Option<&RetroInfo> {
        self.list.get(self.row)
    }

    /// The proposals of the selected retrospective that could be read.
    fn proposals(&self) -> &[Proposal] {
        match self.current().and_then(|r| r.proposals.as_ref()) {
            Some(Ok(file)) => &file.proposals,
            _ => &[],
        }
    }

    fn current_proposal(&self) -> Option<&Proposal> {
        self.proposals().get(self.proposal)
    }

    fn applied(&self, id: u32) -> bool {
        self.current().is_some_and(|r| r.applied.contains(&id))
    }

    fn changed_retro(&mut self) {
        self.proposal = 0;
        self.chosen.clear();
        self.scroll = 0;
    }

    pub fn is_generating(&self) -> bool {
        self.generating.is_some()
    }

    /// A click on a retrospective.
    pub fn select(&mut self, index: usize) {
        self.focus = Focus::Retros;
        if index < self.list.len() && index != self.row {
            self.row = index;
            self.changed_retro();
        }
    }

    /// A click on a proposal.
    pub fn select_proposal(&mut self, index: usize) {
        if index < self.proposals().len() {
            self.focus = Focus::Proposals;
            self.proposal = index;
            self.scroll = 0;
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        match self.focus {
            Focus::Retros => {
                let row = self
                    .row
                    .saturating_add_signed(delta)
                    .min(self.list.len().saturating_sub(1));
                if row != self.row {
                    self.row = row;
                    self.changed_retro();
                }
            }
            Focus::Proposals => {
                self.proposal = self
                    .proposal
                    .saturating_add_signed(delta)
                    .min(self.proposals().len().saturating_sub(1));
                self.scroll = 0;
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

    /// Choose the selected proposal, or take it out of the chosen.
    pub fn toggle(&mut self) -> Action {
        let Some(id) = self.current_proposal().map(|p| p.id) else {
            return Action::None;
        };
        if self.applied(id) {
            return Action::Say("retro.already_applied");
        }
        if !self.chosen.remove(&id) {
            self.chosen.insert(id);
        }
        Action::None
    }

    pub fn on_key(&mut self, key: KeyCode) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Tab | KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') => {
                self.focus = match self.focus {
                    Focus::Proposals => Focus::Retros,
                    Focus::Retros if !self.proposals().is_empty() => Focus::Proposals,
                    Focus::Retros => Focus::Retros,
                };
                self.scroll = 0;
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.focus == Focus::Proposals => {
                return self.toggle();
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('g') => return self.press(ButtonId::RetroGenerate),
            KeyCode::Char('e') => return self.press(ButtonId::RetroOpen),
            KeyCode::Char('a') => return self.press(ButtonId::RetroApply),
            _ => {}
        }
        Action::None
    }

    pub fn press(&mut self, id: ButtonId) -> Action {
        match id {
            ButtonId::RetroGenerate if self.generating.is_none() => Action::Generate,
            ButtonId::RetroOpen => match self.current() {
                Some(retro) if retro.retro.is_some() => {
                    Action::Open(retro.number.clone(), retro.dir.join(RETRO_MD))
                }
                _ => Action::None,
            },
            ButtonId::RetroToggle => {
                self.focus = Focus::Proposals;
                self.toggle()
            }
            ButtonId::RetroApply if !self.chosen.is_empty() => match self.current() {
                Some(retro) => {
                    Action::Apply(retro.dir.clone(), self.chosen.iter().copied().collect())
                }
                None => Action::None,
            },
            _ => Action::None,
        }
    }

    /// Starts «Generate» in the background; the agent writes in `language`.
    pub fn generate(&mut self, builder: RetroBuilder, language: &str) {
        if self.generating.is_some() {
            return;
        }
        let (log, log_rx) = channel();
        let (done, done_rx) = channel();
        let (root, language) = (self.root.clone(), language.to_string());
        thread::spawn(move || {
            process::set_live_log(Some(log));
            let result = generate(&root, builder, &language);
            process::set_live_log(None);
            let _ = done.send(result);
        });
        self.log.clear();
        self.scroll = 0;
        self.generating = Some(Generating {
            log: log_rx,
            done: done_rx,
        });
    }

    /// Takes what the agent printed; once it is done, the message to show.
    pub fn tick(&mut self, tr: &I18n) -> Option<(String, bool)> {
        let generating = self.generating.as_ref()?;
        for line in generating.log.try_iter() {
            if let Some(line) = readable(&line) {
                push_line(&mut self.log, line);
            }
        }
        let result = match generating.done.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the retrospective stopped".into())
            }
        };
        self.generating = None;
        self.reload();
        // The newest one is shown, whether the agent managed or not.
        if let Some(newest) = self.list.first().map(|r| r.number.clone()) {
            self.select_number(&newest);
        }
        self.focus = Focus::Retros;
        Some(match result {
            Ok(number) => (tr.f("retro.generated", &[("number", &number)]), false),
            Err(error) => (tr.f("retro.failed", &[("error", &error)]), true),
        })
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, hits: &mut Hits, tr: &I18n) {
        let [main, bottom] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(main);
        let proposals = self.proposals();
        let [retros_area, proposals_area] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(left);
        let dim = Style::new().fg(Color::DarkGray);
        let green = Style::new().fg(Color::Green);

        let items: Vec<ListItem> = self
            .list
            .iter()
            .map(|retro| {
                let scope = if retro.scope == "all" {
                    tr.t("retro.all").to_string()
                } else {
                    retro.scope.clone()
                };
                let mut spans = vec![Span::raw(format!("{}  ", retro.number))];
                spans.push(Span::raw(format!(
                    "{:<11}",
                    retro.date.clone().unwrap_or_default()
                )));
                spans.push(Span::styled(scope, dim));
                if retro.retro.is_none() {
                    spans.push(Span::styled(
                        format!(" · {}", tr.t("retro.stats_only")),
                        dim,
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        draw_list(
            frame,
            hits,
            retros_area,
            ListId::Retros,
            tr.t("retro.list"),
            items,
            self.row,
            self.focus == Focus::Retros,
        );

        let items: Vec<ListItem> = proposals
            .iter()
            .map(|p| {
                let (mark, style) = if self.applied(p.id) {
                    ("✓  ", green)
                } else if self.chosen.contains(&p.id) {
                    ("[x]", Style::new())
                } else {
                    ("[ ]", Style::new())
                };
                ListItem::new(Line::styled(
                    format!("{mark} {} {}", p.id, p.summary),
                    style,
                ))
            })
            .collect();
        let empty = items.is_empty();
        draw_list(
            frame,
            hits,
            proposals_area,
            ListId::RetroProposals,
            tr.t("retro.proposals"),
            items,
            self.proposal,
            self.focus == Focus::Proposals,
        );
        if empty {
            let note = match self.current().and_then(|r| r.proposals.as_ref()) {
                Some(Err(error)) => error.clone(),
                Some(Ok(_)) => tr.t("retro.no_proposals").to_string(),
                None if self.current().is_some_and(|r| r.retro.is_none()) => {
                    tr.t("retro.no_agent").to_string()
                }
                None => tr.t("retro.no_proposals").to_string(),
            };
            let inner = panel("", false).inner(proposals_area);
            frame.render_widget(
                Paragraph::new(Line::styled(note, dim)).wrap(Wrap { trim: false }),
                inner,
            );
        }

        let (title, lines) = self.text(tr);
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(&title, false))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            right,
        );

        let generating = self.generating.is_some();
        let has_text = self.current().is_some_and(|r| r.retro.is_some());
        let can_choose = self.current_proposal().is_some_and(|p| !self.applied(p.id));
        let choose = match self.current_proposal() {
            Some(p) if self.chosen.contains(&p.id) => tr.t("retro.unchoose"),
            _ => tr.t("retro.choose"),
        };
        let apply = tr.f("retro.apply", &[("count", &self.chosen.len())]);
        buttons(
            frame,
            bottom,
            hits,
            &[
                (tr.t("retro.generate"), ButtonId::RetroGenerate, !generating),
                (tr.t("retro.open"), ButtonId::RetroOpen, has_text),
                (choose, ButtonId::RetroToggle, can_choose),
                (&apply, ButtonId::RetroApply, !self.chosen.is_empty()),
            ],
        );
    }

    /// The right side: the live log while generating, a proposal with what
    /// it changes, or the retrospective with its statistics.
    fn text(&self, tr: &I18n) -> (String, Vec<Line<'static>>) {
        let dim = Style::new().fg(Color::DarkGray);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        if self.generating.is_some() || (self.list.is_empty() && !self.log.is_empty()) {
            let mut lines = vec![Line::styled(tr.t("retro.generating").to_string(), dim)];
            lines.extend(self.log.iter().map(|l| Line::from(l.clone())));
            return (format!(" {} ", tr.t("retro.agent")), lines);
        }
        let Some(retro) = self.current() else {
            let lines = tr
                .t("retro.empty")
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect();
            return (String::new(), lines);
        };
        if self.focus == Focus::Proposals {
            if let Some(proposal) = self.current_proposal() {
                let harness_dir = self.root.join(HARNESS_DIR);
                // After applying, the files already look like the proposal:
                // a diff would only say "already like this".
                let text = match &self.config {
                    _ if self.applied(proposal.id) => format!(
                        "{}\n{}\n\n✓ {}\n",
                        proposal.summary,
                        proposal.reason,
                        tr.f("retro.was_applied", &[("name", &proposal.skill)])
                    ),
                    Some(config) => proposal.describe(&harness_dir, config),
                    None => format!("{}\n{}\n", proposal.summary, proposal.reason),
                };
                let lines = text
                    .lines()
                    .map(|line| {
                        let style = if line.starts_with("+ ") {
                            Style::new().fg(Color::Green)
                        } else if line.starts_with("- ") {
                            Style::new().fg(Color::LightRed)
                        } else {
                            Style::new()
                        };
                        Line::styled(line.to_string(), style)
                    })
                    .collect();
                return (
                    format!(" {} {} ", tr.t("retro.proposal"), proposal.id),
                    lines,
                );
            }
        }
        let scope = if retro.scope == "all" {
            tr.t("retro.all").to_string()
        } else {
            retro.scope.clone()
        };
        let title = match &retro.date {
            Some(date) => format!(" {} · {scope} · {date} ", retro.number),
            None => format!(" {} · {scope} ", retro.number),
        };
        let mut lines: Vec<Line<'static>> = match &retro.retro {
            Some(text) => text.lines().map(|l| Line::from(l.to_string())).collect(),
            None => vec![Line::styled(tr.t("retro.no_agent").to_string(), dim)],
        };
        if let Some(stats) = &retro.stats {
            lines.push(Line::default());
            lines.push(Line::styled(format!("── {} ──", tr.t("retro.stats")), bold));
            lines.extend(stats.lines().map(|l| Line::styled(l.to_string(), dim)));
        }
        (title, lines)
    }
}

/// «Generate»: the statistics of every task, then the agent's lessons and
/// proposals. Returns the number of the new retrospective.
fn generate(root: &Path, builder: RetroBuilder, language: &str) -> Result<String, String> {
    let text = |e: &dyn std::fmt::Display| e.to_string();
    let repo = Repo::open(root).map_err(|e| text(&e))?;
    let config = Config::load(&root.join(HARNESS_DIR)).map_err(|e| text(&e))?;
    // Everything the agent needs is checked before anything is saved.
    retro_ops::check_clean(&repo).map_err(|e| text(&e))?;
    let agent = builder(&config).map_err(|e| text(&e))?;
    let (dir, stats) = retro_ops::save_stats(&repo, None, Some(&config)).map_err(|e| text(&e))?;
    let number = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    runtime
        .block_on(suggest::suggest(
            &repo, &dir, &stats, &config, &agent, language,
        ))
        .map_err(|e| text(&e))?;
    Ok(number)
}
