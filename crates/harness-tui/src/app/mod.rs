//! The work of the whole program rather than of one tab: the event loop,
//! keyboard and mouse, buttons, forms, background jobs and drawing the screen.
//! Each file adds methods to `App`.

pub(crate) mod background;
mod draw;
mod edits;
mod forms;
mod keyboard;
mod mouse;
mod press;
mod setup;
pub(crate) mod splash;
pub(crate) mod terminal;
