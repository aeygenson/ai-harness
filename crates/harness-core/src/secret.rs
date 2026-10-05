//! A password, token or API key held in memory.

use std::fmt;

/// A secret string. `{:?}` prints `Secret(***)`, so it cannot leak into logs by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The only way to read the value, so every use is easy to find.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Secrets shorter than this are left in the text. Every variable of an MCP
/// server is kept as a secret, even a plain `1` or `fast`, and hiding those
/// everywhere would garble the text. Real keys and tokens are much longer.
pub const MIN_HIDDEN_LEN: usize = 8;

/// Replaces every secret in `text` with `***`, so it can go into a log or a message.
///
/// The longest secrets are replaced first: when one secret contains another,
/// replacing the short one first would leave pieces of the long one behind.
/// Secrets shorter than [`MIN_HIDDEN_LEN`] are not replaced.
pub fn hide(text: &str, secrets: &[&str]) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .copied()
        .filter(|secret| secret.len() >= MIN_HIDDEN_LEN)
        .collect();
    // `Reverse` sorts from the longest to the shortest.
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    let mut text = text.to_string();
    for secret in secrets {
        text = text.replace(secret, "***");
    }
    text
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_secret() {
        let secret = Secret::new("sk-very-secret");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
    }

    #[test]
    fn every_secret_in_the_text_becomes_stars() {
        let text = "key sk-12345678 and again sk-12345678, token tok-abcdefgh";
        let hidden = hide(text, &["sk-12345678", "tok-abcdefgh"]);
        assert_eq!(hidden, "key *** and again ***, token ***");
    }

    #[test]
    fn a_secret_inside_a_longer_one_leaves_nothing_behind() {
        let hidden = hide("Bearer abcdefgh12345678", &["abcdefgh", "abcdefgh12345678"]);
        assert_eq!(hidden, "Bearer ***");
    }

    #[test]
    fn short_values_are_not_hidden() {
        assert_eq!(
            hide("mode fast, level 1", &["fast", "1", ""]),
            "mode fast, level 1"
        );
    }
}
