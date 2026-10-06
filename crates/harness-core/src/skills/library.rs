//! Every skill a project can use, as the Skills tab lists it, and reading a
//! skill's header.

use std::fs;
use std::path::{Path, PathBuf};

use super::{built_in, built_in_names, check_name, source_of, Source, SKILLS_DIR};

/// Every skill a project can use: the built-in ones and its own files,
/// sorted by name. A file that cannot be read is listed with its problem.
pub fn library(harness_dir: &Path) -> Vec<LibrarySkill> {
    let dir = harness_dir.join(SKILLS_DIR);
    let mut names: Vec<String> = built_in_names().map(str::to_string).collect();
    if let Ok(entries) = fs::read_dir(&dir) {
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.extension().is_some_and(|e| e == "md")
                && check_name(&name).is_ok()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
    }
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = dir.join(format!("{name}.md"));
            let (text, source) = match fs::read_to_string(&path) {
                Ok(text) => {
                    let source = source_of(&name, &text);
                    (text, source)
                }
                Err(_) => (
                    built_in(&name).unwrap_or_default().to_string(),
                    Source::BuiltIn,
                ),
            };
            let description = split_header(&text).map(|(d, _)| d);
            LibrarySkill {
                name,
                description,
                path,
                text,
                source,
            }
        })
        .collect()
}

/// A skill as the Skills tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySkill {
    pub name: String,
    /// `None` when the header is broken.
    pub description: Option<String>,
    /// The project file (it exists unless the skill is built-in and unchanged).
    pub path: PathBuf,
    /// The whole text, header included.
    pub text: String,
    pub source: Source,
}

/// The value of `key` in a skill's header, such as `builtin`.
pub fn header_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            return None;
        }
        if let Some(value) = line
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            return Some(value.trim().trim_matches('"').trim().to_string());
        }
    }
    None
}

/// Splits `---\ndescription: ...\n---\nbody` into the description and the body.
pub fn split_header(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut description = None;
    for line in lines.by_ref() {
        let line = line.trim();
        if line == "---" {
            let body: Vec<&str> = lines.collect();
            let description: String = description?;
            return Some((description, body.join("\n").trim().to_string()));
        }
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches('"').trim();
            if !value.is_empty() {
                description = Some(value.to_string());
            }
        }
    }
    None
}
