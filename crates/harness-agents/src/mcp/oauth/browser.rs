//! The local address the browser comes back to with the sign-in code.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use harness_core::text::safe_line;

use super::encoding::parse_query;

/// Waits for the browser to come back with the code.
pub(super) fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    limit: Duration,
) -> Result<String, String> {
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
pub(super) fn answer_browser(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
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
        Err(safe_line(
            &format!("the sign-in was refused: {error} {detail}"),
            300,
        ))
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
