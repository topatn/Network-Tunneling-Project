//! Structured log redaction, owner-visible IP masking, and automated secret/IP leak
//! scanning (`T-046`, `PRIV-002`, `SEC-013`, `INV-5`, `contracts.md` §5.13).
//!
//! ## Redaction Rules (`PRIV-002`, `AGENTS.md` §7)
//!
//! Logs, diagnostics, and audit metadata must **never** contain:
//! 1. Access, refresh, or allocation tokens (`Authorization: Bearer ...` or compact 3-part
//!    `<b64>.<b64>.<b64>` tokens),
//! 2. Invitation secrets or PKCE verifiers (`secret`, `refresh_token`, `access_token`,
//!    `allocation_token`, `code_verifier`, `password`),
//! 3. Raw guest IP addresses (IPv4 or IPv6), or
//! 4. Raw payload bytes.

use std::net::IpAddr;

/// Sensitive field keys whose values must always be redacted in structured or text logs.
pub const SENSITIVE_FIELD_KEYS: &[&str] = &[
    "access_token",
    "allocation_token",
    "code_verifier",
    "password",
    "refresh_token",
    "secret",
    "token",
];

/// Mask a guest source IP address for owner-facing display in `GET /v1/tunnels/{id}/admissions`
/// (`contracts.md` §5.13: IPv4 masked to `/24`, IPv6 masked to `/48`).
///
/// Returns `None` if `raw_ip` is not a valid IP address.
pub fn mask_ip_for_owner(raw_ip: &str) -> Option<String> {
    let addr: IpAddr = raw_ip.trim().parse().ok()?;
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            Some(format!("{a}.{b}.{c}.0/24"))
        }
        IpAddr::V6(v6) => {
            let [s0, s1, s2, _, _, _, _, _] = v6.segments();
            Some(format!("{s0:x}:{s1:x}:{s2:x}::/48"))
        }
    }
}

/// Redact tokens, secrets, and raw IP addresses from a log line (`PRIV-002`, `T-046`).
///
/// Bounded to at most `16 KiB` of input so an oversized string cannot cause unbounded
/// allocation (`AGENTS.md` §4).
pub fn redact_log_line(raw: &str) -> String {
    let bounded: String = raw.chars().take(16 * 1024).collect();

    // First, if the line (or a JSON fragment) parses as JSON, redact sensitive keys and raw IP
    // string values recursively.
    if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(&bounded) {
        redact_json_value(&mut json_val);
        if let Ok(serialized) = serde_json::to_string(&json_val) {
            return serialized;
        }
    }

    let mut out = String::with_capacity(bounded.len());
    let mut prev_was_bearer = false;

    for (idx, token) in bounded.split_whitespace().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        if prev_was_bearer {
            out.push_str("[REDACTED-TOKEN]");
            prev_was_bearer = false;
            continue;
        }
        if token.eq_ignore_ascii_case("bearer") || token.eq_ignore_ascii_case("bearer:") {
            out.push_str("Bearer");
            prev_was_bearer = true;
            continue;
        }
        out.push_str(&redact_whitespace_token(token));
    }

    out
}

/// Recursively redact sensitive keys and raw IP addresses inside a `serde_json::Value`.
pub fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let lower = k.to_ascii_lowercase();
                if SENSITIVE_FIELD_KEYS.iter().any(|s| lower == *s) {
                    *v = serde_json::Value::String("[REDACTED-SECRET]".to_owned());
                } else {
                    redact_json_value(v);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_json_value(item);
            }
        }
        serde_json::Value::String(s) => {
            if is_raw_ip_token(s) {
                *s = "[REDACTED-IP]".to_owned();
            } else if looks_like_compact_jwt(s) {
                *s = "[REDACTED-TOKEN]".to_owned();
            }
        }
        _ => {}
    }
}

