//! Finding where to sign in: the server's authorization server and its addresses,
//! then registering the harness there as a client.

use std::path::Path;

use harness_core::mcp::is_allowed_url;
use serde_json::Value;

use super::encoding::{parameter, split_url};
use super::http::{get_json, http, said};

/// What the server and its authorization server said about signing in.
#[derive(Debug)]
pub(super) struct Found {
    pub(super) authorization_endpoint: String,
    pub(super) token_endpoint: String,
    pub(super) registration_endpoint: Option<String>,
    pub(super) scope: Option<String>,
}

pub(super) fn discover(curl: &Path, url: &str) -> Result<Found, String> {
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
pub(super) fn register(
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
    let client: Value = serde_json::from_str(&answer.body)
        .map_err(|error| format!("the registration answer is not JSON: {error}"))?;
    let id = client["client_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("the registration answer has no client_id")?;
    Ok((
        id.to_string(),
        client["client_secret"].as_str().map(str::to_string),
    ))
}
