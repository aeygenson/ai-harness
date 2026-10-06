//! «From catalog»: searching the official MCP registry and choosing a server.

use ratatui::crossterm::event::KeyCode;

use harness_core::mcp::registry::Entry;

use super::tab::{Action, McpCatalogButton, McpTab};

impl McpTab {
    pub fn in_catalog(&self) -> bool {
        self.catalog.is_some()
    }

    /// Opens the catalog and asks what to search.
    pub fn open_catalog(&mut self) -> Action {
        let query = self.last_query.clone();
        let catalog = self.catalog.get_or_insert_with(|| Catalog {
            query,
            ..Catalog::default()
        });
        Action::Search(catalog.query.clone())
    }

    /// A click on a line of the catalog.
    pub fn select_found(&mut self, index: usize) {
        if let Some(catalog) = &mut self.catalog {
            if index < catalog.entries.len() {
                catalog.row = index;
            }
        }
    }

    pub fn move_found(&mut self, delta: isize) {
        if let Some(catalog) = &mut self.catalog {
            catalog.move_by(delta);
        }
    }

    /// A key while the catalog is open.
    pub fn catalog_key(&mut self, key: KeyCode) -> Action {
        match key {
            KeyCode::Up | KeyCode::Char('k') => self.move_found(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_found(1),
            KeyCode::Char('/' | 's' | 'f') => return self.catalog_press(McpCatalogButton::Search),
            KeyCode::Enter | KeyCode::Char('a') => {
                return self.catalog_press(McpCatalogButton::Use)
            }
            KeyCode::Esc | KeyCode::Backspace => self.catalog = None,
            _ => {}
        }
        Action::None
    }

    /// A button of the catalog.
    pub fn catalog_press(&mut self, id: McpCatalogButton) -> Action {
        let Some(catalog) = &mut self.catalog else {
            return Action::None;
        };
        match id {
            McpCatalogButton::Search if catalog.searching => Action::None,
            McpCatalogButton::Search => Action::Search(catalog.query.clone()),
            McpCatalogButton::Use => catalog.choose(),
            McpCatalogButton::Back => {
                self.catalog = None;
                Action::None
            }
        }
    }
}

/// The catalog of the MCP registry, while it is open.
#[derive(Debug, Default)]
pub struct Catalog {
    /// The last search.
    pub query: String,
    pub entries: Vec<Entry>,
    pub row: usize,
    /// The registry is being asked.
    pub searching: bool,
    /// Why the last search failed.
    pub error: Option<String>,
}

impl Catalog {
    pub(super) fn current(&self) -> Option<&Entry> {
        self.entries.get(self.row)
    }

    /// The registry answered.
    pub fn found(&mut self, answer: Result<Vec<Entry>, String>) {
        self.searching = false;
        self.row = 0;
        match answer {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.error = Some(error);
            }
        }
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
    }

    fn choose(&self) -> Action {
        match self.current() {
            Some(Entry {
                offer: Some(offer), ..
            }) => Action::Use(offer.clone()),
            Some(entry) => Action::Unusable(entry.unusable.clone().unwrap_or_default()),
            None => Action::None,
        }
    }
}
