//! Skills: short instruction files the roles can use.
//!
//! All skills of a project live in one folder, `.harness/skills/<name>.md`, and each
//! role lists in `harness.toml` which of them it gets:
//!
//! ```toml
//! [roles.developer]
//! agent = "claude"
//! skills = ["rust-errors"]          # the agent reads the file when it needs it
//! always_skills = ["idiomatic-rust"] # the whole file goes into the prompt
//! ```
//!
//! A skill file starts with a short header that describes it:
//!
//! ```markdown
//! ---
//! description: How to handle errors in Rust with thiserror and anyhow.
//! ---
//! The instructions themselves...
//! ```
//!
//! Skills are plain files, not a feature of one agent, so they work the same way
//! with every agent. Nothing is taken from the user's home folder.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::handoff::Role;

/// The folder with the skills, inside `.harness/`.
pub const SKILLS_DIR: &str = "skills";

/// One skill file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    /// One line from the file's header; it goes into the prompt's skill list.
    pub description: String,
    pub path: PathBuf,
    /// The instructions, without the header.
    pub body: String,
}

/// The skills of one role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleSkills {
    /// Listed in the prompt; the agent reads a file when it needs it.
    pub on_demand: Vec<Skill>,
    /// Put into the prompt in full.
    pub always: Vec<Skill>,
}

impl RoleSkills {
    pub fn is_empty(&self) -> bool {
        self.on_demand.is_empty() && self.always.is_empty()
    }
}

/// The skills of every role in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skills {
    roles: BTreeMap<Role, RoleSkills>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error(
        "skill name {0:?} is not allowed; use lowercase letters, digits and '-', \
         for example \"rust-errors\""
    )]
    BadName(String),
    #[error("the {role:?} role uses skill {name:?}, but {path} does not exist")]
    Missing {
        role: Role,
        name: String,
        path: String,
    },
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{0} has no description; start it with\n---\ndescription: one line about the skill\n---"
    )]
    NoDescription(String),
}

impl Skills {
    /// No skills for anyone.
    pub fn none() -> Self {
        Self::default()
    }

    /// Reads every skill `harness.toml` names from `<harness_dir>/skills/`.
    /// A missing or broken file is an error now, not a surprise in the middle of a task.
    pub fn load(harness_dir: &Path, config: &Config) -> Result<Self, SkillError> {
        let dir = harness_dir.join(SKILLS_DIR);
        let mut roles = BTreeMap::new();
        for (&role, settings) in &config.roles {
            let read_all = |names: &[String]| -> Result<Vec<Skill>, SkillError> {
                names
                    .iter()
                    .map(|name| read_skill(&dir, role, name))
                    .collect()
            };
            let always = read_all(&settings.always_skills)?;
            // A skill that is always in the prompt need not be listed again.
            let on_demand: Vec<String> = settings
                .skills
                .iter()
                .filter(|name| !settings.always_skills.contains(name))
                .cloned()
                .collect();
            let on_demand = read_all(&on_demand)?;
            roles.insert(role, RoleSkills { on_demand, always });
        }
        Ok(Self { roles })
    }

    /// The skills of one role; a role without skills gets an empty set.
    pub fn for_role(&self, role: Role) -> RoleSkills {
        self.roles.get(&role).cloned().unwrap_or_default()
    }
}

/// Only simple names, so a name can never point outside the skills folder.
fn check_name(name: &str) -> Result<(), SkillError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(SkillError::BadName(name.to_string()))
    }
}

fn read_skill(dir: &Path, role: Role, name: &str) -> Result<Skill, SkillError> {
    check_name(name)?;
    let path = dir.join(format!("{name}.md"));
    let shown = path.display().to_string();
    if !path.is_file() {
        return Err(SkillError::Missing {
            role,
            name: name.to_string(),
            path: shown,
        });
    }
    let text = fs::read_to_string(&path).map_err(|source| SkillError::Io {
        path: shown.clone(),
        source,
    })?;
    let (description, body) = split_header(&text).ok_or(SkillError::NoDescription(shown))?;
    Ok(Skill {
        name: name.to_string(),
        description,
        path,
        body,
    })
}

/// Splits `---\ndescription: ...\n---\nbody` into the description and the body.
fn split_header(text: &str) -> Option<(String, String)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)], toml: &str) -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(SKILLS_DIR);
        fs::create_dir_all(&skills).unwrap();
        for (name, text) in files {
            fs::write(skills.join(name), text).unwrap();
        }
        (dir, Config::parse(toml).unwrap())
    }

    const ERRORS: &str = "---\ndescription: Handle errors well.\n---\nUse thiserror.\n";
    const STYLE: &str =
        "---\nname: style\ndescription: \"Write idiomatic Rust.\"\n---\n\nRun clippy.\n";

    #[test]
    fn loads_the_skills_each_role_lists() {
        let (dir, config) = project(
            &[("rust-errors.md", ERRORS), ("style.md", STYLE)],
            "[roles.developer]\nagent = \"claude\"\n\
             skills = [\"rust-errors\", \"style\"]\nalways_skills = [\"style\"]\n\
             [roles.tester]\nagent = \"claude\"\n",
        );
        let skills = Skills::load(dir.path(), &config).unwrap();

        let developer = skills.for_role(Role::Developer);
        assert_eq!(developer.on_demand.len(), 1, "style is not listed twice");
        let errors = &developer.on_demand[0];
        assert_eq!(errors.name, "rust-errors");
        assert_eq!(errors.description, "Handle errors well.");
        assert_eq!(errors.path, dir.path().join("skills/rust-errors.md"));
        assert_eq!(developer.always[0].description, "Write idiomatic Rust.");
        assert_eq!(developer.always[0].body, "Run clippy.");

        assert!(skills.for_role(Role::Tester).is_empty());
        assert!(skills.for_role(Role::Security).is_empty());
    }

    #[test]
    fn a_missing_skill_names_the_role_and_the_file() {
        let (dir, config) = project(
            &[],
            "[roles.tester]\nagent = \"claude\"\nskills = [\"write-tests\"]\n",
        );
        let error = Skills::load(dir.path(), &config).unwrap_err().to_string();
        assert!(error.contains("Tester"), "{error}");
        assert!(error.contains("skills/write-tests.md"), "{error}");
    }

    #[test]
    fn a_name_cannot_leave_the_skills_folder() {
        for name in ["../secret", "a/b", "", "Big", "-x", "a.md"] {
            let toml = format!("[roles.tester]\nagent = \"claude\"\nskills = [{name:?}]\n");
            let (dir, config) = project(&[], &toml);
            assert!(
                matches!(
                    Skills::load(dir.path(), &config),
                    Err(SkillError::BadName(_))
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn a_skill_needs_a_description() {
        for text in [
            "no header",
            "---\nname: x\n---\nbody",
            "---\ndescription: x\n",
        ] {
            let (dir, config) = project(
                &[("x.md", text)],
                "[roles.tester]\nagent = \"claude\"\nskills = [\"x\"]\n",
            );
            assert!(
                matches!(
                    Skills::load(dir.path(), &config),
                    Err(SkillError::NoDescription(_))
                ),
                "{text}"
            );
        }
    }
}
