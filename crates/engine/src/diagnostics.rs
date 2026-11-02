//! Host-exported diagnostics bundle and redaction pipeline (`T-075`, `FR-013`, `PRIV-001`,
//! `PRIV-002`).
//!
//! ## Guarantees
//! 1. **No automatic telemetry (`PRIV-001`):** Diagnostics are built and exported only when the
//!    user explicitly requests a bundle. Nothing in this module performs network I/O.
//! 2. **Strict redaction (`PRIV-002`, `FR-013`):** Every log line, status detail, and event
//!    summary passes through [`redact_diagnostic_text`] before entering [`DiagnosticsBundle`],
//!    stripping bearer tokens, invitation secrets (`gtt_inv_...`), JWS allocation tokens
//!    (`eyJ...`), server management / RCON passwords, raw IPv4/IPv6 addresses, and payload byte
//!    dumps.
//! 3. **Bounded size (`AGENTS.md` §4):** Log lines and events are capped at
//!    [`MAX_DIAGNOSTIC_LOG_LINES`], [`MAX_DIAGNOSTIC_EVENTS`], and [`MAX_DIAGNOSTIC_LINE_CHARS`].

use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::lifecycle::{EngineEvent, EngineStatusSnapshot, SessionConnectivityMode};

/// Current schema version for exported diagnostics bundles.
pub const DIAGNOSTICS_SCHEMA_VERSION: u32 = 1;

/// Maximum number of log lines retained in a diagnostics bundle (`AGENTS.md` §4).
pub const MAX_DIAGNOSTIC_LOG_LINES: usize = 500;

/// Maximum number of recent engine events retained in a diagnostics bundle (`AGENTS.md` §4).
pub const MAX_DIAGNOSTIC_EVENTS: usize = 256;

/// Maximum characters per diagnostic text field or log line (`AGENTS.md` §4).
pub const MAX_DIAGNOSTIC_LINE_CHARS: usize = 512;

/// Disclosure string included in every exported diagnostics bundle (`PRIV-001`, `PRIV-002`).
pub const DIAGNOSTICS_PRIVACY_NOTICE: &str = "Exported locally on user request. No automatic telemetry is sent (PRIV-001). Tokens, secrets, raw IP addresses, and payload bytes are redacted (PRIV-002).";

/// Placeholder replacing a bearer or JWS token (`PRIV-002`).
pub const REDACTED_TOKEN_MARKER: &str = "[REDACTED_TOKEN]";

/// Placeholder replacing an invitation secret (`PRIV-002`, `SEC-007`).
pub const REDACTED_INVITATION_MARKER: &str = "[REDACTED_INVITATION_SECRET]";

/// Placeholder replacing a password or key-value secret (`PRIV-002`).
pub const REDACTED_SECRET_MARKER: &str = "[REDACTED_SECRET]";

/// Placeholder replacing a raw IPv4 or IPv6 address (`PRIV-002`).
pub const REDACTED_IP_MARKER: &str = "[REDACTED_IP]";

/// Placeholder replacing raw payload bytes (`PRIV-002`).
pub const REDACTED_PAYLOAD_MARKER: &str = "[REDACTED_PAYLOAD]";

/// Sensitive key prefixes in `key=value` or `key: value` log tokens whose value must be redacted.
const SENSITIVE_ASSIGNMENT_KEYS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "allocation_token",
    "secret",
    "invitation_secret",
    "password",
    "rcon.password",
    "management-server-secret",
    "management_server_secret",
    "keystore-password",
    "authorization",
    "cookie",
    "set-cookie",
];

/// Payload-related key prefixes in `key=value` or `key: value` log tokens whose value must be
/// replaced with [`REDACTED_PAYLOAD_MARKER`].
const PAYLOAD_ASSIGNMENT_KEYS: &[&str] = &[
    "payload",
    "payload_bytes",
    "payload_hex",
    "raw_payload",
    "datagram_payload",
    "packet_bytes",
];

