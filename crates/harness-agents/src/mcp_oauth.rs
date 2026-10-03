//! Signing in to an MCP server on the web with OAuth, the way the MCP
//! specification describes it, so that servers without API keys (GitHub
//! apps, Notion, ...) can be used too.
//!
//! `harness mcp login <name>` (or «Sign in» on the MCP tab) does it once:
//!
//! 1. asks the server which authorization server it trusts (the
//!    `WWW-Authenticate` answer and `/.well-known/oauth-protected-resource`)
//!    and reads that server's metadata;
//! 2. registers the harness as a client there (dynamic client registration),
//!    with an address on this computer to come back to;
//! 3. opens the browser, where Lisa signs in and agrees; the browser comes
//!    back to `http://127.0.0.1:<port>/callback` with a code;
//! 4. trades the code for tokens (with PKCE, so a stolen code is useless).
//!
//! The tokens are kept in `~/.harness/credentials/oauth/<name>.json`
//! (permissions 600), never in a project. Before a run or a check, the
//! harness renews an access token that is about to end and gives it to the
//! bridge as an `Authorization` header; the agents never see it.
//!
//! Everything goes through `curl` like the rest of the harness. Secrets
//! (codes, tokens, client secrets) are sent on its standard input, never on
//! its command line.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_core::mcp::is_allowed_url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::credentials::{write_private, Secret};
use crate::process::base_command;

/// Where the sign-ins are kept, under `~/.harness/credentials`.
const OAUTH_DIR: &str = "oauth";

/// How long Lisa has to finish in the browser.
pub const BROWSER_LIMIT: Duration = Duration::from_secs(300);

/// An access token that ends sooner than this is renewed first.
const RENEW_BEFORE: u64 = 120;

/// One sign-in, as kept on disk.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    /// The MCP server's address it belongs to.
    pub url: String,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds since 1970; `None` if the server did not say.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignIn")
            .field("url", &self.url)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// How to reach the network and the browser; tests use their own.
pub struct Tools<'a> {
    pub curl: PathBuf,
    /// Opens the address in the browser.
    pub open: &'a dyn Fn(&str) -> Result<(), String>,
    pub browser_limit: Duration,
}

pub fn path(credentials_dir: &Path, server: &str) -> PathBuf {
    credentials_dir
        .join(OAUTH_DIR)
        .join(format!("{server}.json"))
}

/// The saved sign-in of `server`, if it is for this address.
pub fn load(credentials_dir: &Path, server: &str, url: &str) -> Option<SignIn> {
    let text = fs::read_to_string(path(credentials_dir, server)).ok()?;
    let sign_in: SignIn = serde_json::from_str(&text).ok()?;
    (sign_in.url == url).then_some(sign_in)
}

fn save(credentials_dir: &Path, server: &str, sign_in: &SignIn) -> Result<(), String> {
    let file = path(credentials_dir, server);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(sign_in).map_err(|e| e.to_string())?;
    write_private(&file, &text).map_err(|e| format!("cannot save the sign-in: {e}"))
}

/// Forgets the sign-in of `server`.
pub fn logout(credentials_dir: &Path, server: &str) -> std::io::Result<()> {
    match fs::remove_file(path(credentials_dir, server)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A valid access token for `server` at `url`: the saved one, or a renewed
/// one if it is about to end. `None` means: sign in again.
pub fn access_token(
    credentials_dir: &Path,
    server: &str,
    url: &str,
    curl: &Path,
) -> Option<Secret> {
    let mut sign_in = load(credentials_dir, server, url)?;
    let ending = sign_in
        .expires_at
        .is_some_and(|at| at <= now() + RENEW_BEFORE);
    if ending {
        let refresh = sign_in.refresh_token.clone()?;
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh),
            ("client_id", sign_in.client_id.clone()),
            ("resource", url.to_string()),
        ];
        if let Some(secret) = &sign_in.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        let tokens = token_request(curl, &sign_in.token_endpoint, &form).ok()?;
        sign_in.access_token = tokens.access_token;
        if tokens.refresh_token.is_some() {
            sign_in.refresh_token = tokens.refresh_token;
        }
        sign_in.expires_at = tokens.expires_in.map(|s| now() + s);
        save(credentials_dir, server, &sign_in).ok()?;
    }
    Some(Secret::new(sign_in.access_token))
}

