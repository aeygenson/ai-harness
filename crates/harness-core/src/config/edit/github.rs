//! Writing `[github]` into `harness.toml` when a project is put on GitHub.

use toml_edit::{DocumentMut, Item, Table};

use super::{finish, set_text, EditError};
use crate::config::GitHubConfig;

/// Sets `[github]`: the repository and the branch pull requests go into.
/// `Config::parse` checks both names afterwards, as for every change.
pub fn set_github(text: &str, github: &GitHubConfig) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let entry = doc
        .entry("github")
        .or_insert_with(|| Item::Table(Table::new()));
    let github_table = entry
        .as_table_mut()
        .ok_or_else(|| EditError::NotATable("github".into()))?;
    set_text(github_table, "repo", Some(&github.repo));
    set_text(github_table, "base", Some(&github.base));
    finish(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn github(repo: &str, base: &str) -> GitHubConfig {
        GitHubConfig {
            repo: repo.into(),
            base: base.into(),
        }
    }

    #[test]
    fn github_is_added_and_the_rest_is_kept() {
        let text = "# my settings\n[roles.architect]\nagent = \"claude\" # the best\n";

        let changed = set_github(text, &github("lisa/app", "main")).unwrap();

        assert!(changed.starts_with(text), "{changed}");
        let config = Config::parse(&changed).unwrap();
        assert_eq!(config.github, Some(github("lisa/app", "main")));
    }

    #[test]
    fn a_name_github_would_not_take_is_refused() {
        let error = set_github("", &github("lisa/../app", "main")).unwrap_err();
        assert!(matches!(error, EditError::Invalid(_)), "{error}");
        set_github("", &github("lisa/app", "-main")).unwrap_err();
    }
}
