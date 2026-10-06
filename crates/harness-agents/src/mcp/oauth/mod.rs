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

mod browser;
mod discover;
mod encoding;
mod http;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_core::mcp::is_allowed_url;
use serde::{Deserialize, Serialize};

use crate::install::credentials::{write_private, Secret};
use browser::wait_for_code;
use discover::{discover, register};
use encoding::{base64_url, form_encode, random_text, sha256};
use http::token_request;

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
    /// The server's address that hands out and renews access tokens.
    pub token_endpoint: String,
    /// The id the server gave the harness when it registered as a client.
    pub client_id: String,
    /// The client's password from that registration; `None` if the server gave none.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The token sent with each request to the MCP server.
    pub access_token: String,
    /// Gets a new access token when the old one ends; `None` if the server gave none.
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
    /// The `curl` program that sends the requests.
    pub curl: PathBuf,
    /// Opens the address in the browser.
    pub open: &'a dyn Fn(&str) -> Result<(), String>,
    /// How long to wait for Lisa to finish signing in in the browser.
    pub browser_limit: Duration,
}

/// Written by hand because a closure (`open`) has no `Debug` of its own.
impl std::fmt::Debug for Tools<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools")
            .field("curl", &self.curl)
            .field("browser_limit", &self.browser_limit)
            .finish_non_exhaustive()
    }
}

/// The file that keeps the sign-in of `server`: `oauth/<server>.json` in `credentials_dir`.
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
        None => crate::install::credentials::load_secret(credentials_dir, name).ok(),
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
    match (&server.url, server.auth) {
        (Some(url), Some(harness_core::config::McpAuth::OAuth)) => Ok(url.trim().to_string()),
        _ => Err(format!(
            "[mcp.{name}] is not a sign-in server: it needs url = \"https://...\" and auth = \"oauth\""
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::encoding::{parameter, parse_query, split_url};
    use super::*;
    use serde_json::Value;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
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
