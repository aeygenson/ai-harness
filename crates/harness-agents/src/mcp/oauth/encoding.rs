//! Small text helpers for OAuth: address parts, form encoding, random text and PKCE.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// `key="value"` from a `WWW-Authenticate` header.
pub(super) fn parameter(header: &str, key: &str) -> Option<String> {
    let start = header.find(&format!("{key}="))? + key.len() + 1;
    let rest = &header[start..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', ' ']).next()?,
    };
    (!value.is_empty()).then(|| value.to_string())
}

/// `https://host:1/a/b?c` → (`https://host:1`, `/a/b`).
pub(super) fn split_url(url: &str) -> (&str, &str) {
    let scheme_end = url.find("://").map_or(0, |i| i + 3);
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    match without_query[scheme_end..].find('/') {
        Some(slash) => without_query.split_at(scheme_end + slash),
        None => (without_query, ""),
    }
}

/// `a=1&b=x%20y`, with every character outside the unreserved set encoded.
pub(super) fn form_encode(pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent(k), percent(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub(super) fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

pub(super) fn parse_query(query: &str) -> Vec<(String, String)> {
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
pub(super) fn random_text(length: usize) -> Result<String, String> {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers on this computer: {e}"))?;
    Ok(bytes
        .iter()
        .map(|b| char::from(LETTERS[usize::from(*b) % LETTERS.len()]))
        .collect())
}

/// Base64 for addresses: `-` and `_` instead of `+` and `/`, no `=`.
pub(super) fn base64_url(bytes: &[u8]) -> String {
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

/// SHA-256, for the PKCE challenge. From the `sha2` crate: a well-tested
/// library instead of our own copy of the algorithm.
pub(super) fn sha256(data: &[u8]) -> [u8; 32] {
    // `digest` returns a fixed-size array type of the crate; `into` turns it
    // into a plain Rust array of 32 bytes.
    Sha256::digest(data).into()
}
