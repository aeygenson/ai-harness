//! `[github]` in `harness.toml`: the project lives on GitHub, so every task
//! works on its own branch (see `crate::task::branch`).
//!
//! ```toml
//! [github]
//! repo = "aeygenson/vgs-proposal-prototype"
//! base = "main"   # optional: the branch pull requests go into
//! ```

use serde::{Deserialize, Serialize};

/// `[github]`: the project's repository on GitHub and its main branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    /// `repo`: the repository as `owner/name`.
    #[serde(deserialize_with = "repo_name")]
    pub repo: String,
    /// `base`: the branch a task's pull request goes into; `main` if not set.
    #[serde(default = "default_base", deserialize_with = "branch_name")]
    pub base: String,
}

fn default_base() -> String {
    "main".to_string()
}

/// Letters GitHub allows in an owner or a repository name, besides ASCII
/// letters and digits.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// One part of a name between `/`: allowed letters only, not starting with
/// `-` (git and `gh` would read it as an option) or `.` (hidden, or `..`).
fn is_safe_part(part: &str) -> bool {
    !part.is_empty() && !part.starts_with(['-', '.']) && part.chars().all(is_name_char)
}

/// Is `value` a repository the harness accepts: `owner/name`, each part made
/// only of the letters GitHub allows and not starting with `-` or `.`? A name
/// ending in `.git` is refused: the push address adds `.git` itself.
pub fn is_repo(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| is_safe_part(part))
        && !value.to_ascii_lowercase().ends_with(".git")
}

/// `owner/name`, as [`is_repo`] checks it.
fn repo_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    if is_repo(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "repo {value:?} is not a GitHub repository; write it as \"owner/name\""
        )))
    }
}

/// A branch name the harness can safely give to git: parts between `/` that
/// cannot look like options, and none of the forms git itself refuses.
fn branch_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = String::deserialize(d)?;
    let valid = value != "HEAD"
        && !value.contains("..")
        && value.split('/').all(|part| {
            is_safe_part(part)
                && !part.ends_with('.')
                && !part.to_ascii_lowercase().ends_with(".lock")
        });
    if valid {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "base {value:?} is not a branch name the harness accepts; use letters, digits, \
             '-', '_', '.' and '/', for example \"main\""
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn the_base_branch_is_main_unless_set() {
        let config = Config::parse("[github]\nrepo = \"lisa/app\"\n").unwrap();
        let github = config.github.unwrap();
        assert_eq!(github.repo, "lisa/app");
        assert_eq!(github.base, "main");

        let config = Config::parse("[github]\nrepo = \"lisa/app\"\nbase = \"develop\"\n").unwrap();
        assert_eq!(config.github.unwrap().base, "develop");
        assert_eq!(Config::parse("").unwrap().github, None);
    }

    #[test]
    fn a_repository_is_owner_and_name() {
        for wrong in [
            "app",
            "lisa/",
            "/app",
            "lisa/app/x",
            "lisa/my app",
            "",
            "lisa/..",
            "-x/--upload-pack",
        ] {
            let error = Config::parse(&format!("[github]\nrepo = \"{wrong}\"\n")).unwrap_err();
            assert!(error.to_string().contains("owner/name"), "{wrong}: {error}");
        }
        Config::parse("[github]\nrepo = \"Lisa-1/vgs_app.v2\"\n").unwrap();
    }

    #[test]
    fn a_base_branch_cannot_look_like_an_option() {
        for wrong in [
            "-x", "--force", "a..b", "", "a b", "main/", ".hidden", "a//b", "x.lock", "a/.b",
            "a.lock/b", "a/-b", "HEAD",
        ] {
            let text = format!("[github]\nrepo = \"lisa/app\"\nbase = \"{wrong}\"\n");
            let error = Config::parse(&text).unwrap_err();
            assert!(
                error.to_string().contains("branch name"),
                "{wrong}: {error}"
            );
        }
        Config::parse("[github]\nrepo = \"lisa/app\"\nbase = \"release/1.0\"\n").unwrap();
    }

    #[test]
    fn a_repository_ending_in_git_is_refused() {
        assert!(super::is_repo("lisa/app"));
        assert!(!super::is_repo("lisa/app.git"));
        assert!(!super::is_repo("lisa/App.GIT"));
    }

    #[test]
    fn typos_in_the_github_section_are_errors() {
        Config::parse("[github]\nrepo = \"lisa/app\"\nbrnach = \"main\"\n").unwrap_err();
        Config::parse("[github]\nbase = \"main\"\n").unwrap_err();
    }
}
