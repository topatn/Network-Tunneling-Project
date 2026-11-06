//! Tests for Bounded HTTP server request reader (`apps/desktop/src/http_server.rs`)
//! (`T-080`, `AGENTS.md` §4).

use std::error::Error;
use std::io::Cursor;

use gtt_desktop::http_server::{
    find_header_end, parse_content_length, read_http_request, HttpReadError, MAX_HTTP_HEADER_BYTES,
    MAX_HTTP_PAYLOAD_BYTES,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn http_header_end_detection_and_content_length_parsing() -> TestResult {
    let header_sample =
        b"POST /api/engine/status HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 42\r\n\r\n";
    let end = find_header_end(header_sample);
    assert_eq!(end, Some(header_sample.len()));

    let headers_text = "Host: 127.0.0.1\r\nContent-Length: 128\r\nConnection: close\r\n";
    assert_eq!(parse_content_length(headers_text), Some(128));

    let no_cl = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n";
    assert_eq!(parse_content_length(no_cl), None);

    Ok(())
}

#[test]
fn http_get_request_parses_cleanly() -> TestResult {
    let raw = b"GET /api/engine/server/status?foo=bar HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    let mut cursor = Cursor::new(raw.to_vec());

    let req = read_http_request(&mut cursor)?;
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/api/engine/server/status");
    assert!(req.body.is_empty());

    Ok(())
}

#[test]
fn http_post_fragmented_delivery_reassembles_full_body() -> TestResult {
    // Simulate TCP fragmentation: headers in buffer 1, body in buffer 2
    let headers = "POST /api/engine/configure HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 26\r\nContent-Type: application/json\r\n\r\n";
    let body = "{\"profile_id\":\"minecraft\"}";
    assert_eq!(body.len(), 26);

    let full_stream = format!("{headers}{body}");
    let mut cursor = Cursor::new(full_stream.into_bytes());

    let req = read_http_request(&mut cursor)?;
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/api/engine/configure");
    assert_eq!(req.body, body.as_bytes());

    Ok(())
}

#[test]
fn http_post_large_payload_reassembles_across_multiple_chunks() -> TestResult {
    let large_body = vec![b'x'; 32 * 1024]; // 32 KiB
    let headers = format!(
        "POST /api/engine/test HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        large_body.len()
    );
    let mut data = headers.into_bytes();
    data.extend_from_slice(&large_body);

    let mut cursor = Cursor::new(data);
    let req = read_http_request(&mut cursor)?;
    assert_eq!(req.method, "POST");
    assert_eq!(req.body.len(), 32 * 1024);
    assert_eq!(req.body, large_body);

    Ok(())
}

#[test]
fn http_payload_too_large_is_refused_with_typed_error() -> TestResult {
    let too_large = MAX_HTTP_PAYLOAD_BYTES + 1;
    let headers = format!("POST /api/upload HTTP/1.1\r\nContent-Length: {too_large}\r\n\r\n");
    let mut cursor = Cursor::new(headers.into_bytes());

    let err = match read_http_request(&mut cursor) {
        Err(e) => e,
        Ok(req) => return Err(format!("expected error, got Ok({req:?})").into()),
    };
    assert_eq!(
        err,
        HttpReadError::PayloadTooLarge {
            length: too_large,
            max: MAX_HTTP_PAYLOAD_BYTES
        }
    );

    Ok(())
}

#[test]
fn http_header_too_large_is_refused() -> TestResult {
    let mut large_headers = String::from("GET / HTTP/1.1\r\n");
    while large_headers.len() <= MAX_HTTP_HEADER_BYTES {
        large_headers.push_str("X-Long-Header: test-padding-string-for-bounded-headers\r\n");
    }
    large_headers.push_str("\r\n");

    let mut cursor = Cursor::new(large_headers.into_bytes());
    let err = match read_http_request(&mut cursor) {
        Err(e) => e,
        Ok(req) => return Err(format!("expected error, got Ok({req:?})").into()),
    };
    assert_eq!(err, HttpReadError::HeaderTooLarge);

    Ok(())
}

#[test]
fn http_unexpected_eof_on_truncated_body_is_reported() -> TestResult {
    let headers = "POST /api/test HTTP/1.1\r\nContent-Length: 100\r\n\r\npartial";
    let mut cursor = Cursor::new(headers.as_bytes().to_vec());

    let err = match read_http_request(&mut cursor) {
        Err(e) => e,
        Ok(req) => return Err(format!("expected error, got Ok({req:?})").into()),
    };
    assert_eq!(err, HttpReadError::UnexpectedEof);

    Ok(())
}
