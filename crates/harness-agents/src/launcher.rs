//! A tiny helper the `harness` program runs for Codex, so that Codex never
//! holds a secret in its own environment. Commands the agent runs inherit that
//! environment, and a live run showed that Codex's `shell_environment_policy`
//! does not keep variables out of them.
//!
//! - `harness mcp-exec <file>` starts an MCP server: the file (JSON, readable
//!   only by Lisa, in a temporary folder outside the project) holds its
//!   command, arguments and variables with the secrets.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::mcp::McpServer;

use crate::credentials::write_private;

/// The `harness` subcommand.
pub const MCP_EXEC: &str = "mcp-exec";

/// The servers with the web bridge (`harness mcp-remote`) pointed at
/// `harness`, the running program; other servers stay as they are.
pub fn with_bridge(servers: Vec<McpServer>, harness: &Path) -> Vec<McpServer> {
    servers
        .into_iter()
        .map(|mut server| {
            if server.command == harness_core::mcp::BRIDGE_COMMAND
                && server.args == [harness_core::mcp::BRIDGE_ARG]
            {
                server.command = harness.display().to_string();
            }
            server
        })
        .collect()
}

/// Writes how to start `server` into `<dir>/<name>.json`.
pub fn write_server(dir: &Path, server: &McpServer) -> io::Result<PathBuf> {
    let env: serde_json::Map<String, serde_json::Value> = server
        .env
        .iter()
        .map(|(name, value)| (name.clone(), value.expose().into()))
        .collect();
    let spec = serde_json::json!({
        "command": server.command,
        "args": server.args,
        "env": env,
    });
    let path = dir.join(format!("{}.json", server.name));
    write_private(&path, &spec.to_string())?;
    Ok(path)
}

/// Reads the file `write_server` wrote and builds the server's command.
pub fn server_command(spec_file: &Path) -> io::Result<Command> {
    let text = fs::read_to_string(spec_file)?;
    let spec: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let program = spec["command"]
        .as_str()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no command"))?;
    let mut command = Command::new(harness_platform::program::resolve(program));
    for arg in spec["args"].as_array().into_iter().flatten() {
        command.arg(arg.as_str().unwrap_or_default());
    }
    for (name, value) in spec["env"].as_object().into_iter().flatten() {
        command.env(name, value.as_str().unwrap_or_default());
    }
    Ok(command)
}

/// `harness mcp-exec`: becomes the MCP server, keeping standard input and
/// output, so Codex talks to the server directly. Returns only on an error.
pub fn exec_server(spec_file: &Path) -> io::Error {
    let mut command = match server_command(spec_file) {
        Ok(command) => command,
        Err(e) => return e,
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.exec()
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::Secret;
    use std::collections::BTreeMap;

    #[test]
    fn a_server_file_is_private_and_starts_the_server_with_its_variables() {
        let dir = tempfile::tempdir().unwrap();
        let server = McpServer {
            name: "everything".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "server".into()],
            env: BTreeMap::from([("TOKEN".into(), Secret::new("tok-secret-1"))]),
        };
        let path = write_server(dir.path(), &server).unwrap();
        assert_eq!(path, dir.path().join("everything.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let command = server_command(&path).unwrap();
        // On Windows the program is found first: `npx` is `npx.cmd` there.
        assert_eq!(
            command.get_program(),
            harness_platform::program::resolve("npx")
        );
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-y", "server"]);
        let env: Vec<_> = command.get_envs().collect();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "TOKEN");
        assert_eq!(env[0].1.unwrap(), "tok-secret-1");
    }

    #[test]
    fn the_web_bridge_points_at_the_running_harness() {
        let web = McpServer {
            name: "github".into(),
            command: harness_core::mcp::BRIDGE_COMMAND.into(),
            args: vec![harness_core::mcp::BRIDGE_ARG.into()],
            env: BTreeMap::new(),
        };
        let program = McpServer {
            name: "docs".into(),
            command: "harness".into(),
            args: vec!["other".into()],
            env: BTreeMap::new(),
        };
        let servers = with_bridge(vec![web, program], Path::new("/opt/bin/harness"));
        assert_eq!(servers[0].command, "/opt/bin/harness");
        assert_eq!(servers[1].command, "harness");
    }
}
