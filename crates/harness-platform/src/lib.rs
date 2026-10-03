//! Everything the harness does differently on Linux, macOS and Windows lives
//! here, so the rest of the code never asks which system it runs on.
//!
//! - [`home`]: the home folder and `~/.harness`;
//! - [`env`]: the variables an agent inherits, and the ones a server may not set;
//! - [`private`]: files and folders only Lisa can read;
//! - [`process`]: stopping a program together with everything it started;
//! - [`open`]: opening an address in the browser;
//! - [`program`]: finding a program by name (`codex.cmd` on Windows);
//! - [`path`]: paths written with `/`, as git writes them.
//!
//! Each function has one small version per system (`#[cfg(...)]`); outside
//! this crate there is no `#[cfg(unix)]` in the harness's own logic.

pub mod env;
pub mod home;
pub mod open;
pub mod path;
pub mod private;
pub mod process;
pub mod program;