/// Scan a text buffer (log output, diagnostic bundle, or serialized audit stream) for any
/// unredacted tokens, secrets, or raw IP addresses (`T-046`, `PRIV-002`, `SEC-013`).
///
/// Returns the list of leak categories detected (empty when clean).
pub fn scan_for_sensitive_leaks(text: &str) -> Vec<&'static str> {
    let mut findings = Vec::new();

    // If the input is valid JSON, scan it structurally and return immediately to avoid
    // tokenizing JSON structural syntax as text.
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(text) {
        scan_json_leaks(&json_val, &mut findings);
        return findings;
    }

    let mut prev_bearer = false;
    for raw_tok in text.split_whitespace() {
        let trimmed = trim_punctuation(raw_tok);
        if prev_bearer {
            if !is_redacted_marker(trimmed) {
                push_unique(&mut findings, "unredacted_bearer_token");
            }
            prev_bearer = false;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("bearer") {
            prev_bearer = true;
            continue;
        }
        if is_raw_ip_token(trimmed) {
            push_unique(&mut findings, "raw_ip_address");
        }
        if looks_like_compact_jwt(trimmed) {
            push_unique(&mut findings, "compact_signed_token");
        }
        if let Some((k, v)) = split_kv(trimmed) {
            let k_lower = k.to_ascii_lowercase();
            if SENSITIVE_FIELD_KEYS.iter().any(|s| k_lower == *s)
                && !is_redacted_marker(v)
                && !v.is_empty()
            {
                push_unique(&mut findings, "unredacted_secret_field");
            }
            if is_raw_ip_token(v) {
                push_unique(&mut findings, "raw_ip_address");
            }
            if looks_like_compact_jwt(v) {
                push_unique(&mut findings, "compact_signed_token");
            }
        }
    }

    findings
}

fn is_redacted_marker(s: &str) -> bool {
    let clean = s.trim_matches(|c: char| matches!(c, '[' | ']'));
    clean.starts_with("REDACTED")
}

fn scan_json_leaks(val: &serde_json::Value, findings: &mut Vec<&'static str>) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let k_lower = k.to_ascii_lowercase();
                if SENSITIVE_FIELD_KEYS.iter().any(|s| k_lower == *s) {
                    let is_redacted = v.as_str().is_some_and(is_redacted_marker);
                    if !is_redacted {
                        push_unique(findings, "unredacted_secret_field");
                    }
                } else {
                    scan_json_leaks(v, findings);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                scan_json_leaks(item, findings);
            }
        }
        serde_json::Value::String(s) => {
            if is_raw_ip_token(s) {
                push_unique(findings, "raw_ip_address");
            }
            if looks_like_compact_jwt(s) {
                push_unique(findings, "compact_signed_token");
            }
        }
        _ => {}
    }
}

fn redact_whitespace_token(token: &str) -> String {
    if let Some((k, v)) = split_kv(token) {
        let k_lower = k.to_ascii_lowercase();
        if SENSITIVE_FIELD_KEYS.iter().any(|s| k_lower == *s) {
            return format!("{k}=[REDACTED-SECRET]");
        }
        let v_clean = trim_punctuation(v);
        if is_raw_ip_token(v_clean) {
            return format!("{k}=[REDACTED-IP]");
        }
        if looks_like_compact_jwt(v_clean) {
            return format!("{k}=[REDACTED-TOKEN]");
        }
    }

    let clean = trim_punctuation(token);
    if is_raw_ip_token(clean) {
        return "[REDACTED-IP]".to_owned();
    }
    if looks_like_compact_jwt(clean) {
        return "[REDACTED-TOKEN]".to_owned();
    }

    token.to_owned()
}

fn split_kv(token: &str) -> Option<(&str, &str)> {
    let (k, v) = token.split_once('=').or_else(|| token.split_once(':'))?;
    let k_clean = trim_punctuation(k);
    if k_clean.is_empty() {
        return None;
    }
    Some((k_clean, trim_punctuation(v)))
}

fn trim_punctuation(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    })
}

fn is_raw_ip_token(s: &str) -> bool {
    if s.is_empty() || is_redacted_marker(s) {
        return false;
    }
    if s.parse::<IpAddr>().is_ok() {
        return true;
    }
    // Also detect `ip:port` for IPv4 sockets.
    if let Some((host, port)) = s.rsplit_once(':') {
        if port.parse::<u16>().is_ok() && host.parse::<std::net::Ipv4Addr>().is_ok() {
            return true;
        }
    }
    false
}

fn looks_like_compact_jwt(s: &str) -> bool {
    let mut parts = s.split('.');
    let Some(p1) = parts.next() else {
        return false;
    };
    let Some(p2) = parts.next() else {
        return false;
    };
    let Some(p3) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let is_b64url = |part: &str| {
        part.len() >= 10
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    is_b64url(p1) && is_b64url(p2) && is_b64url(p3)
}

fn push_unique(vec: &mut Vec<&'static str>, item: &'static str) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}