/// The secret function for MCP servers: `oauth/<server> <url>` is a sign-in,
/// any other name a secret saved with `harness secret set`.
pub fn mcp_secret(credentials_dir: &Path, name: &str) -> Option<Secret> {
    match name
        .strip_prefix(harness_core::mcp::OAUTH_SECRET)
        .and_then(|rest| rest.split_once(' '))
    {
        Some((server, url)) => access_token(credentials_dir, server, url, Path::new("curl")),
        None => crate::credentials::load_secret(credentials_dir, name).ok(),
    }
}

/// Signs in to the MCP server `server` at `url` and keeps the tokens.
pub fn login(credentials_dir: &Path, server: &str, url: &str, tools: &Tools) -> Result<(), String> {
    if !is_allowed_url(url) {
        return Err(format!("{url:?} is not an address to sign in to"));
    }
    let curl = &tools.curl;
    let found = discover(curl, url)?;
    // The browser comes back here; the port is chosen now, as the client is
    // registered with this exact address.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot listen on this computer: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let (client_id, client_secret) = register(curl, &found, &redirect)?;

    let verifier = random_text(64)?;
    let challenge = base64_url(&sha256(verifier.as_bytes()));
    let state = random_text(32)?;
    let mut query = vec![
        ("response_type", "code".to_string()),
        ("client_id", client_id.clone()),
        ("redirect_uri", redirect.clone()),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256".to_string()),
        ("state", state.clone()),
        ("resource", url.to_string()),
    ];
    if let Some(scope) = &found.scope {
        query.push(("scope", scope.clone()));
    }
    let separator = if found.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    let address = format!(
        "{}{separator}{}",
        found.authorization_endpoint,
        form_encode(&query)
    );
    (tools.open)(&address)?;
    let code = wait_for_code(&listener, &state, tools.browser_limit)?;

    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", client_id.clone()),
        ("code_verifier", verifier),
        ("resource", url.to_string()),
    ];
    if let Some(secret) = &client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let tokens = token_request(curl, &found.token_endpoint, &form)?;
    save(
        credentials_dir,
        server,
        &SignIn {
            url: url.to_string(),
            token_endpoint: found.token_endpoint,
            client_id,
            client_secret,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_in.map(|s| now() + s),
        },
    )
}

/// Opens `address` in Lisa's browser (see `harness_platform::open`).
pub fn open_in_browser(address: &str) -> Result<(), String> {
    harness_platform::open::url(address)
}

/// The server `name` of the project, if it is one to sign in to: its address.
pub fn oauth_url(config: &harness_core::config::Config, name: &str) -> Result<String, String> {
    let server = config
        .mcp
        .get(name)
        .ok_or_else(|| format!("harness.toml has no [mcp.{name}]"))?;
    match (&server.url, server.auth.as_deref()) {
        (Some(url), Some(harness_core::mcp::AUTH_OAUTH)) => Ok(url.trim().to_string()),
        _ => Err(format!(
            "[mcp.{name}] is not a sign-in server: it needs url = \"https://...\" and auth = \"oauth\""
        )),
    }
}

/// What the server and its authorization server said about signing in.
#[derive(Debug)]
struct Found {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scope: Option<String>,
}

