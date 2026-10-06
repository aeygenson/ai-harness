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
//!
//! **Built-in skills** come with the program (`crates/harness-core/skills/`):
//!
//! - every role always gets its *base*: `common` (rules for all roles), its own
//!   (`architect`, `developer`, `tester`, `security`) and a short note on the
//!   agent it runs on (`agent-claude`, `agent-codex`, `agent-antigravity`,
//!   `agent-dsh`);
//! - optional ones (`filesystem-attacks`, ...) are chosen like any skill.
//!
//! A project file with the same name replaces a built-in skill: that is how
//! Lisa changes one. Its header remembers which built-in text it was copied
//! from (`builtin: <hash>`), so a newer built-in text can be pointed out.

mod built_ins;
mod library;

pub use built_ins::{
    agent_note, base_names, built_in, built_in_names, copy_of_built_in, fingerprint, is_base,
};
pub use library::{header_value, library, split_header, LibrarySkill};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::task::handoff::Role;

/// The folder with the skills, inside `.harness/`.
pub const SKILLS_DIR: &str = "skills";

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The program's text, unchanged.
    BuiltIn,
    /// A project file replaces a built-in skill; `outdated` when the
    /// built-in text has changed since the copy was made.
    Changed {
        /// True when the built-in text has changed since the copy was made.
        outdated: bool,
    },
    /// The project's own skill.
    Own,
}

/// One skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The skill's name: its file name in `.harness/skills/` without `.md`.
    pub name: String,
    /// One line from the file's header; it goes into the prompt's skill list.
    pub description: String,
    /// The project file: it exists unless the skill is built-in and unchanged.
    pub path: PathBuf,
    /// The instructions, without the header.
    pub body: String,
    /// Whether the skill is built-in, a changed built-in or the project's own.
    pub source: Source,
}

/// The skills of one role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleSkills {
    /// The role's base: common rules, the role, the agent note.
    pub base: Vec<Skill>,
    /// Listed in the prompt; the agent reads a file when it needs it.
    pub on_demand: Vec<Skill>,
    /// Put into the prompt in full.
    pub always: Vec<Skill>,
}

impl RoleSkills {
    /// No chosen skills (the base does not count).
    pub fn is_empty(&self) -> bool {
        self.on_demand.is_empty() && self.always.is_empty()
    }
}

/// The skills of every role in a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skills {
    roles: BTreeMap<Role, RoleSkills>,
}

/// A problem with a skill a role uses, found while loading the skills.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// A skill name with characters that are not allowed.
    #[error(
        "skill name {0:?} is not allowed; use lowercase letters, digits and '-', \
         for example \"rust-errors\""
    )]
    BadName(String),
    /// A role lists a skill that has neither a file nor a built-in text.
    #[error(
        "the {role:?} role uses skill {name:?}, but {path} does not exist \
         and there is no built-in skill with that name"
    )]
    Missing {
        /// The role that lists the skill.
        role: Role,
        /// The skill name as the role lists it.
        name: String,
        /// The file that was looked for, ready to show.
        path: String,
    },
    /// A skill file exists but could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The skill file, ready to show.
        path: String,
        /// The error the operating system gave.
        #[source]
        source: std::io::Error,
    },
    /// A skill file without a `description:` line in its header.
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

    /// Reads every role's base and every skill `harness.toml` names, from
    /// `<harness_dir>/skills/` or the built-in ones. A missing or broken file
    /// is an error now, not a surprise in the middle of a task.
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
            let base = base_names(role, settings.agent)
                .into_iter()
                .map(|name| read_skill(&dir, role, name))
                .collect::<Result<Vec<_>, _>>()?;
            let mut always = read_all(&settings.always_skills)?;
            // A skill that is always in the prompt need not be listed again.
            let on_demand: Vec<String> = settings
                .skills
                .iter()
                .filter(|name| !settings.always_skills.contains(name))
                .cloned()
                .collect();
            // An unchanged built-in skill has no file to read: it goes into
            // the prompt in full (they are short).
            let (files, built_in): (Vec<Skill>, Vec<Skill>) = read_all(&on_demand)?
                .into_iter()
                .partition(|skill| skill.source != Source::BuiltIn);
            always.extend(built_in);
            roles.insert(
                role,
                RoleSkills {
                    base,
                    on_demand: files,
                    always,
                },
            );
        }
        Ok(Self { roles })
    }

    /// The skills of one role; a role without skills gets an empty set.
    pub fn for_role(&self, role: Role) -> RoleSkills {
        self.roles.get(&role).cloned().unwrap_or_default()
    }
}