/// Summary of a mapping included in [`DiagnosticsStatusSummary`] without exposing raw host/relay
/// IP literals (`PRIV-002`, `INV-1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsMappingSummary {
    /// Mapping key (`"game"`, `"rcon"`, `"query"`, etc.).
    pub mapping_key: String,
    /// Protocol (`"tcp"` or `"udp"`).
    pub protocol: String,
    /// Public port number (`1024..=65535`).
    pub public_port: u16,
}

/// Summary of a local reachability probe result with redacted detail (`FR-011`, `PRIV-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsProbeSummary {
    /// Mapping key probed.
    pub mapping_key: String,
    /// Probe level (`"L0"` or `"L1"`).
    pub level: String,
    /// Probe status (`"reachable"`, `"unreachable"`, or `"unknown"`).
    pub status: String,
    /// Redacted diagnostic detail.
    pub detail: String,
}

/// Redacted multi-layer status snapshot inside a [`DiagnosticsBundle`] (`FR-010`, `FR-013`,
/// `UX-002`, `PRIV-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsStatusSummary {
    /// Active tunnel UUID, if any.
    pub tunnel_id: Option<String>,
    /// Active allocation UUID, if any.
    pub allocation_id: Option<String>,
    /// Layer 1: Engine tunnel session state (`Idle`, `Active`, `Reconnecting`, etc.).
    pub tunnel_state: String,
    /// Most recent sampled `ping`/`pong` RTT in milliseconds.
    pub last_rtt_ms: Option<u64>,
    /// Enabled public mapping summaries (ports and keys only; no raw IPs).
    pub public_mappings: Vec<DiagnosticsMappingSummary>,
    /// Layer 2: Independent local probe summaries.
    pub local_probes: Vec<DiagnosticsProbeSummary>,
    /// Layer 3: Independent observed relay traffic count (`FR-012`).
    pub players_seen_distinct_sources: u32,
    /// Active TCP flows observed.
    pub players_seen_tcp_flows: u32,
    /// Active UDP flows observed.
    pub players_seen_udp_flows: u32,
    /// Mandatory disclosure label for observed traffic.
    pub players_seen_label: String,
    /// Layer 4: Managed server state (`SRV-006`).
    pub managed_server_state: String,
    /// Managed server profile ID, if configured.
    pub managed_server_profile_id: Option<String>,
    /// Managed server version, if known.
    pub managed_server_version: Option<String>,
    /// Redacted last log excerpt from the managed server.
    pub managed_server_last_log: Option<String>,
    /// Layer 5: Confirmed in-game player count (if MSMP is enabled; player names and IPs are
    /// excluded per `PRIV-002`).
    pub confirmed_players_online: Option<u32>,
}

/// Exportable diagnostics bundle (`T-075`, `FR-013`, `PRIV-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsBundle {
    /// Bundle format version ([`DIAGNOSTICS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Export timestamp in Unix epoch milliseconds.
    pub generated_at_epoch_ms: u64,
    /// Session connectivity mode (`"gtt_relay"` or `"tailscale_node_share"`).
    pub connectivity_mode: String,
    /// Privacy notice (`PRIV-001`, `PRIV-002`).
    pub privacy_notice: String,
    /// Redacted 5-layer status snapshot.
    pub status: DiagnosticsStatusSummary,
    /// Redacted recent engine events (bounded by [`MAX_DIAGNOSTIC_EVENTS`]).
    pub recent_events: Vec<String>,
    /// Redacted log lines (bounded by [`MAX_DIAGNOSTIC_LOG_LINES`]).
    pub redacted_logs: Vec<String>,
}

