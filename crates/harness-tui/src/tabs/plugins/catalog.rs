//! «From catalog» and «Catalogs» on the Plugins tab.
//!
//! ```text
//!  Plugins in the catalogs · search: review          Agent: [ claude ] [ codex ] [ all ]
//! ┌ Found 12 ───────────────────────────┐┌ code-review · claude-plugins-official ─┐
//! │> code-review      claude  added     ││ Reviews the diff for bugs and style.    │
//! │  pr-review        claude            ││ From: a folder in the catalog           │
//! │  npm-thing        claude  cannot    ││                                         │
//! └─────────────────────────────────────┘└─────────────────────────────────────────┘
//!  [ Search ] [ Add ] [ Add and give to the developer ] [ Catalogs ] [ Back ]
//! ```
//!
//! The list is every plugin of every added catalog, read from their copies
//! in `~/.harness/marketplaces/`, so searching is instant. Adding downloads
//! the plugin (in the background) and copies it into the project; nothing
//! of a catalog reaches an agent until then. The «Catalogs» screen adds,
//! updates and removes catalogs; the official Claude catalog is added by
//! itself the first time the catalog opens with none.

use std::path::Path;

use harness_core::config::AgentKind;
use harness_core::plugins::catalog::{Entry, Source};
use harness_core::plugins::ops::{self, CatalogInfo};

use crate::ui::i18n::I18n;

/// The catalog anyone starts with: Anthropic's plugins for Claude Code.
pub const OFFICIAL: &str = "anthropics/claude-plugins-official";

/// The agent filter: index into these; `None` (all agents) last.
pub const FILTERS: [Option<AgentKind>; 3] = [Some(AgentKind::Claude), Some(AgentKind::Codex), None];

/// The plugins of all catalogs, while «From catalog» is open.
#[derive(Debug, Default)]
pub struct CatalogView {
    pub entries: Vec<Entry>,
    /// Catalogs that cannot be read.
    pub errors: Vec<String>,
    /// Words to look for in the name and description.
    pub query: String,
    /// Index into `FILTERS`.
    pub filter: usize,
    pub row: usize,
}

impl CatalogView {
    /// Reads every added catalog; `agent` sets the filter (all agents when
    /// it has no plugins).
    pub fn load(home: Option<&Path>, agent: Option<AgentKind>) -> Self {
        let mut view = Self {
            filter: FILTERS
                .iter()
                .position(|f| *f == agent)
                .unwrap_or(FILTERS.len() - 1),
            ..Self::default()
        };
        view.reload(home);
        view
    }

    pub fn reload(&mut self, home: Option<&Path>) {
        self.entries.clear();
        self.errors.clear();
        let Some(home) = home else {
            return;
        };
        match ops::catalogs(home) {
            Ok(catalogs) => {
                for catalog in catalogs {
                    match catalog.entries {
                        Ok(entries) => self.entries.extend(entries),
                        Err(error) => self.errors.push(format!("{}: {error}", catalog.name)),
                    }
                }
            }
            Err(error) => self.errors.push(error.to_string()),
        }
        self.row = self.row.min(self.shown().len().saturating_sub(1));
    }

    /// The entries that match the filter and the search, those that can be
    /// added first.
    pub fn shown(&self) -> Vec<&Entry> {
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let agent = FILTERS[self.filter.min(FILTERS.len() - 1)];
        let mut shown: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| agent.is_none_or(|agent| e.agent == agent))
            .filter(|e| {
                let text = format!("{} {}", e.name, e.description).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
        shown.sort_by_key(|e| unusable(e).is_some());
        shown
    }

    pub fn current(&self) -> Option<&Entry> {
        self.shown().get(self.row).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.shown().len();
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    pub fn select(&mut self, index: usize) {
        if index < self.shown().len() {
            self.row = index;
        }
    }

    pub fn choose_filter(&mut self, index: usize) {
        if index < FILTERS.len() {
            self.filter = index;
            self.row = 0;
        }
    }
}

/// Why a catalog plugin cannot be added yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    /// It comes from a kind of source the harness cannot download.
    Source(String),
    /// Its name could not be a folder name.
    Name,
}

impl Unusable {
    /// What is not supported, in Lisa's language.
    pub fn text(&self, tr: &I18n) -> String {
        match self {
            Unusable::Source(kind) => tr.f("plugins.unusable_source", &[("kind", kind)]),
            Unusable::Name => tr.t("plugins.unusable_name").to_string(),
        }
    }
}

/// Why an entry cannot be added, if it cannot.
pub fn unusable(entry: &Entry) -> Option<Unusable> {
    match &entry.source {
        Source::Unsupported(kind) => Some(Unusable::Source(kind.clone())),
        _ if !harness_core::mcp::is_simple_name(&entry.name) => Some(Unusable::Name),
        _ => None,
    }
}

/// The added catalogs, while «Catalogs» is open.
#[derive(Debug, Default)]
pub struct CatalogsView {
    pub list: Vec<CatalogInfo>,
    pub row: usize,
    pub error: Option<String>,
}

impl CatalogsView {
    pub fn load(home: Option<&Path>) -> Self {
        let mut view = Self::default();
        view.reload(home);
        view
    }

    pub fn reload(&mut self, home: Option<&Path>) {
        match home.map(ops::catalogs) {
            Some(Ok(list)) => {
                self.list = list;
                self.error = None;
            }
            Some(Err(error)) => self.error = Some(error.to_string()),
            None => self.list.clear(),
        }
        self.row = self.row.min(self.list.len().saturating_sub(1));
    }

    pub fn current(&self) -> Option<&CatalogInfo> {
        self.list.get(self.row)
    }

    pub fn move_by(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.list.len().saturating_sub(1));
    }

    pub fn select(&mut self, index: usize) {
        if index < self.list.len() {
            self.row = index;
        }
    }
}
