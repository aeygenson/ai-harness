//! Reading the status and headers that `curl -i` prints before an HTTP answer.
//! Shared by the web bridge and the OAuth sign-in.

use std::io::BufRead;

/// The status and headers of an HTTP answer, as `curl -i` prints them. The
/// proxy's «Connection established» and `100 Continue` come first and are
/// skipped.
pub(crate) fn read_head(reader: &mut impl BufRead) -> Option<(u16, Vec<(String, String)>)> {
    loop {
        let mut status = String::new();
        if reader.read_line(&mut status).ok()? == 0 {
            return None;
        }
        let status = status.trim();
        if status.is_empty() {
            continue;
        }
        let code: u16 = status.split_whitespace().nth(1)?.parse().ok()?;
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
        let tunnel = status
            .to_ascii_lowercase()
            .contains("connection established");
        if (100..200).contains(&code) || tunnel {
            continue;
        }
        return Some((code, headers));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn proxy_and_continue_heads_are_skipped() {
        let answer = "HTTP/1.1 200 Connection established\r\n\r\n\
                      HTTP/1.1 100 Continue\r\n\r\n\
                      HTTP/2 202 \r\nmcp-session-id: x\r\n\r\n";
        let (code, headers) = read_head(&mut Cursor::new(answer)).unwrap();
        assert_eq!(code, 202);
        assert_eq!(headers, [("mcp-session-id".to_string(), "x".to_string())]);
    }
}