fn discover(curl: &Path, url: &str) -> Result<Found, String> {
    // Without a token the server answers 401 and says where its metadata is.
    let hello = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"harness","version":"1"}}}"#;
    let answer = http(
        curl,
        "POST",
        url,
        &["Accept: application/json, text/event-stream"],
        Some(("application/json", hello)),
    )?;
    let challenge = answer.header("www-authenticate").unwrap_or_default();
    let scope = parameter(&challenge, "scope");
    let (origin, path) = split_url(url);
    let mut resource_addresses: Vec<String> = parameter(&challenge, "resource_metadata")
        .into_iter()
        .collect();
    let path = path.trim_end_matches('/');
    if !path.is_empty() {
        resource_addresses.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    resource_addresses.push(format!("{origin}/.well-known/oauth-protected-resource"));
    let resource = resource_addresses
        .iter()
        .filter(|a| is_allowed_url(a))
        .find_map(|a| get_json(curl, a));
    let issuer = resource
        .as_ref()
        .and_then(|r| r["authorization_servers"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| origin.to_string());
    let scope = scope.or_else(|| {
        let scopes = resource.as_ref()?["scopes_supported"].as_array()?;
        let scopes: Vec<&str> = scopes.iter().filter_map(Value::as_str).collect();
        (!scopes.is_empty()).then(|| scopes.join(" "))
    });
    if !is_allowed_url(&issuer) {
        return Err(format!("the authorization server {issuer:?} is not https"));
    }
    let (issuer_origin, issuer_path) = split_url(&issuer);
    let issuer_path = issuer_path.trim_end_matches('/');
    let metadata_addresses = if issuer_path.is_empty() {
        vec![
            format!("{issuer_origin}/.well-known/oauth-authorization-server"),
            format!("{issuer_origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{issuer_origin}/.well-known/oauth-authorization-server{issuer_path}"),
            format!("{issuer_origin}/.well-known/openid-configuration{issuer_path}"),
            format!("{issuer_origin}{issuer_path}/.well-known/openid-configuration"),
        ]
    };
    let metadata = metadata_addresses.iter().find_map(|a| get_json(curl, a));
    let endpoint = |key: &str, default: &str| -> Result<String, String> {
        let address = metadata
            .as_ref()
            .and_then(|m| m[key].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{issuer_origin}{default}"));
        if is_allowed_url(&address) {
            Ok(address)
        } else {
            Err(format!("the {key} {address:?} is not https"))
        }
    };
    let registration_endpoint = match &metadata {
        Some(m) => m["registration_endpoint"].as_str().map(str::to_string),
        None => Some(format!("{issuer_origin}/register")),
    };
    if let Some(methods) = metadata
        .as_ref()
        .and_then(|m| m["code_challenge_methods_supported"].as_array())
    {
        if !methods.iter().any(|m| m == "S256") {
            return Err("the authorization server does not offer PKCE (S256)".into());
        }
    }
    Ok(Found {
        authorization_endpoint: endpoint("authorization_endpoint", "/authorize")?,
        token_endpoint: endpoint("token_endpoint", "/token")?,
        registration_endpoint: registration_endpoint.filter(|a| is_allowed_url(a)),
        scope,
    })
}

/// Registers the harness as a client; returns its id and secret, if any.
fn register(
    curl: &Path,
    found: &Found,
    redirect: &str,
) -> Result<(String, Option<String>), String> {
    let Some(endpoint) = &found.registration_endpoint else {
        return Err("this server does not let the harness register itself \
                    (no dynamic client registration); use a key or token for it instead"
            .into());
    };
    let request = serde_json::json!({
        "client_name": "AI harness",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let answer = http(
        curl,
        "POST",
        endpoint,
        &["Accept: application/json"],
        Some(("application/json", &request.to_string())),
    )?;
    if !(200..300).contains(&answer.code) {
        return Err(format!(
            "registering the harness failed: HTTP {}{}",
            answer.code,
            said(&answer.body)
        ));
    }
    let client: Value =
        serde_json::from_str(&answer.body).map_err(|_| "the registration answer is not JSON")?;
    let id = client["client_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("the registration answer has no client_id")?;
    Ok((
        id.to_string(),
        client["client_secret"].as_str().map(str::to_string),
    ))
}

/// Waits for the browser to come back with the code.
fn wait_for_code(listener: &TcpListener, state: &str, limit: Duration) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + limit;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(result) = answer_browser(stream, state) {
                    return result;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "no sign-in in the browser within {} minutes",
                        limit.as_secs() / 60
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// One visit of the browser. `None` for a visit that is not the callback
/// (a browser also asks for `/favicon.ico`).
fn answer_browser(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut first = String::new();
    BufReader::new(stream.try_clone().ok()?)
        .read_line(&mut first)
        .ok()?;
    let target = first.split_whitespace().nth(1).unwrap_or_default();
    let Some(query) = target.strip_prefix("/callback?") else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return None;
    };
    let values = parse_query(query);
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let result = if get("state").as_deref() != Some(state) {
        Err("the browser came back with a wrong state; try again".to_string())
    } else if let Some(error) = get("error") {
        let detail = get("error_description").unwrap_or_default();
        Err(clean(&format!("the sign-in was refused: {error} {detail}")))
    } else {
        get("code").ok_or_else(|| "the browser came back without a code".to_string())
    };
    let text = match &result {
        Ok(_) => "Signed in. You can close this window and go back to the harness.",
        Err(_) => "The sign-in did not work. Go back to the harness to see why.",
    };
    let page = format!("<!doctype html><meta charset=utf-8><title>harness</title><p>{text}</p>");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{page}",
        page.len()
    );
    Some(result)
}

/// What a token endpoint gave.
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

fn token_request(curl: &Path, endpoint: &str, form: &[(&str, String)]) -> Result<Tokens, String> {
    let answer = http(
        curl,
        "POST",
        endpoint,
        &["Accept: application/json"],
        Some(("application/x-www-form-urlencoded", &form_encode(form))),
    )?;
    if !(200..300).contains(&answer.code) {
        return Err(format!(
            "the token request failed: HTTP {}{}",
            answer.code,
            said(&answer.body)
        ));
    }
    let tokens: Value =
        serde_json::from_str(&answer.body).map_err(|_| "the token answer is not JSON")?;
    let access_token = tokens["access_token"]
        .as_str()
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
        .ok_or("the token answer has no access_token")?;
    Ok(Tokens {
        access_token: access_token.to_string(),
        refresh_token: tokens["refresh_token"].as_str().map(str::to_string),
        expires_in: tokens["expires_in"]
            .as_u64()
            .or_else(|| tokens["expires_in"].as_str()?.parse().ok()),
    })
}

/// The `error` and `error_description` of an OAuth error answer, if any.
fn said(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    let text = [&value["error"], &value["error_description"]]
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(": ");
    if text.is_empty() {
        String::new()
    } else {
        format!(" ({})", clean(&text))
    }
}

/// An HTTP answer.
struct Answer {
    code: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

fn get_json(curl: &Path, address: &str) -> Option<Value> {
    let answer = http(curl, "GET", address, &["Accept: application/json"], None).ok()?;
    if !(200..300).contains(&answer.code) {
        return None;
    }
    serde_json::from_str(&answer.body).ok()
}

/// One request with curl. The body goes on curl's standard input, so the
/// codes and tokens in it are never on a command line.
fn http(
    curl: &Path,
    method: &str,
    address: &str,
    headers: &[&str],
    body: Option<(&str, &str)>,
) -> Result<Answer, String> {
    if !is_allowed_url(address) {
        return Err(format!("{address:?} is not https"));
    }
    let dir = std::env::temp_dir();
    let mut command = base_command(curl, &dir);
    let protocols = if address.starts_with("http://") {
        "=http,https"
    } else {
        "=https"
    };
    command
        .args(["-sS", "-i", "-m", "30", "--max-filesize", "1000000"])
        .args(["--noproxy", "localhost,127.0.0.1,::1", "--proto", protocols])
        .args(["-X", method]);
    for header in headers {
        command.arg("-H").arg(header);
    }
    if let Some((content_type, _)) = body {
        command
            .arg("-H")
            .arg(format!("Content-Type: {content_type}"))
            .args(["--data-binary", "@-"]);
    }
    command
        .arg(address)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        crate::process::spawn(&mut command).map_err(|e| format!("cannot start curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some((_, text)) = body {
            let _ = stdin.write_all(text.as_bytes());
        }
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stdout.as_bytes());
    match crate::mcp_remote::read_head(&mut reader) {
        Some((code, headers)) => {
            let mut body = String::new();
            let _ = reader.read_to_string(&mut body);
            Ok(Answer {
                code,
                headers,
                body,
            })
        }
        None => {
            let why = stderr.lines().map(str::trim).rfind(|l| !l.is_empty());
            Err(clean(&format!(
                "{address}: {}",
                why.unwrap_or(&format!("no answer ({status})"))
            )))
        }
    }
}

/// `key="value"` from a `WWW-Authenticate` header.
fn parameter(header: &str, key: &str) -> Option<String> {
    let start = header.find(&format!("{key}="))? + key.len() + 1;
    let rest = &header[start..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', ' ']).next()?,
    };
    (!value.is_empty()).then(|| value.to_string())
}

/// `https://host:1/a/b?c` → (`https://host:1`, `/a/b`).
fn split_url(url: &str) -> (&str, &str) {
    let scheme_end = url.find("://").map_or(0, |i| i + 3);
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    match without_query[scheme_end..].find('/') {
        Some(slash) => without_query.split_at(scheme_end + slash),
        None => (without_query, ""),
    }
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(300).collect()
}

/// `a=1&b=x%20y`, with every character outside the unreserved set encoded.
fn form_encode(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent(k), percent(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    let decode = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                    match u8::from_str_radix(hex, 16) {
                        Ok(byte) => {
                            out.push(byte);
                            i += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                byte => out.push(byte),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// Text of `length` random letters and digits (from the system's random
/// source, on every system), for the PKCE verifier and the state. Without a
/// random source the sign-in stops: a guessable verifier is worse than none.
fn random_text(length: usize) -> Result<String, String> {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers on this computer: {e}"))?;
    Ok(bytes
        .iter()
        .map(|b| char::from(LETTERS[usize::from(*b) % LETTERS.len()]))
        .collect())
}

/// Base64 for addresses: `-` and `_` instead of `+` and `/`, no `=`.
fn base64_url(bytes: &[u8]) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(LETTERS[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// SHA-256 (FIPS 180-4), for the PKCE challenge.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (state, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *state = state.wrapping_add(add);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn sha256_and_base64_match_the_pkce_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            base64_url(&sha256(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        // FIPS 180-4 examples: empty, and longer than one block.
        assert_eq!(
            base64_url(&sha256(b"")),
            "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"
        );
        assert_eq!(sha256(&[b'a'; 1000])[..4], [0x41, 0xed, 0xec, 0xe4]);
        assert_eq!(base64_url(b"ab"), "YWI");
        assert_eq!(random_text(40).unwrap().len(), 40);
        assert_ne!(random_text(40).unwrap(), random_text(40).unwrap());
    }

    #[test]
    fn addresses_and_queries_are_read_and_written() {
        assert_eq!(
            split_url("https://h.io:8/a/b?c=1"),
            ("https://h.io:8", "/a/b")
        );
        assert_eq!(split_url("https://h.io"), ("https://h.io", ""));
        assert_eq!(
            parameter(
                r#"Bearer error="invalid_token", resource_metadata="https://h/.well-known/x", scope=read"#,
                "resource_metadata"
            )
            .as_deref(),
            Some("https://h/.well-known/x")
        );
        let encoded = form_encode(&[("a b", "x/y=z&ü".to_string())]);
        assert_eq!(encoded, "a%20b=x%2Fy%3Dz%26%C3%BC");
        assert_eq!(
            parse_query("code=a%2Fb+c&state=%zz&x"),
            [
                ("code".to_string(), "a/b c".to_string()),
                ("state".to_string(), "%zz".to_string()),
                ("x".to_string(), String::new())
            ]
        );
    }

    /// What the fake authorization server remembers.
    #[derive(Default)]
    struct Fake {
        challenge: String,
        refreshed: bool,
    }

    /// A web MCP server with its own authorization server, on this computer.
    fn fake_servers() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(Mutex::new(Fake::default()));
        let address = base.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (base, state) = (address.clone(), Arc::clone(&state));
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    reader.read_line(&mut first).unwrap();
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let line = line.trim_end().to_ascii_lowercase();
                        if line.is_empty() {
                            break;
                        }
                        if let Some(n) = line.strip_prefix("content-length:") {
                            length = n.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let body = String::from_utf8(body).unwrap();
                    let form: std::collections::HashMap<String, String> =
                        parse_query(&body).into_iter().collect();
                    let target = first.split_whitespace().nth(1).unwrap().to_string();
                    let json = |code: u16, value: Value| {
                        let text = value.to_string();
                        format!(
                            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\n\r\n{text}",
                            text.len()
                        )
                    };
                    let answer = match target.as_str() {
                        "/mcp" => format!(
                            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer \
                             resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\"\r\n\
                             Content-Length: 0\r\n\r\n"
                        ),
                        "/.well-known/oauth-protected-resource/mcp" => json(
                            200,
                            serde_json::json!({"resource": format!("{base}/mcp"),
                                "authorization_servers": [format!("{base}/auth")]}),
                        ),
                        "/.well-known/oauth-authorization-server/auth" => json(
                            200,
                            serde_json::json!({
                                "issuer": format!("{base}/auth"),
                                "authorization_endpoint": format!("{base}/auth/authorize"),
                                "token_endpoint": format!("{base}/auth/token"),
                                "registration_endpoint": format!("{base}/auth/register"),
                                "code_challenge_methods_supported": ["S256"],
                            }),
                        ),
                        "/auth/register" => {
                            let request: Value = serde_json::from_str(&body).unwrap();
                            assert!(request["redirect_uris"][0]
                                .as_str()
                                .unwrap()
                                .starts_with("http://127.0.0.1:"));
                            json(201, serde_json::json!({"client_id": "client-1"}))
                        }
                        "/auth/token" => match form["grant_type"].as_str() {
                            "authorization_code" => {
                                let fake = state.lock().unwrap();
                                let ok = form["code"] == "code-1"
                                    && form["client_id"] == "client-1"
                                    && form["resource"] == format!("{base}/mcp")
                                    && base64_url(&sha256(form["code_verifier"].as_bytes()))
                                        == fake.challenge;
                                if ok {
                                    json(200, serde_json::json!({"access_token": "at-1",
                                        "refresh_token": "rt-1", "expires_in": 3600,
                                        "token_type": "Bearer"}))
                                } else {
                                    json(400, serde_json::json!({"error": "invalid_grant"}))
                                }
                            }
                            "refresh_token" if form["refresh_token"] == "rt-1" => {
                                state.lock().unwrap().refreshed = true;
                                json(200, serde_json::json!({"access_token": "at-2",
                                    "expires_in": 3600}))
                            }
                            _ => json(400, serde_json::json!({"error": "invalid_grant",
                                "error_description": "bad"})),
                        },
                        _ if target.starts_with("/auth/authorize?") => {
                            let query: std::collections::HashMap<String, String> =
                                parse_query(target.split_once('?').unwrap().1)
                                    .into_iter()
                                    .collect();
                            assert_eq!(query["code_challenge_method"], "S256");
                            state.lock().unwrap().challenge = query["code_challenge"].clone();
                            // The «browser» goes back with the code.
                            format!(
                                "HTTP/1.1 302 Found\r\nLocation: {}?code=code-1&state={}\r\n\
                                 Content-Length: 0\r\n\r\n",
                                query["redirect_uri"], query["state"]
                            )
                        }
                        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into(),
                    };
                    let mut stream = stream;
                    let _ = stream.write_all(answer.as_bytes());
                });
            }
        });
        base
    }

    /// A «browser» that follows the redirects with curl, in the background.
    fn browser(address: &str) -> Result<(), String> {
        let address = address.to_string();
        thread::spawn(move || {
            let _ = std::process::Command::new("curl")
                .args(["-sS", "-L", "--noproxy", "*", "-o", "/dev/null", &address])
                .status();
        });
        Ok(())
    }

    #[test]
    fn a_sign_in_keeps_private_tokens_and_renews_them() {
        let base = fake_servers();
        let url = format!("{base}/mcp");
        let home = tempfile::tempdir().unwrap();
        let tools = Tools {
            curl: PathBuf::from("curl"),
            open: &browser,
            browser_limit: Duration::from_secs(20),
        };
        login(home.path(), "notion", &url, &tools).unwrap();
        let file = path(home.path(), "notion");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let saved = load(home.path(), "notion", &url).unwrap();
        assert_eq!(saved.access_token, "at-1");
        assert!(!format!("{saved:?}").contains("at-1"));
        assert!(load(home.path(), "notion", "https://other/mcp").is_none());

        // A valid token is used as it is.
        let token = mcp_secret(home.path(), &format!("oauth/notion {url}")).unwrap();
        assert_eq!(token.expose(), "at-1");
        // One about to end is renewed; the refresh token stays.
        let mut ending = saved.clone();
        ending.expires_at = Some(now() + 10);
        save(home.path(), "notion", &ending).unwrap();
        let token = access_token(home.path(), "notion", &url, Path::new("curl")).unwrap();
        assert_eq!(token.expose(), "at-2");
        let renewed = load(home.path(), "notion", &url).unwrap();
        assert_eq!(renewed.refresh_token.as_deref(), Some("rt-1"));
        assert!(renewed.expires_at.unwrap() > now() + 3000);

        // A refresh the server refuses means: sign in again.
        let mut refused = renewed;
        refused.expires_at = Some(0);
        refused.refresh_token = Some("old".into());
        save(home.path(), "notion", &refused).unwrap();
        assert!(access_token(home.path(), "notion", &url, Path::new("curl")).is_none());

        logout(home.path(), "notion").unwrap();
        assert!(!file.exists());
        logout(home.path(), "notion").unwrap();
    }

    #[test]
    fn a_browser_that_never_comes_back_runs_out_of_time() {
        let base = fake_servers();
        let home = tempfile::tempdir().unwrap();
        let tools = Tools {
            curl: PathBuf::from("curl"),
            open: &|_| Ok(()),
            browser_limit: Duration::from_millis(300),
        };
        let error = login(home.path(), "x", &format!("{base}/mcp"), &tools).unwrap_err();
        assert!(error.contains("no sign-in"), "{error}");
        assert!(!path(home.path(), "x").exists());
        assert!(login(home.path(), "x", "http://example.com/mcp", &tools).is_err());
    }

    #[test]
    fn the_browser_callback_checks_the_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let visit = |path: String| {
            thread::spawn(move || {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut answer = String::new();
                let _ = stream.read_to_string(&mut answer);
                answer
            })
        };
        let favicon = visit("/favicon.ico".into());
        let wrong = visit("/callback?code=c&state=bad".into());
        // The two visits may come in either order; the favicon is answered
        // and skipped, the wrong state ends the wait.
        let first = wait_for_code(&listener, "good", Duration::from_secs(5));
        assert!(first.unwrap_err().contains("wrong state"));
        assert!(wrong.join().unwrap().contains("did not work"));
        let refused = visit("/callback?error=access_denied&state=good".into());
        let error = wait_for_code(&listener, "good", Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("access_denied"), "{error}");
        // Answered by the first wait or the second one.
        assert!(favicon.join().unwrap().contains("404"));
        refused.join().unwrap();
    }
}