impl DiagnosticsBundle {
    /// Build a redacted [`DiagnosticsBundle`] from an [`EngineStatusSnapshot`], connectivity mode,
    /// recent [`EngineEvent`]s, and raw log lines.
    pub fn build(
        generated_at_epoch_ms: u64,
        connectivity_mode: SessionConnectivityMode,
        snapshot: &EngineStatusSnapshot,
        events: &[EngineEvent],
        raw_logs: &[&str],
    ) -> Self {
        let mode_str = match connectivity_mode {
            SessionConnectivityMode::GttRelay => "gtt_relay".to_owned(),
            SessionConnectivityMode::TailscaleNodeShare => "tailscale_node_share".to_owned(),
        };

        let public_mappings = snapshot
            .public_endpoints
            .iter()
            .map(|ep| DiagnosticsMappingSummary {
                mapping_key: ep.mapping_key.clone(),
                protocol: ep.protocol.as_str().to_owned(),
                public_port: ep.public_port,
            })
            .collect();

        let local_probes = snapshot
            .local_probes
            .iter()
            .map(|p| DiagnosticsProbeSummary {
                mapping_key: p.mapping_key.clone(),
                level: format!("{:?}", p.level),
                status: format!("{:?}", p.status),
                detail: redact_diagnostic_text(&p.detail),
            })
            .collect();

        let confirmed_players_online = if snapshot.confirmed_presence.enabled {
            Some(snapshot.confirmed_presence.confirmed_players)
        } else {
            None
        };

        let managed_server_last_log = if snapshot.managed_server.last_log_excerpt.is_empty() {
            None
        } else {
            Some(redact_diagnostic_text(
                &snapshot.managed_server.last_log_excerpt,
            ))
        };

        let managed_server_profile_id = if snapshot.managed_server.profile_id.is_empty() {
            None
        } else {
            Some(snapshot.managed_server.profile_id.clone())
        };

        let status = DiagnosticsStatusSummary {
            tunnel_id: snapshot.tunnel_id.clone(),
            allocation_id: snapshot.allocation_id.clone(),
            tunnel_state: format!("{:?}", snapshot.tunnel_state),
            last_rtt_ms: snapshot.last_rtt_ms,
            public_mappings,
            local_probes,
            players_seen_distinct_sources: snapshot.players_seen.distinct_sources,
            players_seen_tcp_flows: snapshot.players_seen.tcp_flows,
            players_seen_udp_flows: snapshot.players_seen.udp_flows,
            players_seen_label: snapshot.players_seen.label.to_owned(),
            managed_server_state: format!("{:?}", snapshot.managed_server.state),
            managed_server_profile_id,
            managed_server_version: snapshot.managed_server.version.clone(),
            managed_server_last_log,
            confirmed_players_online,
        };

        let recent_events = events
            .iter()
            .rev()
            .take(MAX_DIAGNOSTIC_EVENTS)
            .rev()
            .map(summarize_and_redact_event)
            .collect();

        let redacted_logs = raw_logs
            .iter()
            .rev()
            .take(MAX_DIAGNOSTIC_LOG_LINES)
            .rev()
            .map(|line| redact_diagnostic_text(line))
            .collect();

        Self {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION,
            generated_at_epoch_ms,
            connectivity_mode: mode_str,
            privacy_notice: DIAGNOSTICS_PRIVACY_NOTICE.to_owned(),
            status,
            recent_events,
            redacted_logs,
        }
    }

    /// Serialize the bundle as pretty-printed JSON.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Write the redacted JSON bundle to `path`.
    pub fn write_to_path(&self, path: &Path) -> std::io::Result<()> {
        let json = self
            .to_json_pretty()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json.as_bytes())
    }
}

fn summarize_and_redact_event(event: &EngineEvent) -> String {
    match event {
        EngineEvent::TunnelStatusChanged { state, reason } => {
            format!(
                "tunnel_status_changed state={state:?} reason={}",
                redact_diagnostic_text(reason)
            )
        }
        EngineEvent::LocalProbeResult(probe) => {
            format!(
                "local_probe_result mapping={} level={:?} status={:?} detail={}",
                probe.mapping_key,
                probe.level,
                probe.status,
                redact_diagnostic_text(&probe.detail)
            )
        }
        EngineEvent::PlayersSeen(ps) => {
            format!(
                "players_seen distinct_sources={} tcp_flows={} udp_flows={}",
                ps.distinct_sources, ps.tcp_flows, ps.udp_flows
            )
        }
        EngineEvent::ManagedServerStatusChanged { state, progress } => {
            format!("managed_server_status_changed state={state:?} progress={progress:?}")
        }
        EngineEvent::ManagedServerLog { excerpt } => {
            format!(
                "managed_server_log excerpt={}",
                redact_diagnostic_text(excerpt)
            )
        }
        EngineEvent::ManagedServerUpdate { state } => {
            format!(
                "managed_server_update state={}",
                redact_diagnostic_text(state)
            )
        }
        EngineEvent::ConfirmedPresence(cp) => {
            format!(
                "confirmed_presence enabled={} count={}",
                cp.enabled, cp.confirmed_players
            )
        }
        EngineEvent::EngineError { code, message } => {
            format!(
                "engine_error code={code} message={}",
                redact_diagnostic_text(message)
            )
        }
    }
}

