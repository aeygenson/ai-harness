//! Small helpers for reading and writing the files of a task folder. History
//! is never overwritten.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

use super::StoreError;
use crate::task::handoff::Handoff;
use crate::text;

pub(super) fn io_error(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn create_dir(path: &Path) -> Result<(), StoreError> {
    fs::create_dir(path).map_err(|e| io_error(path, e))
}

/// Writes a file that must not exist yet: history is never overwritten.
pub(super) fn write_new(path: &Path, contents: &str) -> Result<(), StoreError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_error(path, e))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| io_error(path, e))
}

pub(super) fn to_json<T: Serialize>(path: &Path, value: &T) -> Result<String, StoreError> {
    serde_json::to_string_pretty(value).map_err(|source| StoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// A handoff written by an agent, with its texts made safe to show.
pub(super) fn read_handoff(path: &Path) -> Result<Handoff, StoreError> {
    let mut handoff: Handoff = read_json(path)?;
    handoff.make_text_safe();
    Ok(handoff)
}

/// The agent's `notes.md`, safe to show; missing notes are an empty text.
pub(super) fn read_notes(path: &Path) -> String {
    text::safe(&fs::read_to_string(path).unwrap_or_default())
}

pub(super) fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    write_new(path, &to_json(path, value)?)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let text = fs::read_to_string(path).map_err(|e| io_error(path, e))?;
    serde_json::from_str(&text).map_err(|source| StoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Sub-folders of `dir`, sorted by name. Numbered names sort in time order.
pub(super) fn sorted_subdirs(dir: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_error(dir, e))? {
        let path = entry.map_err(|e| io_error(dir, e))?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}
