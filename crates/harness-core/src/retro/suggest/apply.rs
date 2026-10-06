//! Applying the proposals Lisa picked: skill files and the skill lists in harness.toml.

use std::fs;
use std::path::{Path, PathBuf};

use super::ApplyError;
use crate::config::edit;
use crate::config::{Config, CONFIG_FILE};
use crate::retro::proposals::{FileChange, ProposalsFile, SkillList};
use crate::skills::Skills;

/// Applies the chosen proposals: writes the skill files and adds the skills to
/// the roles in harness.toml. If the result does not load, everything is put
/// back. Returns the changed files.
pub fn apply(
    harness_dir: &Path,
    proposals: &ProposalsFile,
    ids: &[u32],
) -> Result<Vec<PathBuf>, ApplyError> {
    let config_path = harness_dir.join(CONFIG_FILE);
    let old_config = fs::read_to_string(&config_path).map_err(|source| ApplyError::Io {
        path: config_path.clone(),
        source,
    })?;
    let config = Config::load(harness_dir)?;

    // Check everything before changing anything.
    let mut chosen = Vec::new();
    for &id in ids {
        let proposal = proposals.get(id).ok_or(ApplyError::UnknownId(id))?;
        proposal.check(harness_dir, &config)?;
        chosen.push(proposal);
    }
    let mut text = old_config.clone();
    for proposal in &chosen {
        for given in proposal.missing_roles(&config) {
            let always = given.list == SkillList::AlwaysSkills;
            text = edit::add_role_skill(&text, given.role, &proposal.skill, always)?;
        }
    }

    // (path, old text or None if the file is new), to undo a failed change.
    let mut backups: Vec<(PathBuf, Option<String>)> = Vec::new();
    let mut result = Ok(());
    for proposal in &chosen {
        let path = proposal.skill_path(harness_dir);
        let (FileChange::New | FileChange::Changed { .. }, Some(content)) =
            (proposal.file_change(harness_dir), &proposal.content)
        else {
            continue;
        };
        if backups.iter().any(|(p, _)| p == &path) {
            // Two chosen proposals write the same file: the later one wins.
        } else {
            backups.push((path.clone(), fs::read_to_string(&path).ok()));
        }
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, content));
        if let Err(source) = written {
            result = Err(ApplyError::Io { path, source });
            break;
        }
    }
    if result.is_ok() && text != old_config {
        backups.push((config_path.clone(), Some(old_config)));
        if let Err(source) = fs::write(&config_path, &text) {
            result = Err(ApplyError::Io {
                path: config_path.clone(),
                source,
            });
        }
    }
    if result.is_ok() {
        result = Config::load(harness_dir)
            .map_err(ApplyError::from)
            .and_then(|config| Skills::load(harness_dir, &config).map_err(ApplyError::from))
            .map(|_| ());
    }
    if let Err(error) = result {
        for (path, old) in backups {
            let _ = match old {
                Some(text) => fs::write(&path, text),
                None => fs::remove_file(&path),
            };
        }
        return Err(error);
    }
    Ok(backups.into_iter().map(|(path, _)| path).collect())
}
