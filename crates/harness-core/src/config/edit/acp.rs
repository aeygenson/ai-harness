//! Writing `[acp]` into `harness.toml`: how to start the agent that roles
//! with `agent = "acp"` run.

use toml_edit::{DocumentMut, Item, Table};

use super::{finish, set_list, set_text, EditError};
use crate::config::AcpConfig;

/// Sets `[acp]`: the program and its arguments; `None` removes the section.
pub fn set_acp(text: &str, acp: Option<&AcpConfig>) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let Some(acp) = acp else {
        doc.remove("acp");
        return finish(&doc);
    };
    let entry = doc
        .entry("acp")
        .or_insert_with(|| Item::Table(Table::new()));
    let acp_table = entry
        .as_table_mut()
        .ok_or_else(|| EditError::NotATable("acp".into()))?;
    set_text(acp_table, "command", Some(&acp.command));
    set_list(acp_table, "args", &acp.args);
    finish(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_command_is_written_and_comments_stay() {
        let text = "# my settings\nmax_rounds = 5\n";
        let acp = AcpConfig {
            command: "opencode".into(),
            args: vec!["acp".into()],
        };

        let text = set_acp(text, Some(&acp)).unwrap();

        assert!(text.starts_with("# my settings\n"), "{text}");
        assert_eq!(Config::parse(&text).unwrap().acp, Some(acp));
        let removed = set_acp(&text, None).unwrap();
        assert_eq!(Config::parse(&removed).unwrap().acp, None);
    }
}
