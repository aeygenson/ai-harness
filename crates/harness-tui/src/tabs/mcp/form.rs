//! The server form of the MCP tab: turning its lines into a server and back.

use harness_core::config::{McpAuth, McpConfig};

use crate::ui::i18n::I18n;

/// Why the lines of the server form do not make a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormProblem {
    /// A `"` with no closing `"`.
    OpenQuote,
    /// A word among the variables or headers without `=`.
    NotPair(String),
    /// An address (`https://...`) with a space in it.
    AddressSpaces,
    /// The browser sign-in was ticked for a command, not an address.
    SignInNeedsAddress,
}

impl FormProblem {
    /// The problem in Lisa's language.
    pub fn text(&self, tr: &I18n) -> String {
        match self {
            FormProblem::OpenQuote => tr.t("mcp.form_open_quote").to_string(),
            FormProblem::NotPair(word) => {
                tr.f("mcp.form_not_pair", &[("word", &format!("{word:?}"))])
            }
            FormProblem::AddressSpaces => tr.t("mcp.form_address_spaces").to_string(),
            FormProblem::SignInNeedsAddress => tr.t("mcp.form_sign_in_address").to_string(),
        }
    }
}

/// `npx -y "a b"` → `npx`, `-y`, `a b`: words split at spaces, a quoted
/// part stays one word.
pub fn split_words(text: &str) -> Result<Vec<String>, FormProblem> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err(FormProblem::OpenQuote);
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// The words again as one line; a word with a space is quoted.
pub fn join_words<'a>(words: impl IntoIterator<Item = &'a String>) -> String {
    words
        .into_iter()
        .map(|w| {
            if w.is_empty() || w.contains(char::is_whitespace) {
                format!("\"{w}\"")
            } else {
                w.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the server form shows for `server`: the command line (or the
/// address), the variables (or headers) and whether to sign in through the
/// browser.
pub fn form_values(server: &McpConfig) -> (String, String, bool) {
    let pairs = |map: &std::collections::BTreeMap<String, String>| {
        let words: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
        join_words(&words)
    };
    match &server.url {
        Some(url) => (url.clone(), pairs(&server.headers), server.auth.is_some()),
        None => (
            join_words(server.command.iter().chain(&server.args)),
            pairs(&server.env),
            false,
        ),
    }
}

/// A server from the form: a command line, or an address (`https://...`)
/// on the web; `NAME=value` pairs separated by spaces, which are variables
/// for a command and headers for an address; and whether to sign in through
/// the browser.
pub fn server_from(
    command: &str,
    variables: &str,
    sign_in: bool,
) -> Result<McpConfig, FormProblem> {
    let mut pairs = std::collections::BTreeMap::new();
    for word in split_words(variables)? {
        let Some((name, value)) = word.split_once('=') else {
            return Err(FormProblem::NotPair(word));
        };
        pairs.insert(name.trim().to_string(), value.trim().to_string());
    }
    let command = command.trim();
    if command.starts_with("https://") || command.starts_with("http://") {
        if command.contains(char::is_whitespace) {
            return Err(FormProblem::AddressSpaces);
        }
        return Ok(McpConfig {
            url: Some(command.to_string()),
            headers: pairs,
            auth: sign_in.then_some(McpAuth::OAuth),
            ..McpConfig::default()
        });
    }
    if sign_in {
        return Err(FormProblem::SignInNeedsAddress);
    }
    let mut words = split_words(command)?.into_iter();
    Ok(McpConfig {
        command: words.next(),
        args: words.collect(),
        env: pairs,
        ..McpConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_lines_become_a_server_and_back() {
        let server = server_from(
            "npx -y \"@scope/a b\"",
            "API_KEY=secret:docs  MODE=\"read only\"",
            false,
        )
        .unwrap();
        assert_eq!(server.command.as_deref(), Some("npx"));
        assert_eq!(server.args, ["-y", "@scope/a b"]);
        assert_eq!(server.env["API_KEY"], "secret:docs");
        assert_eq!(server.env["MODE"], "read only");
        assert_eq!(join_words(&server.args), "-y \"@scope/a b\"");
        assert_eq!(
            server_from("npx \"open", "", false),
            Err(FormProblem::OpenQuote)
        );
        assert_eq!(
            server_from("npx", "NOVALUE", false),
            Err(FormProblem::NotPair("NOVALUE".into()))
        );
        assert_eq!(
            server_from("npx", "", true),
            Err(FormProblem::SignInNeedsAddress)
        );
        assert_eq!(
            server_from("https://a.b/mcp x", "", false),
            Err(FormProblem::AddressSpaces)
        );

        let web = server_from(
            " https://a.b/mcp ",
            "Authorization=\"Bearer secret:gh\" X-Mode=read",
            false,
        )
        .unwrap();
        assert_eq!(web.url.as_deref(), Some("https://a.b/mcp"));
        assert_eq!(web.headers["Authorization"], "Bearer secret:gh");
        assert!(web.command.is_none() && web.auth.is_none());
        let (address, headers, sign_in) = form_values(&web);
        assert_eq!(server_from(&address, &headers, sign_in).unwrap(), web);

        assert_eq!(
            form_values(&McpConfig::default()),
            (String::new(), String::new(), false)
        );

        let oauth = server_from("https://a.b/mcp", "", true).unwrap();
        assert_eq!(oauth.auth, Some(McpAuth::OAuth));
        assert!(form_values(&oauth).2);
    }
}
