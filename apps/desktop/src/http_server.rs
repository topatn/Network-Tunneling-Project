//! Bounded HTTP request reading and stream handling for GTT desktop shell (`T-080`, `AGENTS.md` §4).
//!
//! Enforces:
//! - Bound on header length: [`MAX_HTTP_HEADER_BYTES`] (32 KiB).
//! - Bound on total payload length: [`MAX_HTTP_PAYLOAD_BYTES`] (1 MiB).
//! - Complete body reading based on `Content-Length` header across TCP packet fragments.
//! - Zero panics, zero unwrap/expect, and bounded buffer reads (`indexing_slicing` safe).

use std::fmt;
use std::io::Read;

/// Maximum bytes permitted for HTTP request headers (32 KiB, `AGENTS.md` §4).
pub const MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;

/// Maximum bytes permitted for HTTP request body payload (1 MiB, `AGENTS.md` §4).
pub const MAX_HTTP_PAYLOAD_BYTES: usize = 1024 * 1024;

/// Parsed HTTP request structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpRequest {
    /// HTTP method (e.g. `"GET"`, `"POST"`, `"HEAD"`).
    pub method: String,
    /// Target path (e.g. `"/api/engine/status"`).
    pub path: String,
    /// Parsed request body bytes.
    pub body: Vec<u8>,
}

/// Errors that can occur when reading an HTTP request from a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpReadError {
    /// Connection was closed before complete headers arrived.
    UnexpectedEof,
    /// Request headers exceeded [`MAX_HTTP_HEADER_BYTES`].
    HeaderTooLarge,
    /// Declared or received body payload exceeded [`MAX_HTTP_PAYLOAD_BYTES`].
    PayloadTooLarge { length: usize, max: usize },
    /// Malformed HTTP request line or headers.
    MalformedRequest(String),
    /// I/O error occurred while reading from the underlying stream.
    Io(String),
}

impl fmt::Display for HttpReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected EOF while reading HTTP request"),
            Self::HeaderTooLarge => write!(
                f,
                "HTTP request headers exceeded maximum of {MAX_HTTP_HEADER_BYTES} bytes"
            ),
            Self::PayloadTooLarge { length, max } => write!(
                f,
                "HTTP payload length ({length} bytes) exceeded maximum of {max} bytes"
            ),
            Self::MalformedRequest(reason) => write!(f, "malformed HTTP request: {reason}"),
            Self::Io(reason) => write!(f, "I/O error reading HTTP stream: {reason}"),
        }
    }
}

impl std::error::Error for HttpReadError {}

/// Find the byte offset right after the `\r\n\r\n` sequence that terminates HTTP headers.
pub fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

/// Parse the value of the `Content-Length` header if present.
pub fn parse_content_length(headers_text: &str) -> Option<usize> {
    for line in headers_text.lines() {
        let trimmed = line.trim();
        if let Some((name, val)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                return val.trim().parse::<usize>().ok();
            }
        }
    }
    None
}

/// Read and parse an HTTP request from `reader` with strict bounds and fragment reassembly.
pub fn read_http_request<R: Read>(reader: &mut R) -> Result<ParsedHttpRequest, HttpReadError> {
    let mut header_buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    let header_end;

    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| HttpReadError::Io(e.to_string()))?;
        if n == 0 {
            if header_buf.is_empty() {
                return Err(HttpReadError::UnexpectedEof);
            }
            if let Some(pos) = find_header_end(&header_buf) {
                header_end = pos;
                break;
            }
            return Err(HttpReadError::UnexpectedEof);
        }

        let slice = chunk.get(..n).ok_or_else(|| {
            HttpReadError::MalformedRequest("chunk slice bounds error".to_owned())
        })?;
        header_buf.extend_from_slice(slice);

        if let Some(pos) = find_header_end(&header_buf) {
            if pos > MAX_HTTP_HEADER_BYTES {
                return Err(HttpReadError::HeaderTooLarge);
            }
            header_end = pos;
            break;
        }

        if header_buf.len() > MAX_HTTP_HEADER_BYTES {
            return Err(HttpReadError::HeaderTooLarge);
        }
    }

    let header_bytes = header_buf
        .get(..header_end)
        .ok_or_else(|| HttpReadError::MalformedRequest("header slice bounds error".to_owned()))?;
    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|e| HttpReadError::MalformedRequest(format!("non-UTF8 headers: {e}")))?;

    let first_line = header_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| HttpReadError::MalformedRequest("missing HTTP method".to_owned()))?
        .to_owned();
    let full_path = parts
        .next()
        .ok_or_else(|| HttpReadError::MalformedRequest("missing HTTP path".to_owned()))?;
    let clean_path = full_path.split('?').next().unwrap_or(full_path).to_owned();

    let content_len = parse_content_length(header_str).unwrap_or(0);
    if content_len > MAX_HTTP_PAYLOAD_BYTES {
        return Err(HttpReadError::PayloadTooLarge {
            length: content_len,
            max: MAX_HTTP_PAYLOAD_BYTES,
        });
    }

    let mut body = Vec::with_capacity(content_len);
    if let Some(initial_body_bytes) = header_buf.get(header_end..) {
        let to_take = std::cmp::min(initial_body_bytes.len(), content_len);
        if let Some(slice) = initial_body_bytes.get(..to_take) {
            body.extend_from_slice(slice);
        }
    }

    while body.len() < content_len {
        let needed = content_len - body.len();
        let to_read = std::cmp::min(needed, chunk.len());
        let dest = chunk
            .get_mut(..to_read)
            .ok_or_else(|| HttpReadError::MalformedRequest("chunk slice error".to_owned()))?;
        let n = reader
            .read(dest)
            .map_err(|e| HttpReadError::Io(e.to_string()))?;
        if n == 0 {
            return Err(HttpReadError::UnexpectedEof);
        }
        let read_slice = dest
            .get(..n)
            .ok_or_else(|| HttpReadError::MalformedRequest("read slice error".to_owned()))?;
        body.extend_from_slice(read_slice);
    }

    Ok(ParsedHttpRequest {
        method,
        path: clean_path,
        body,
    })
}