/// Redact sensitive tokens, invitation secrets, passwords, payload dumps, and raw IPv4/IPv6
/// addresses from a diagnostic string (`PRIV-002`, `FR-013`).
pub fn redact_diagnostic_text(input: &str) -> String {
    let mut out = Vec::new();
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let mut idx = 0usize;

    while let Some(&raw_tok) = tokens.get(idx) {
        // Handle `Bearer <token>` or `Authorization: Bearer <token>`.
        if raw_tok.eq_ignore_ascii_case("bearer")
            || raw_tok.eq_ignore_ascii_case("bearer:")
            || raw_tok.eq_ignore_ascii_case("\"bearer")
        {
            out.push(format!("Bearer {REDACTED_TOKEN_MARKER}"));
            idx = idx.saturating_add(2);
            continue;
        }

        // Handle `key: <value>` where `key:` is followed by a sensitive value in the next token.
        let trimmed_key_colon = raw_tok
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';'));
        if let Some(key_without_colon) = trimmed_key_colon.strip_suffix(':') {
            let lower_key = key_without_colon.to_ascii_lowercase();
            if SENSITIVE_ASSIGNMENT_KEYS.iter().any(|&k| k == lower_key) {
                out.push(format!("{key_without_colon}: {REDACTED_SECRET_MARKER}"));
                idx = idx.saturating_add(2);
                continue;
            }
            if PAYLOAD_ASSIGNMENT_KEYS.iter().any(|&k| k == lower_key) {
                out.push(format!("{key_without_colon}: {REDACTED_PAYLOAD_MARKER}"));
                idx = idx.saturating_add(2);
                continue;
            }
        }

        out.push(redact_single_token(raw_tok));
        idx = idx.saturating_add(1);
    }

    let joined = out.join(" ");
    truncate_chars(&joined, MAX_DIAGNOSTIC_LINE_CHARS)
}

fn redact_single_token(tok: &str) -> String {
    // Check `key=value` or `key:value` assignments first.
    for sep in ['=', ':'] {
        if let Some((lhs, rhs)) = tok.split_once(sep) {
            let clean_lhs = lhs
                .trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | '[' | '{' | ',' | ';'))
                .to_ascii_lowercase();
            let clean_rhs = rhs.trim_matches(|c: char| {
                matches!(
                    c,
                    '"' | '\'' | ',' | ';' | '(' | ')' | '{' | '}' | '<' | '>'
                )
            });
            if clean_rhs.starts_with("gtt_inv_") {
                return format!("{lhs}{sep}{REDACTED_INVITATION_MARKER}");
            }
            if looks_like_jws(clean_rhs) {
                return format!("{lhs}{sep}{REDACTED_TOKEN_MARKER}");
            }
            if SENSITIVE_ASSIGNMENT_KEYS.iter().any(|&k| k == clean_lhs) {
                return format!("{lhs}{sep}{REDACTED_SECRET_MARKER}");
            }
            if PAYLOAD_ASSIGNMENT_KEYS.iter().any(|&k| k == clean_lhs) {
                return format!("{lhs}{sep}{REDACTED_PAYLOAD_MARKER}");
            }
            // Even if `lhs` is benign (e.g. `addr=198.51.100.42:25565`), redact `rhs` if needed!
            let redacted_rhs = redact_value_atom(rhs);
            if redacted_rhs != rhs {
                return format!("{lhs}{sep}{redacted_rhs}");
            }
        }
    }

    redact_value_atom(tok)
}

