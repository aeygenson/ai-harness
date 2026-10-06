//! Measuring a role's changes, so a huge change (a build folder, downloaded
//! packages) is never committed.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The changed paths (as `git status` lists them, relative to `root`) that
/// are bigger than `limit`, with their sizes. A folder counts the files under
/// it; only the most exact path is named: `app/node_modules`, not `app` too.
/// `.` stands for the change as a whole, when no single part is too big.
pub fn oversized_changes(root: &Path, files: &[String], limit: u64) -> Vec<(String, u64)> {
    // The size of every changed path and of every folder above it; "" is the
    // whole change.
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for file in files {
        let size = size_of(&root.join(file));
        if size == 0 {
            continue;
        }
        let path = file.trim_end_matches('/');
        *sizes.entry(String::new()).or_default() += size;
        for (i, c) in path.char_indices() {
            if c == '/' {
                *sizes.entry(path[..i].to_string()).or_default() += size;
            }
        }
        *sizes.entry(path.to_string()).or_default() += size;
    }
    let over: Vec<&String> = sizes
        .iter()
        .filter(|(_, size)| **size > limit)
        .map(|(path, _)| path)
        .collect();
    let inside = |inner: &str, outer: &str| {
        outer.is_empty() && !inner.is_empty() || inner.starts_with(&format!("{outer}/"))
    };
    over.iter()
        .filter(|outer| !over.iter().any(|inner| inside(inner, outer)))
        .map(|path| {
            let name = if path.is_empty() { "." } else { path.as_str() };
            (name.to_string(), sizes[*path])
        })
        .collect()
}

/// Bytes in a file, or in all files under a folder. Links are not followed.
pub(super) fn size_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}
