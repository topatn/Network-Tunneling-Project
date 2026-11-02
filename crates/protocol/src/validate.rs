//! Pure, bounded validators for wire-format primitives (`contracts.md` §§1, 3, 15, `INV-1`).
//!
//! ## Why format checks live here in pure Rust
//!
//! `contracts.md`'s 2026-10-17 status note recorded an open question for T-031:
//!
//! > Whether the `uuid` and `date-time` string formats are actually enforced remains
//! > unknown, since no validator has ever run here.
//!
//! Standard JSON Schema draft 2020-12 treats `format` as an annotation by default unless the
//! format-assertion vocabulary is enabled. Relying on a schema engine's default `format`
//! behaviour would leave `"not-a-uuid"` or `"not-a-timestamp"` accepted at the wire boundary.
//! Enforcing both formats here — with bounded, panic-free byte parsers and no external regex
//! or datetime crate — makes format enforcement unconditional in both `gtt-protocol` and the
//! `contracts/examples.json` conformance suite.

use std::fmt;

/// Maximum JSON nesting depth inspected when scanning untrusted payloads for `INV-1`
/// forbidden local-target keys.
pub const MAX_JSON_DEPTH: usize = 16;

/// Field-name stems that would carry a host-local destination (`INV-1`, `ADR-003`).
///
/// Kept identical to `crates/control-plane/src/db/invariants.rs` so the wire boundary and the
/// persistence boundary refuse the exact same vocabulary.
pub const FORBIDDEN_LOCAL_TARGET_STEMS: &[&str] = &[
    "bind_addr",
    "bind_port",
    "dest_addr",
    "dest_host",
    "dest_port",
    "destination",
    "local_addr",
    "local_host",
    "local_port",
    "local_target",
    "target_addr",
    "target_host",
    "target_port",
];

/// Why a wire payload failed domain or format validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// `INV-1`: a field name carrying a host-local destination was present.
    LocalTargetField(String),
    /// A JSON payload exceeded [`MAX_JSON_DEPTH`].
    JsonTooDeep,
    /// A field failed a `contracts.md` §15 validation rule.
    InvalidField {
        /// The field path or name.
        field: &'static str,
        /// A short static reason safe to include in error details (`AGENTS.md` §7).
        issue: &'static str,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalTargetField(name) => {
                write!(f, "field {name:?} is forbidden on the wire (INV-1)")
            }
            Self::JsonTooDeep => {
                write!(f, "JSON nesting depth exceeds maximum of {MAX_JSON_DEPTH}")
            }
            Self::InvalidField { field, issue } => write!(f, "invalid {field}: {issue}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// True when `field` contains any stem that would name a host-local destination (`INV-1`).
pub fn is_local_target_field(field: &str) -> bool {
    let lowered = field.to_ascii_lowercase();
    FORBIDDEN_LOCAL_TARGET_STEMS
        .iter()
        .any(|stem| lowered.contains(stem))
}

/// Recursively scan a JSON value for any object key that violates `INV-1`.
///
/// Bounded by [`MAX_JSON_DEPTH`] so a deeply nested untrusted document cannot exhaust the
/// call stack (`AGENTS.md` §4).
pub fn scan_json_for_local_target(value: &serde_json::Value) -> Result<(), ValidationError> {
    scan_json_depth(value, 0)
}

fn scan_json_depth(value: &serde_json::Value, depth: usize) -> Result<(), ValidationError> {
    if depth > MAX_JSON_DEPTH {
        return Err(ValidationError::JsonTooDeep);
    }
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if is_local_target_field(k) {
                    return Err(ValidationError::LocalTargetField(k.clone()));
                }
                scan_json_depth(v, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for item in items {
                scan_json_depth(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Validate a profile or mapping identifier against `^[a-z][a-z0-9-]{0,31}$` (`contracts.md` §15).
pub fn is_valid_identifier(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return false;
    };
    if bytes.len() > 32 || !first.is_ascii_lowercase() {
        return false;
    }
    rest.iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Validate a tunnel name (`contracts.md` §15: 1–64 characters after trimming, no control characters).
pub fn is_valid_tunnel_name(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let char_count = s.chars().count();
    let trimmed_count = trimmed.chars().count();
    (1..=64).contains(&trimmed_count) && char_count <= 64 && !s.chars().any(char::is_control)
}

/// Validate an invitation secret against `^[A-Za-z0-9_-]{22,64}$` (`contracts.md` §15).
pub fn is_valid_invitation_secret(s: &str) -> bool {
    let bytes = s.as_bytes();
    if !(22..=64).contains(&bytes.len()) {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

/// Validate a canonical 36-character hyphenated UUID (`8-4-4-4-12` hex digits).
pub fn is_valid_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (idx, &b) in bytes.iter().enumerate() {
        if idx == 8 || idx == 13 || idx == 18 || idx == 23 {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Validate an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff...]Z`, `contracts.md` §1).
pub fn is_valid_rfc3339_utc(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Minimum valid length is 20 bytes: "YYYY-MM-DDTHH:MM:SSZ". Maximum bounded to 35.
    if !(20..=35).contains(&bytes.len()) {
        return false;
    }
    let Some(year) = parse_4_digits(bytes, 0) else {
        return false;
    };
    if byte_at(bytes, 4) != Some(b'-') {
        return false;
    }
    let Some(month) = parse_2_digits(bytes, 5) else {
        return false;
    };
    if byte_at(bytes, 7) != Some(b'-') {
        return false;
    }
    let Some(day) = parse_2_digits(bytes, 8) else {
        return false;
    };
    let Some(sep) = byte_at(bytes, 10) else {
        return false;
    };
    if sep != b'T' && sep != b't' {
        return false;
    }
    let Some(hour) = parse_2_digits(bytes, 11) else {
        return false;
    };
    if byte_at(bytes, 13) != Some(b':') {
        return false;
    }
    let Some(minute) = parse_2_digits(bytes, 14) else {
        return false;
    };
    if byte_at(bytes, 16) != Some(b':') {
        return false;
    }
    let Some(second) = parse_2_digits(bytes, 17) else {
        return false;
    };

    if year < 1970 || !(1..=12).contains(&month) {
        return false;
    }
    let max_day = days_in_month(year, month);
    if !(1..=max_day).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return false;
    }

    let Some(tail) = bytes.get(19..) else {
        return false;
    };
    if tail == b"Z" || tail == b"z" {
        return true;
    }
    let Some((&dot, frac_and_z)) = tail.split_first() else {
        return false;
    };
    if dot != b'.' {
        return false;
    }
    let Some((&last, frac)) = frac_and_z.split_last() else {
        return false;
    };
    if (last != b'Z' && last != b'z') || frac.is_empty() || frac.len() > 9 {
        return false;
    }
    frac.iter().all(u8::is_ascii_digit)
}

fn byte_at(bytes: &[u8], index: usize) -> Option<u8> {
    bytes.get(index).copied()
}

fn parse_2_digits(bytes: &[u8], start: usize) -> Option<u32> {
    let d0 = byte_at(bytes, start)?;
    let d1 = byte_at(bytes, start + 1)?;
    if !d0.is_ascii_digit() || !d1.is_ascii_digit() {
        return None;
    }
    Some(u32::from(d0 - b'0') * 10 + u32::from(d1 - b'0'))
}

fn parse_4_digits(bytes: &[u8], start: usize) -> Option<u32> {
    let hi = parse_2_digits(bytes, start)?;
    let lo = parse_2_digits(bytes, start + 2)?;
    Some(hi * 100 + lo)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap =
                (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}