/// Only simple names, so a name can never point outside the skills folder.
pub fn check_name(name: &str) -> Result<(), SkillError> {
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
    let (text, source) = if path.is_file() {
        let text = fs::read_to_string(&path).map_err(|source| SkillError::Io {
            path: shown.clone(),
            source,
        })?;
        let source = source_of(name, &text);
        (text, source)
    } else if let Some(text) = built_in(name) {
        (text.to_string(), Source::BuiltIn)
    } else {
        return Err(SkillError::Missing {
            role,
            name: name.to_string(),
            path: shown,
        });
    };
    let (description, body) = split_header(&text).ok_or(SkillError::NoDescription(shown))?;
    Ok(Skill {
        name: name.to_string(),
        description,
        path,
        body,
        source,
    })
}

/// A project file named `name` with this text: a changed built-in skill or
/// the project's own.
fn source_of(name: &str, text: &str) -> Source {
    match built_in(name) {
        None => Source::Own,
        Some(original) => Source::Changed {
            outdated: header_value(text, "builtin")
                .is_some_and(|copied| copied != fingerprint(original)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::built_ins::BUILT_IN;
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
    fn every_role_gets_its_base_and_built_in_skills_need_no_file() {
        let (dir, config) = project(
            &[],
            "[roles.architect]\nagent = \"codex\"\n\
             [roles.tester]\nagent = \"claude\"\nskills = [\"crash-recovery\"]\n",
        );
        let skills = Skills::load(dir.path(), &config).unwrap();
        let architect = skills.for_role(Role::Architect);
        let names: Vec<&str> = architect.base.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["common", "architect", "agent-codex"]);
        assert!(architect.base.iter().all(|s| s.source == Source::BuiltIn));
        assert!(architect.base[1].body.starts_with("# Architect"));
        assert!(architect.is_empty());

        // There is no file to read, so the built-in skill is in the prompt.
        let tester = skills.for_role(Role::Tester);
        assert!(tester.on_demand.is_empty());
        assert_eq!(tester.always[0].name, "crash-recovery");
        assert_eq!(tester.base[2].name, "agent-claude");
    }

    #[test]
    fn a_project_file_replaces_a_built_in_skill() {
        let copy = copy_of_built_in("tester").unwrap();
        assert!(copy.starts_with("---\nbuiltin: "), "{copy}");
        let changed = copy.replace("You check the work", "You check everything");
        let old = "---\nbuiltin: 0000000000000000\ndescription: Old.\n---\nOld text.\n";
        let (dir, config) = project(
            &[
                ("tester.md", &changed),
                ("security.md", old),
                ("mine.md", ERRORS),
            ],
            "[roles.tester]\nagent = \"claude\"\n\
             [roles.security]\nagent = \"antigravity\"\n",
        );
        let skills = Skills::load(dir.path(), &config).unwrap();
        let tester = &skills.for_role(Role::Tester).base[1];
        assert!(tester.body.contains("You check everything"));
        assert_eq!(tester.source, Source::Changed { outdated: false });
        let security = &skills.for_role(Role::Security).base[1];
        assert_eq!(security.source, Source::Changed { outdated: true });

        let library = library(dir.path());
        let find = |name: &str| library.iter().find(|s| s.name == name).unwrap();
        assert_eq!(find("mine").source, Source::Own);
        assert_eq!(find("developer").source, Source::BuiltIn);
        assert_eq!(find("developer").text, built_in("developer").unwrap());
        assert_eq!(find("security").description.as_deref(), Some("Old."));
        assert_eq!(library.len(), BUILT_IN.len() + 1);
        assert!(is_base("agent-codex") && is_base("tester") && !is_base("mine"));
    }

    #[test]
    fn every_built_in_skill_has_a_description() {
        for (name, text) in BUILT_IN {
            assert!(split_header(text).is_some(), "{name}");
            assert!(check_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn a_missing_skill_names_the_role_and_the_file() {
        let (dir, config) = project(
            &[],
            "[roles.tester]\nagent = \"claude\"\nskills = [\"write-tests\"]\n",
        );
        let error = Skills::load(dir.path(), &config).unwrap_err().to_string();
        assert!(error.contains("Tester"), "{error}");
        let file = Path::new("skills").join("write-tests.md");
        assert!(error.contains(&file.display().to_string()), "{error}");
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
