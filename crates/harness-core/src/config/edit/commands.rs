//! Adding and removing a role's `extra_commands` in harness.toml.

use toml_edit::{value, Array, DocumentMut, Item};

use super::{finish, table, EditError};
use crate::task::handoff::Role;

/// Adds `command` to the role's `extra_commands`; one already there is not
/// added twice. A command that would allow too much, or a role that does not
/// run checks, makes the change fail ([`EditError::Invalid`]).
pub fn add_extra_command(text: &str, role: Role, command: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let list = role_list(&mut doc, role)?;
    if !list.iter().any(|item| item.as_str() == Some(command)) {
        list.push(command);
    }
    finish(&doc)
}

/// Removes `command` from the role's `extra_commands`; the key goes away
/// with its last command. A command that is not there changes nothing.
pub fn remove_extra_command(text: &str, role: Role, command: &str) -> Result<String, EditError> {
    let mut doc: DocumentMut = text.parse()?;
    let list = role_list(&mut doc, role)?;
    list.retain(|item| item.as_str() != Some(command));
    if list.is_empty() {
        if let Some(role_table) = doc["roles"][role.as_str()].as_table_mut() {
            role_table.remove("extra_commands");
        }
    }
    finish(&doc)
}

/// The role's `extra_commands` list, created empty if missing.
fn role_list(doc: &mut DocumentMut, role: Role) -> Result<&mut Array, EditError> {
    let key = role.as_str();
    table(doc, "roles")?
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| EditError::NoRole(key.to_string()))?
        .entry("extra_commands")
        .or_insert(value(Array::new()))
        .as_array_mut()
        .ok_or_else(|| EditError::NotATable(format!("roles.{key}.extra_commands")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const TOML: &str = "# mine\n[roles.security]\nagent = \"claude\" # careful\n\n\
                        [roles.developer]\nagent = \"codex\"\n";

    #[test]
    fn a_command_is_added_once_and_removed_with_its_key() {
        let text = add_extra_command(TOML, Role::Security, "trivy fs").unwrap();
        // Written as typed, then checked: odd spacing is refused, not tidied.
        assert!(matches!(
            add_extra_command(TOML, Role::Security, "trivy\nrm"),
            Err(EditError::Invalid(_))
        ));
        let text = add_extra_command(&text, Role::Security, "trivy fs").unwrap();
        assert!(text.contains("agent = \"claude\" # careful"), "{text}");
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.roles[&Role::Security].extra_commands, ["trivy fs"]);

        let text = remove_extra_command(&text, Role::Security, "trivy fs").unwrap();
        assert!(!text.contains("extra_commands"), "{text}");
    }

    #[test]
    fn a_broad_command_or_a_role_without_checks_is_refused() {
        assert!(matches!(
            add_extra_command(TOML, Role::Security, "bash"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            add_extra_command(TOML, Role::Developer, "trivy"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            add_extra_command(TOML, Role::Tester, "trivy"),
            Err(EditError::NoRole(_))
        ));
    }
}