fn redact_value_atom(atom: &str) -> String {
    let trimmed = atom.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '{' | '}' | '<' | '>'
        )
    });

    if trimmed.is_empty() {
        return atom.to_owned();
    }

    if trimmed.starts_with("gtt_inv_") {
        return REDACTED_INVITATION_MARKER.to_owned();
    }
    if looks_like_jws(trimmed) {
        return REDACTED_TOKEN_MARKER.to_owned();
    }
    if looks_like_hex_payload_dump(trimmed) {
        return REDACTED_PAYLOAD_MARKER.to_owned();
    }
    if contains_raw_ip(trimmed) {
        return REDACTED_IP_MARKER.to_owned();
    }

    atom.to_owned()
}

fn looks_like_jws(s: &str) -> bool {
    if !s.starts_with("eyJ") {
        return false;
    }
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
    parts.next().is_none() && !p1.is_empty() && !p2.is_empty() && !p3.is_empty()
}

fn looks_like_hex_payload_dump(s: &str) -> bool {
    let hex_body = s.strip_prefix("0x").unwrap_or(s);
    // Treat contiguous hex strings >= 48 chars without dashes (so 36-char UUIDs are preserved)
    // as raw payload or digest dumps.
    hex_body.len() >= 48 && hex_body.bytes().all(|b| b.is_ascii_hexdigit())
}

fn contains_raw_ip(s: &str) -> bool {
    // Strip optional `/prefix` or `:port` and bracketed `[ipv6]:port`.
    let without_cidr = s.split('/').next().unwrap_or(s);
    if let Some(rest) = without_cidr.strip_prefix('[') {
        if let Some((inside, _port)) = rest.split_once(']') {
            if inside.parse::<Ipv6Addr>().is_ok() || inside.parse::<Ipv4Addr>().is_ok() {
                return true;
            }
        }
    }
    let unbracketed = without_cidr.trim_matches(|c: char| matches!(c, '[' | ']'));
    if unbracketed.parse::<Ipv4Addr>().is_ok() || unbracketed.parse::<Ipv6Addr>().is_ok() {
        return true;
    }
    if let Some((host, _port)) = unbracketed.rsplit_once(':') {
        let clean_host = host.trim_matches(|c: char| matches!(c, '[' | ']'));
        if clean_host.parse::<Ipv4Addr>().is_ok() || clean_host.parse::<Ipv6Addr>().is_ok() {
            return true;
        }
    }
    false
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...[truncated]");
            break;
        }
        out.push(ch);
    }
    out
}

/// Automated security scanner verifying that an exported diagnostics bundle JSON string contains
/// no unredacted tokens, invitation secrets, passwords, payload dumps, or raw IP addresses
/// (`T-075`, `FR-013`, `PRIV-002`).
///
/// Returns an empty `Vec` if clean, or a list of leak descriptions if any violation is found.
pub fn scan_diagnostics_for_leaks(bundle_json: &str) -> Vec<String> {
    let mut findings = Vec::new();

    if bundle_json.contains("gtt_inv_") {
        findings.push("found unredacted invitation secret prefix `gtt_inv_`".to_owned());
    }

    for raw_tok in bundle_json.split_whitespace() {
        let trimmed = raw_tok.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
            )
        });
        if looks_like_jws(trimmed) {
            findings.push("found unredacted 3-part JWS token (`eyJ...`)".to_owned());
        }
        if looks_like_hex_payload_dump(trimmed) {
            findings.push(format!("found unredacted hex payload dump (`{trimmed}`)"));
        }
        if contains_raw_ip(trimmed) {
            findings.push(format!("found raw IP address (`{trimmed}`)"));
        }
    }

    for forbidden_marker in [
        "rcon.password=",
        "management-server-secret=",
        "allocation_token=",
        "refresh_token=",
        "access_token=",
        "payload_bytes=",
        "raw_payload=",
    ] {
        if let Some(pos) = bundle_json.find(forbidden_marker) {
            let after = bundle_json
                .get(pos + forbidden_marker.len()..)
                .unwrap_or("");
            if !after.starts_with(REDACTED_SECRET_MARKER)
                && !after.starts_with(REDACTED_PAYLOAD_MARKER)
                && !after.starts_with(REDACTED_TOKEN_MARKER)
            {
                findings.push(format!(
                    "found unredacted assignment for `{forbidden_marker}`"
                ));
            }
        }
    }

    findings
}
