//! Everything the harness does differently on Linux, macOS and Windows lives
//! here, so the rest of the code never asks which system it runs on.
//!
//! - [`home`]: the home folder and `~/.harness`;
//! - [`env`]: the variables an agent inherits, and the ones a server may not set;
//! - [`private`]: files and folders only the user can read;
//! - [`process`]: stopping a program with everything it started, or becoming it;
//! - [`file`]: making a symbolic link, removing what an agent made hard to remove;
//! - [`lock`]: a lock file only one running program holds (one TUI window);
//! - [`run_mark`]: finding the processes an agent run started that left its group;
//! - [`stop`]: noticing that the system asks the harness to stop (terminal closed);
//! - [`open`]: opening an address in the browser;
//! - [`program`]: finding a program by name (`codex.cmd` on Windows);
//! - [`path`]: paths written with `/`, as git writes them;
//! - [`editor`]: opening a file in Zed, `$EDITOR` or the system's editor;
//! - [`folder_dialog`]: the system's «choose a folder» window;
//! - [`terminal`]: reading a secret without echo, the text-selection key;
//! - [`input`]: waiting for a key, and noticing that the terminal is closed.
//!
//! Each function has one small version per system (`#[cfg(...)]`); outside
//! this crate there is no `#[cfg(unix)]` in the harness's own logic.

pub mod editor;
pub mod env;
pub mod file;
pub mod folder_dialog;
pub mod home;
pub mod input;
pub mod lock;
pub mod open;
pub mod path;
pub mod private;
pub mod process;
pub mod program;
pub mod run_mark;
pub mod stop;
pub mod terminal;
