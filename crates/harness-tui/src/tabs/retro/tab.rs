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
use std::sync::mpsc::Receiver;

use ratatui::crossterm::event::KeyCode;

use harness_agents::build::BuildError;
use harness_agents::AnyAgent;
use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::retro::ops::{self, RetroInfo};
use harness_core::retro::proposals::Proposal;
use harness_core::retro::suggest::RETRO_MD;

use crate::tabs::tasks::runner::Background;

/// Builds the agent of `[retro]`: `retro_agent`, or a mock in tests.
pub type RetroBuilder = fn(&Config) -> Result<AnyAgent, BuildError>;

/// A button of the Retro tab, clicked or chosen with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetroButton {
    /// A new retrospective of the whole project.
    Generate,
    /// Open the retrospective's text in the editor.
    Open,
    /// Choose the selected proposal, or take it out.
    Toggle,
    /// Apply the chosen proposals.
    Apply,
}

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
pub(super) struct Generating {
    pub(super) log: Receiver<String>,
    pub(super) done: Receiver<Result<String, String>>,
    /// Kept to stop the agent when this is dropped.
    pub(super) _background: Background,
}

#[derive(Debug)]
pub struct RetroTab {
    pub(super) root: PathBuf,
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
    pub(super) config: Option<Config>,
    pub(super) generating: Option<Generating>,
    /// What the agent printed during the last «Generate».
    pub(super) log: VecDeque<String>,
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
            .map(|repo| ops::list(&repo))
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
    pub(super) fn proposals(&self) -> &[Proposal] {
        match self.current().and_then(|r| r.proposals.as_ref()) {
            Some(Ok(file)) => &file.proposals,
            _ => &[],
        }
    }

    pub(super) fn current_proposal(&self) -> Option<&Proposal> {
        self.proposals().get(self.proposal)
    }

    pub(super) fn applied(&self, id: u32) -> bool {
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
                    Focus::Retros if !self.proposals().is_empty() => Focus::Proposals,
                    // Without proposals the focus stays on «Retros».
                    Focus::Proposals | Focus::Retros => Focus::Retros,
                };
                self.scroll = 0;
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.focus == Focus::Proposals => {
                return self.toggle();
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('g') => return self.press(RetroButton::Generate),
            KeyCode::Char('e') => return self.press(RetroButton::Open),
            KeyCode::Char('a') => return self.press(RetroButton::Apply),
            _ => {}
        }
        Action::None
    }

    /// A button of the tab.
    pub fn press(&mut self, id: RetroButton) -> Action {
        match id {
            RetroButton::Generate if self.generating.is_some() => Action::None,
            RetroButton::Generate => Action::Generate,
            RetroButton::Open => match self.current() {
                Some(retro) if retro.retro.is_some() => {
                    Action::Open(retro.number.clone(), retro.dir.join(RETRO_MD))
                }
                _ => Action::None,
            },
            RetroButton::Toggle => {
                self.focus = Focus::Proposals;
                self.toggle()
            }
            RetroButton::Apply if self.chosen.is_empty() => Action::None,
            RetroButton::Apply => match self.current() {
                Some(retro) => {
                    Action::Apply(retro.dir.clone(), self.chosen.iter().copied().collect())
                }
                None => Action::None,
            },
        }
    }
}
