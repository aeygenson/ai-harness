//! The messages of the Agent Client Protocol that the harness sends and
//! reads: JSON-RPC 2.0, one JSON object per line on the agent's standard
//! input and output (<https://agentclientprotocol.com/protocol/>).

use std::path::Path;

use harness_core::mcp::McpServer;
use serde_json::{json, Value};

/// The protocol version the harness speaks. It changes only on breaking
/// changes; an agent that answers with another one cannot be used.
pub(super) const PROTOCOL_VERSION: u64 = 1;

/// The ids of the harness's three requests, sent in this order. Fixed
/// numbers keep the conversation easy to follow in the log and in tests.
pub(super) const INITIALIZE_ID: u64 = 0;
/// See [`INITIALIZE_ID`].
pub(super) const NEW_SESSION_ID: u64 = 1;
/// See [`INITIALIZE_ID`].
pub(super) const PROMPT_ID: u64 = 2;

/// JSON-RPC's code for "the method does not exist" (also used for methods
/// the harness does not offer, such as reading files for the agent).
const METHOD_NOT_FOUND: i64 = -32601;

/// A request of the harness, as one line.
fn request(id: u64, method: &str, params: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// `initialize`: the harness offers no file or terminal methods of its own,
/// so the agent works with its own tools, inside the folder it is given.
pub(super) fn initialize() -> String {
    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
        },
        "clientInfo": { "name": "harness", "version": env!("CARGO_PKG_VERSION") },
    });
    request(INITIALIZE_ID, "initialize", &params)
}

/// `session/new` in `cwd` with the role's MCP servers. The servers' secrets
/// go only into this message on the agent's input, never on its command line.
pub(super) fn new_session(cwd: &Path, servers: &[McpServer]) -> String {
    let servers: Vec<Value> = servers.iter().map(server).collect();
    let params = json!({ "cwd": cwd.to_string_lossy(), "mcpServers": servers });
    request(NEW_SESSION_ID, "session/new", &params)
}

/// One MCP server in the protocol's form: a program, its arguments and its
/// environment as a list of names and values.
fn server(server: &McpServer) -> Value {
    let env: Vec<Value> = server
        .env
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value.expose() }))
        .collect();
    json!({
        "name": server.name,
        "command": server.command,
        "args": server.args,
        "env": env,
    })
}

/// `session/prompt`: the role's whole prompt as one text block.
pub(super) fn prompt(session_id: &str, text: &str) -> String {
    let params = json!({
        "sessionId": session_id,
        "prompt": [{ "type": "text", "text": text }],
    });
    request(PROMPT_ID, "session/prompt", &params)
}

/// The harness's answer to a request of the agent.
pub(super) fn answer(id: &Value, result: &Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// The answer to a request the harness does not offer.
pub(super) fn not_offered(id: &Value, method: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": METHOD_NOT_FOUND, "message": format!("{method} is not offered") },
    })
    .to_string()
}

/// One line the agent printed, sorted by what it is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Message {
    /// The answer to the harness's request `id`: its result or its error text.
    Answer {
        /// The id of the harness's request.
        id: u64,
        /// The result, or the error's message.
        result: Result<Value, String>,
    },
    /// A request of the agent that the harness must answer.
    Request {
        /// The agent's id, given back as it came (a number or a text).
        id: Value,
        /// Such as `session/request_permission`.
        method: String,
        /// The request's parameters.
        params: Value,
    },
    /// A notification (such as `session/update`) or a line that is not a
    /// message at all; the log keeps it, nothing else is done.
    Other,
}

/// Reads one line of the agent.
pub(super) fn read(line: &str) -> Message {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Message::Other;
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        if id.is_null() {
            return Message::Other;
        }
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        return Message::Request {
            id,
            method: method.to_string(),
            params,
        };
    }
    // Some agents give the id back as text ("2"); it is the same request.
    let number = id
        .as_u64()
        .or_else(|| id.as_str().and_then(|text| text.parse().ok()));
    let Some(id) = number else {
        return Message::Other;
    };
    let result = match (value.get("result"), value.get("error")) {
        (_, Some(error)) => Err(error_text(error)),
        (Some(result), None) => Ok(result.clone()),
        (None, None) => Ok(Value::Null),
    };
    Message::Answer { id, result }
}

/// The message of a JSON-RPC error, or the whole error when it has none.
fn error_text(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| error.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use harness_core::secret::Secret;

    use super::*;

    #[test]
    fn requests_are_one_line_of_json_rpc() {
        let line = prompt("s1", "Design a parser\nwith tests");

        assert!(!line.contains('\n'));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], PROMPT_ID);
        assert_eq!(value["method"], "session/prompt");
        assert_eq!(value["params"]["sessionId"], "s1");
        assert_eq!(
            value["params"]["prompt"][0]["text"],
            "Design a parser\nwith tests"
        );
        let init: Value = serde_json::from_str(&initialize()).unwrap();
        assert_eq!(init["params"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
    }

    #[test]
    fn mcp_servers_go_into_the_new_session_with_their_environment() {
        let servers = [McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "ctx".into()],
            env: BTreeMap::from([("KEY".to_string(), Secret::new("k-1"))]),
        }];

        let value: Value = serde_json::from_str(&new_session(Path::new("/p"), &servers)).unwrap();

        assert_eq!(value["params"]["cwd"], "/p");
        let server = &value["params"]["mcpServers"][0];
        assert_eq!(server["name"], "context7");
        assert_eq!(server["args"], json!(["-y", "ctx"]));
        assert_eq!(server["env"], json!([{ "name": "KEY", "value": "k-1" }]));
    }

    #[test]
    fn lines_of_the_agent_are_sorted() {
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#),
            Message::Answer {
                id: 1,
                result: Ok(json!({ "sessionId": "s1" })),
            }
        );
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"no key"}}"#),
            Message::Answer {
                id: 1,
                result: Err("no key".into()),
            }
        );
        assert!(matches!(
            read(r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}}"#),
            Message::Request { id, method, .. } if id == "p1" && method == "session/request_permission"
        ));
        assert_eq!(
            read(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#),
            Message::Other
        );
        assert_eq!(read("Loading extensions..."), Message::Other);
        assert!(matches!(
            read(r#"{"jsonrpc":"2.0","id":"2","result":{}}"#),
            Message::Answer { id: 2, .. }
        ));
    }

    #[test]
    fn requests_that_are_not_offered_get_an_error() {
        let value: Value =
            serde_json::from_str(&not_offered(&json!(7), "fs/read_text_file")).unwrap();

        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["code"], METHOD_NOT_FOUND);
    }
}
