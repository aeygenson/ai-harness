//! The Plugins tab: plugins from catalogs, which role gets which, and updates.
//!
//! `tab.rs` holds the tab's state; `events.rs` (keys and buttons), `draw.rs`
//! and `details.rs` (drawing) add to the same `PluginsTab`. `catalog.rs` holds
//! «From catalog» and «Catalogs», drawn by `catalog_draw.rs` and
//! `catalogs_draw.rs`; `actions.rs` and `jobs.rs` add the work behind the
//! tab to `App`.

mod actions;
pub(crate) mod catalog;
mod catalog_draw;
mod catalogs_draw;
mod details;
mod draw;
mod events;
mod jobs;
mod tab;

pub(crate) use tab::{Action, PluginButton, PluginCatalogButton, PluginsTab};
