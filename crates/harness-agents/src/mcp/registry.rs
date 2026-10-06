//! Asking the official MCP registry for servers (see
//! `harness_core::mcp::registry`). `curl` runs with only the whitelisted
//! environment in a temporary folder; no key is sent, the registry is public.

use std::path::Path;

use harness_core::mcp::registry::{parse, Entry, REGISTRY_URL};

use crate::install::models::run;
use crate::process::base_command;

/// At most this many servers per search.
pub const LIMIT: usize = 30;

/// The newest version of each server whose name or text matches `query`
/// (all servers if it is empty), at most [`LIMIT`].
pub fn search(query: &str) -> Result<Vec<Entry>, String> {
    search_with(Path::new("curl"), query)
}

pub fn search_with(curl: &Path, query: &str) -> Result<Vec<Entry>, String> {
    let dir = tempfile::Builder::new()
        .prefix("harness-registry-")
        .tempdir()
        .map_err(|e| format!("cannot make a temporary folder: {e}"))?;
    let mut command = base_command(curl, dir.path());
    command
        .args(["-sS", "-f", "-m", "80", "--proto", "=https", "--get"])
        .args([
            "--data",
            &format!("limit={LIMIT}"),
            "--data",
            "version=latest",
        ]);
    let query = query.trim();
    if !query.is_empty() {
        // `--data-urlencode` encodes it; it is one argument, never an option.
        command.args(["--data-urlencode", &format!("search={query}")]);
    }
    command.arg(REGISTRY_URL);
    parse(&run(command, "", &[])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use {std::fs, std::os::unix::fs::PermissionsExt};

    /// A fake curl that writes its arguments to a file and prints `answer`.
    #[cfg(unix)] // a shell script stands in for the program
    fn fake_curl(dir: &Path, answer: &str) -> std::path::PathBuf {
        let path = dir.join("curl");
        let log = dir.join("args");
        fs::write(dir.join("answer"), answer).unwrap();
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\n",
                log.display(),
                dir.join("answer").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)] // a shell script stands in for the program
    #[test]
    fn the_search_is_one_encoded_argument() {
        let dir = tempfile::tempdir().unwrap();
        let answer = r#"{"servers":[{"server":{"name":"io.github.x/demo","version":"1.0.0",
            "packages":[{"registryType":"npm","identifier":"demo-mcp","version":"1.0.0",
            "transport":{"type":"stdio"}}]}}]}"#;
        let curl = fake_curl(dir.path(), answer);
        let entries = search_with(&curl, " --output /etc/x ").unwrap();
        assert_eq!(entries[0].offer.as_ref().unwrap().name, "demo");
        let args = fs::read_to_string(dir.path().join("args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert!(args.contains(&"search=--output /etc/x"), "{args:?}");
        assert!(!args.contains(&"--output"), "{args:?}");
        assert_eq!(args.last(), Some(&REGISTRY_URL));

        search_with(&curl, "").unwrap();
        let args = fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(!args.contains("search="), "{args}");
    }

    #[cfg(unix)] // a shell script stands in for the program
    #[test]
    fn a_failing_curl_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let curl = dir.path().join("curl");
        fs::write(
            &curl,
            "#!/bin/sh\necho 'Could not resolve host' >&2\nexit 6\n",
        )
        .unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        let error = search_with(&curl, "x").unwrap_err();
        assert!(error.contains("Could not resolve host"), "{error}");
        assert!(search_with(&dir.path().join("none"), "x")
            .unwrap_err()
            .contains("cannot start"));
    }

    /// The real registry: `cargo test -p harness-agents -- --ignored`.
    #[test]
    #[ignore = "asks the real registry"]
    fn the_real_registry_answers() {
        let entries = search("context7").unwrap();
        assert!(entries.iter().any(|e| e.offer.is_some()), "{entries:?}");
    }
}
