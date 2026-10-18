//! INV-5: audit metadata holds no secrets and no raw guest IPs.
//!
//! `data-model.md` §4 specifies "a code-level allow-list and a secret-leak test". This
//! module is the allow-list; `tests/invariants_unit.rs` is the leak test.
//!
//! ## The allow-list is deliberately narrow, and that is a known limitation
//!
//! `contracts.md` does **not** enumerate the `action` values or the metadata keys any
//! route is allowed to write. Inventing a complete vocabulary here would be an
//! architectural decision taken inside a persistence task, and would either block every
//! legitimate caller or be widened until it enforced nothing. So the list below contains
//! only the keys this crate's own audit writer needs, and **adding a key is a deliberate,
//! reviewed code change** — which is what an allow-list is for.
//!
//! What the list does enforce today is the property INV-5 actually names, and it
//! enforces it on the *shape* of the data, not on the caller's intent:
//!
//! 1. only allow-listed keys, each matching the project's identifier shape;
//! 2. scalar values only — never a nested object or array, which is where request
//!    payloads would otherwise arrive;
//! 3. string values bounded in length, so a payload or a token cannot fit;
//! 4. **no string value that parses as an IP address**, which is the raw-guest-IP rule
//!    and the only part that catches a leak regardless of what the key was called.

use serde_json::Value;

use crate::error::Rejection;

/// Keys audit metadata may carry.
///
/// Matched exactly. This is an allow-list, not a prefix or pattern match: a key that
/// merely *starts* with an allowed key would let `reason_token` through.
pub const ALLOWED_METADATA_KEYS: &[&str] = &[
    "count",
    "profile_id",
    "reason",
    "state_after",
    "state_before",
];

/// Longest permitted string value. Chosen so a state name, a reason code, or a profile
/// id fits comfortably while a request body, a token, or a base64 blob does not.
const MAX_VALUE_LEN: usize = 256;

/// Longest permitted metadata value when rendered, as a second bound on nesting and
/// total size. Cheaper to reason about than a recursive depth limit.
const MAX_TOTAL_LEN: usize = 1024;

/// Validated audit metadata, ready to be written.
///
/// Constructing one of these is the only way to obtain metadata that has passed INV-5,
/// so the check cannot be skipped by using the struct directly: the inner value is
/// private and `new` is the sole constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditMetadata {
    value: Value,
}

/// Why a key or value was refused. Fixed phrases only: the offending value must never
/// appear in the reason, or the rejection log becomes a leak path (`AGENTS.md` §7).
fn reject(reason: &'static str) -> crate::error::PersistenceError {
    crate::error::PersistenceError::Rejected(Rejection::AuditMetadata(reason))
}

/// True when `s` parses as an IPv4 or IPv6 address.
///
/// `std::net::IpAddr::from_str` accepts both families and rejects almost everything
/// else, so it is stricter and simpler than any hand-written pattern — and it cannot be
/// fooled by a near-miss the way a regex can.
fn looks_like_ip_address(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// Build metadata from key/value pairs, applying the INV-5 rules.
///
/// Keys are checked against the allow-list and the project's identifier shape. Values
/// must be `null`, a boolean, a non-negative integer, or a bounded string that does not
/// parse as an IP address. Objects and arrays are refused outright.
///
/// Returns [`Rejection::AuditMetadata`] with a fixed reason on the first problem found.
pub fn build<I>(entries: I) -> Result<AuditMetadata, crate::error::PersistenceError>
where
    I: IntoIterator<Item = (String, Value)>,
{
    let mut map = serde_json::Map::new();

    for (key, value) in entries {
        if !ALLOWED_METADATA_KEYS.contains(&key.as_str()) {
            return Err(reject("key is not on the allow-list"));
        }
        // The allow-list is the primary control; the shape check is defence in depth for
        // the day the list grows, so a key can never be both allowed and malformed.
        if !is_project_identifier(&key) {
            return Err(reject("key does not match the project identifier shape"));
        }
        check_value(&value)?;
        if map.insert(key, value).is_some() {
            return Err(reject("duplicate key"));
        }
    }

    let value = Value::Object(map);
    if value.to_string().len() > MAX_TOTAL_LEN {
        return Err(reject("metadata exceeds the total size bound"));
    }
    Ok(AuditMetadata { value })
}

/// True when `s` matches `^[a-z][a-z0-9_]{0,63}$`, the shape `AGENTS.md` §4 gives
/// mapping keys and which is reused here for metadata keys.
pub fn is_project_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut len = 1;
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return false;
        }
        len += 1;
    }
    len <= 64
}

fn check_value(value: &Value) -> Result<(), crate::error::PersistenceError> {
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Number(n) => {
            if n.is_f64() {
                // Floats are refused: usage counters and durations are integers, and a
                // float here is usually an accidental serialisation of something else.
                Err(reject("non-integer number"))
            } else {
                Ok(())
            }
        }
        Value::String(s) => {
            if s.chars().count() > MAX_VALUE_LEN {
                return Err(reject("string value exceeds the length bound"));
            }
            if looks_like_ip_address(s) {
                return Err(reject("string value looks like a raw IP address"));
            }
            Ok(())
        }
        // Nested containers are where a request payload would be smuggled in.
        Value::Array(_) => Err(reject("array values are not permitted")),
        Value::Object(_) => Err(reject("object values are not permitted")),
    }
}

impl AuditMetadata {
    /// The validated value, for binding to a `jsonb` column.
    pub fn as_value(&self) -> &Value {
        &self.value
    }
}

impl Default for AuditMetadata {
    fn default() -> Self {
        // `{}` is valid under every rule above, so this cannot fail and does not need to
        // be fallible at the type level.
        Self {
            value: Value::Object(serde_json::Map::new()),
        }
    }
}
