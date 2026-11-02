//! Internal relay ↔ control-plane route handlers (`api.md` Routes 19–20, `contracts.md` §7,
//! `REL-004`, `SEC-009`, `T-048`):
//!
//! * Route 19: `GET /internal/v1/relays/{relay_id}/state?cursor=<opaque>` — [`get_relay_state`]
//! * Route 20: `POST /internal/v1/relays/{relay_id}/report` — [`report_relay_status_and_usage`]
//!
//! ## Relay Restart Recovery (`REL-004`)
//!
//! When a relay starts up or restarts with an empty in-memory table, it polls
//! `GET /internal/v1/relays/{relay_id}/state` with `cursor = None`. The control plane returns
//! the full active state for that relay (`full_snapshot: true`): all live reservations
//! (`reserved` or `active`) with their published `allocation_ports`, all live
//! `guest_admissions` for tunnels hosted on that relay, the relay's `draining` flag, and the
//! active/overlap token verification keys (`kid`). Subsequent polls pass `Some(next_cursor)`
//! to receive delta changes (new or released reservations and added or revoked admissions)
//! since that cursor.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use gtt_protocol::http::{ApiErrorCode, ErrorDetail, Protocol as WireProtocol};
use gtt_protocol::token::SigningKeyRing;
use gtt_protocol::validate::{
    is_valid_rfc3339_utc, is_valid_uuid, scan_json_for_local_target, ValidationError,
};

use crate::db::entities::{
    AllocationPort, GuestAdmission, RelayAllocation, RelayNode, RelayStatus,
};
use crate::db::ops::{add_usage, UsageDelta};
use crate::db::relay::{
    activate_allocation, get_relay_node, list_allocation_ports, list_allocations_for_relay,
    set_relay_status,
};
use crate::db::sharing::list_all_admissions_for_tunnel;
use crate::policy::{decide, Actor, Decision, ErrorCode as PolicyErrorCode, Operation, Target};
use crate::routes::tunnels::{ApiFailure, ApiResponse, MAX_REQUEST_BODY_BYTES};

/// One published port inside a [`RelayReservationDelta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayPortSpecDto {
    /// Mapping UUID.
    pub mapping_id: String,
    /// Transport protocol (`"tcp"` or `"udp"`).
    pub protocol: WireProtocol,
    /// Public port on the relay (`1024..=65535`).
    pub public_port: u16,
}

/// One allocation reservation (live or newly released/expired) returned by
/// `GET /internal/v1/relays/{relay_id}/state` (`contracts.md` §7, `REL-004`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayReservationDelta {
    /// Allocation UUID.
    pub allocation_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Allocation state (`"reserved"`, `"active"`, `"released"`, or `"expired"`).
    pub state: String,
    /// Allocation expiration timestamp (RFC 3339 UTC).
    pub expires_at: String,
    /// Published public ports for this allocation.
    pub ports: Vec<RelayPortSpecDto>,
}

/// One guest source-IP admission change returned to the relay (`contracts.md` §7, `E-13`).
///
/// Never logged (`PRIV-002`, `INV-5`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayAdmissionDelta {
    /// Admission UUID.
    pub admission_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Guest source IP enforced by the relay.
    pub source_ip: String,
    /// Whether this admission is currently active (`true`) or revoked/expired (`false`).
    pub active: bool,
    /// Expiration timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

/// Metadata for a token verification key (`kid`) distributed to a relay (`contracts.md` §7,
/// `SEC-016`, `T-050`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayVerificationKeyDto {
    /// Key identifier (`kid`).
    pub kid: String,
    /// Optional overlap-window expiration Unix timestamp (`None` for the primary active key).
    pub valid_until_epoch_s: Option<i64>,
}

/// Response payload for `GET /internal/v1/relays/{relay_id}/state` (`contracts.md` §7,
/// `REL-004`, `T-048`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayStateResponse {
    /// Relay UUID.
    pub relay_id: String,
    /// True when the response is a full recovery snapshot (`cursor == None`, `REL-004`).
    pub full_snapshot: bool,
    /// Opaque monotonic cursor to pass on the next state poll.
    pub next_cursor: String,
    /// True when the relay is in `draining` state and should refuse new reservations.
    pub draining: bool,
    /// Active and overlap token verification keys (`kid`).
    pub verification_keys: Vec<RelayVerificationKeyDto>,
    /// Reservations (all live reservations on a full snapshot, or changed reservations since
    /// `cursor`).
    pub reservations: Vec<RelayReservationDelta>,
    /// Guest source-IP admissions (all live admissions on a full snapshot, or changed
    /// admissions since `cursor`).
    pub admissions: Vec<RelayAdmissionDelta>,
}

/// Usage and flow metrics reported for a single allocation interval in [`RelayReportRequest`]
/// (`contracts.md` §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationUsageReportItem {
    /// Allocation UUID.
    pub allocation_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// True if the engine has completed a `hello` handshake on this allocation.
    pub connected: bool,
    /// Interval start timestamp (RFC 3339 UTC).
    pub interval_start: String,
    /// Bytes sent from guests during the interval (`>= 0`).
    pub bytes_from_guests: i64,
    /// Bytes sent to guests during the interval (`>= 0`).
    pub bytes_to_guests: i64,
    /// Peak concurrent TCP + UDP flows observed during the interval (`>= 0`).
    pub peak_flows: i32,
    /// Oversized or malformed UDP datagrams dropped during the interval (`>= 0`).
    pub datagrams_dropped: i64,
}

/// Request body for `POST /internal/v1/relays/{relay_id}/report` (`contracts.md` §7, `T-048`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayReportRequest {
    /// Whether the relay considers itself healthy.
    pub healthy: bool,
    /// Per-allocation usage deltas and connection observations (`0..=200` items).
    pub allocations: Vec<AllocationUsageReportItem>,
}

impl RelayReportRequest {
    /// Validate bounds and format rules on [`RelayReportRequest`].
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.allocations.len() > 200 {
            return Err(ValidationError::InvalidField {
                field: "allocations",
                issue: "at most 200 allocation report items are allowed per batch",
            });
        }
        for item in &self.allocations {
            if !is_valid_uuid(&item.allocation_id) || !is_valid_uuid(&item.tunnel_id) {
                return Err(ValidationError::InvalidField {
                    field: "allocations.allocation_id/tunnel_id",
                    issue: "must be valid UUIDs",
                });
            }
            if !is_valid_rfc3339_utc(&item.interval_start) {
                return Err(ValidationError::InvalidField {
                    field: "allocations.interval_start",
                    issue: "must be a valid RFC 3339 UTC timestamp",
                });
            }
            if item.bytes_from_guests < 0
                || item.bytes_to_guests < 0
                || item.peak_flows < 0
                || item.datagrams_dropped < 0
            {
                return Err(ValidationError::InvalidField {
                    field: "allocations.counters",
                    issue: "usage and flow counters must be non-negative",
                });
            }
        }
        Ok(())
    }
}

/// Response body for `POST /internal/v1/relays/{relay_id}/report` (`T-048`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayReportResponse {
    /// Number of usage intervals recorded.
    pub recorded_intervals: usize,
    /// Number of reservations transitioned from `reserved` to `active`.
    pub activated_allocations: usize,
}

/// Encode a state-poll cursor for `now`.
pub fn encode_state_cursor(now: DateTime<Utc>) -> String {
    format!("cur_{}", now.timestamp_millis().max(0))
}

/// Parse an opaque state-poll cursor (`"cur_<epoch_ms>"`).
pub fn parse_state_cursor(cursor: &str) -> Result<DateTime<Utc>, &'static str> {
    let Some(ms_str) = cursor.strip_prefix("cur_") else {
        return Err("cursor must start with 'cur_'");
    };
    if ms_str.is_empty() || ms_str.len() > 20 || !ms_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err("cursor timestamp is malformed");
    }
    let ms: i64 = ms_str
        .parse()
        .map_err(|_| "cursor timestamp out of range")?;
    Utc.timestamp_millis_opt(ms)
        .single()
        .ok_or("cursor timestamp is not a valid UTC instant")
}

/// Pure helper that constructs a [`RelayStateResponse`] from in-memory rows (`REL-004`).
///
/// * When `since` is `None` (initial startup or post-restart recovery), includes all currently
///   live (`reserved` or `active`, unexpired) reservations and all currently live
///   (`revoked_at IS NULL`, unexpired) admissions (`full_snapshot: true`).
/// * When `since` is `Some(cursor_ts)`, includes reservations issued or released after
///   `cursor_ts` and admissions created or revoked after `cursor_ts` (`full_snapshot: false`).
pub fn build_relay_state_snapshot(
    relay_node: &RelayNode,
    key_ring: &SigningKeyRing,
    allocations_with_ports: &[(RelayAllocation, Vec<AllocationPort>)],
    admissions: &[GuestAdmission],
    since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<RelayStateResponse, ApiFailure> {
    let exported_keys = key_ring.export_verification_keys(now.timestamp());
    let verification_keys: Vec<RelayVerificationKeyDto> = exported_keys
        .keys()
        .iter()
        .map(|k| RelayVerificationKeyDto {
            kid: k.kid.as_str().to_owned(),
            valid_until_epoch_s: k.valid_until_epoch_s,
        })
        .collect();

    let mut reservation_deltas = Vec::new();
    for (alloc, ports) in allocations_with_ports {
        let is_live =
            matches!(alloc.state.as_str(), "reserved" | "active") && alloc.expires_at > now;
        let include = match since {
            None => is_live,
            Some(cursor_ts) => {
                alloc.issued_at > cursor_ts || alloc.released_at.is_some_and(|r| r > cursor_ts)
            }
        };
        if !include {
            continue;
        }

        let mut port_dtos = Vec::new();
        for p in ports {
            // For live allocations, include unreleased ports; for released allocations in a
            // delta poll, include the ports that belonged to that allocation so the relay knows
            // which listeners to close.
            if is_live && p.released_at.is_some() {
                continue;
            }
            let protocol = match p.protocol.as_str() {
                "tcp" => WireProtocol::Tcp,
                "udp" => WireProtocol::Udp,
                _ => {
                    return Err(ApiFailure::new(
                        ApiErrorCode::Internal,
                        "Stored allocation port protocol is invalid.",
                        "req_relay_state",
                        None,
                    ));
                }
            };
            let public_port = u16::try_from(p.public_port).map_err(|_| {
                ApiFailure::new(
                    ApiErrorCode::Internal,
                    "Stored allocation public port is out of bounds.",
                    "req_relay_state",
                    None,
                )
            })?;
            port_dtos.push(RelayPortSpecDto {
                mapping_id: p.mapping_id.to_string(),
                protocol,
                public_port,
            });
        }

        reservation_deltas.push(RelayReservationDelta {
            allocation_id: alloc.id.to_string(),
            tunnel_id: alloc.tunnel_id.to_string(),
            state: alloc.state.clone(),
            expires_at: alloc
                .expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ports: port_dtos,
        });
    }

    let mut admission_deltas = Vec::new();
    for adm in admissions {
        let is_active = adm.revoked_at.is_none() && adm.expires_at > now;
        let include = match since {
            None => is_active,
            Some(cursor_ts) => {
                adm.created_at > cursor_ts || adm.revoked_at.is_some_and(|r| r > cursor_ts)
            }
        };
        if !include {
            continue;
        }

        admission_deltas.push(RelayAdmissionDelta {
            admission_id: adm.id.to_string(),
            tunnel_id: adm.tunnel_id.to_string(),
            source_ip: adm.source_ip.clone(),
            active: is_active,
            expires_at: adm
                .expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });
    }

    Ok(RelayStateResponse {
        relay_id: relay_node.id.to_string(),
        full_snapshot: since.is_none(),
        next_cursor: encode_state_cursor(now),
        draining: RelayStatus::parse(&relay_node.status) == Some(RelayStatus::Draining),
        verification_keys,
        reservations: reservation_deltas,
        admissions: admission_deltas,
    })
}

/// Route 19 (`api.md`): `GET /internal/v1/relays/{relay_id}/state?cursor=<opaque>`.
///
/// Enforces relay self-scoping (`SEC-009`: a relay may only fetch its own state) and returns
/// either a full recovery snapshot (`cursor == None`, `REL-004`) or delta changes since
/// `cursor`.
pub async fn get_relay_state(
    pool: &PgPool,
    key_ring: &SigningKeyRing,
    actor: Actor,
    relay_id_str: &str,
    cursor: Option<&str>,
    request_id: &str,
) -> Result<ApiResponse<RelayStateResponse>, ApiFailure> {
    let relay_id =
        authorize_relay_operation(actor, Operation::FetchRelayState, relay_id_str, request_id)?;

    let since = match cursor {
        None => None,
        Some(raw_cur) => {
            let parsed = parse_state_cursor(raw_cur).map_err(|issue| {
                ApiFailure::new(
                    ApiErrorCode::ValidationFailed,
                    "State cursor is invalid.",
                    request_id,
                    Some(vec![ErrorDetail {
                        field: "cursor".to_owned(),
                        issue: issue.to_owned(),
                    }]),
                )
            })?;
            Some(parsed)
        }
    };

    let Some(relay_node) = get_relay_node(pool, relay_id).await.map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Database error fetching relay node.",
            request_id,
            None,
        )
    })?
    else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Relay not found.",
            request_id,
            None,
        ));
    };

    let allocations = list_allocations_for_relay(pool, relay_id, 500)
        .await
        .map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Database error fetching relay allocations.",
                request_id,
                None,
            )
        })?;

    let mut allocations_with_ports = Vec::with_capacity(allocations.len());
    let mut admissions = Vec::new();
    let mut seen_tunnels = Vec::new();

    for alloc in allocations {
        let ports = list_allocation_ports(pool, alloc.id).await.map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Database error fetching allocation ports.",
                request_id,
                None,
            )
        })?;
        if !seen_tunnels.contains(&alloc.tunnel_id) {
            seen_tunnels.push(alloc.tunnel_id);
            let tunnel_adms = list_all_admissions_for_tunnel(pool, alloc.tunnel_id)
                .await
                .map_err(|_| {
                    ApiFailure::new(
                        ApiErrorCode::Internal,
                        "Database error fetching tunnel guest admissions.",
                        request_id,
                        None,
                    )
                })?;
            admissions.extend(tunnel_adms);
        }
        allocations_with_ports.push((alloc, ports));
    }

    let now = Utc::now();
    let snapshot = build_relay_state_snapshot(
        &relay_node,
        key_ring,
        &allocations_with_ports,
        &admissions,
        since,
        now,
    )?;

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: snapshot,
    })
}

/// Parse and validate a raw JSON body for `POST /internal/v1/relays/{relay_id}/report`.
pub fn parse_relay_report_request(
    raw_json: &str,
    request_id: &str,
) -> Result<RelayReportRequest, ApiFailure> {
    if raw_json.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body exceeds maximum size.",
            request_id,
            None,
        ));
    }
    let raw_val: serde_json::Value = serde_json::from_str(raw_json).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body is not valid JSON.",
            request_id,
            None,
        )
    })?;
    if scan_json_for_local_target(&raw_val).is_err() {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Local destination fields are forbidden (INV-1).",
            request_id,
            None,
        ));
    }
    let parsed: RelayReportRequest = serde_json::from_value(raw_val).map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Request body does not match relay report schema.",
            request_id,
            None,
        )
    })?;
    parsed.validate().map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Relay report fields failed validation.",
            request_id,
            None,
        )
    })?;
    Ok(parsed)
}

/// Route 20 (`api.md`): `POST /internal/v1/relays/{relay_id}/report`.
///
/// Updates relay health (`last_health_at`), transitions `reserved` allocations to `active`
/// when `connected == true`, and upserts aggregated per-interval usage counters into
/// `usage_records` (`contracts.md` §7, `T-048`).
pub async fn report_relay_status_and_usage(
    pool: &PgPool,
    actor: Actor,
    relay_id_str: &str,
    raw_body: &str,
    request_id: &str,
) -> Result<ApiResponse<RelayReportResponse>, ApiFailure> {
    let relay_id =
        authorize_relay_operation(actor, Operation::ReportHealth, relay_id_str, request_id)?;
    let _ = authorize_relay_operation(actor, Operation::ReportUsage, relay_id_str, request_id)?;

    let report = parse_relay_report_request(raw_body, request_id)?;

    let Some(relay_node) = get_relay_node(pool, relay_id).await.map_err(|_| {
        ApiFailure::new(
            ApiErrorCode::Internal,
            "Database error fetching relay node.",
            request_id,
            None,
        )
    })?
    else {
        return Err(ApiFailure::new(
            ApiErrorCode::NotFound,
            "Relay not found.",
            request_id,
            None,
        ));
    };

    // Preserve `draining` or `retired` operator states while updating health; otherwise
    // transition between `healthy` and `unhealthy` based on the report.
    let current_status =
        RelayStatus::parse(&relay_node.status).unwrap_or(RelayStatus::Provisioning);
    let next_status = match current_status {
        RelayStatus::Draining | RelayStatus::Retired => current_status,
        RelayStatus::Provisioning | RelayStatus::Healthy | RelayStatus::Unhealthy => {
            if report.healthy {
                RelayStatus::Healthy
            } else {
                RelayStatus::Unhealthy
            }
        }
    };

    set_relay_status(pool, relay_id, next_status, report.healthy)
        .await
        .map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Database error updating relay status.",
                request_id,
                None,
            )
        })?;

    let mut activated_allocations = 0usize;
    let mut recorded_intervals = 0usize;

    for item in &report.allocations {
        let Ok(alloc_id) = Uuid::parse_str(&item.allocation_id) else {
            continue;
        };
        let Ok(tunnel_id) = Uuid::parse_str(&item.tunnel_id) else {
            continue;
        };
        let Ok(parsed_dt) = DateTime::parse_from_rfc3339(&item.interval_start) else {
            continue;
        };
        let interval_start = parsed_dt.with_timezone(&Utc);

        if item.connected {
            let activated = activate_allocation(pool, alloc_id, relay_id)
                .await
                .map_err(|_| {
                    ApiFailure::new(
                        ApiErrorCode::Internal,
                        "Database error activating relay allocation.",
                        request_id,
                        None,
                    )
                })?;
            if activated {
                activated_allocations = activated_allocations.saturating_add(1);
            }
        }

        add_usage(
            pool,
            &UsageDelta {
                tunnel_id,
                relay_id,
                interval_start,
                bytes_from_guests: item.bytes_from_guests,
                bytes_to_guests: item.bytes_to_guests,
                peak_flows: item.peak_flows,
                datagrams_dropped: item.datagrams_dropped,
            },
        )
        .await
        .map_err(|_| {
            ApiFailure::new(
                ApiErrorCode::Internal,
                "Database error recording relay usage.",
                request_id,
                None,
            )
        })?;
        recorded_intervals = recorded_intervals.saturating_add(1);
    }

    Ok(ApiResponse {
        status: 200,
        replayed: false,
        body: RelayReportResponse {
            recorded_intervals,
            activated_allocations,
        },
    })
}

fn authorize_relay_operation(
    actor: Actor,
    operation: Operation,
    relay_id_str: &str,
    request_id: &str,
) -> Result<Uuid, ApiFailure> {
    if matches!(actor, Actor::Anonymous | Actor::Guest { .. }) {
        return Err(ApiFailure::new(
            ApiErrorCode::Unauthenticated,
            "Relay authentication required.",
            request_id,
            None,
        ));
    }

    if !is_valid_uuid(relay_id_str) {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Relay id is not a valid UUID.",
            request_id,
            None,
        ));
    }
    let Ok(relay_id) = Uuid::parse_str(relay_id_str) else {
        return Err(ApiFailure::new(
            ApiErrorCode::ValidationFailed,
            "Relay id is not a valid UUID.",
            request_id,
            None,
        ));
    };

    // A User or Admin actor has no relay of their own (`policy.rs`: capability failure returns
    // `forbidden`); another Relay asking about `relay_id` fails visibility with `not_found`
    // (`SEC-009`).
    let target = match actor {
        Actor::Relay { .. } => Target::RelayOwned {
            owner: Some(&relay_id),
        },
        _ => Target::RelayOwned {
            owner: Some(&relay_id),
        },
    };

    if matches!(actor, Actor::User { .. } | Actor::Admin) {
        return Err(ApiFailure::new(
            ApiErrorCode::Forbidden,
            "Only the owning relay credential may call internal relay routes.",
            request_id,
            None,
        ));
    }

    if let Decision::Deny(code) = decide(actor, operation, target) {
        let api_code = match code {
            PolicyErrorCode::Unauthenticated => ApiErrorCode::Unauthenticated,
            PolicyErrorCode::Forbidden => ApiErrorCode::Forbidden,
            PolicyErrorCode::NotFound => ApiErrorCode::NotFound,
            PolicyErrorCode::ValidationFailed => ApiErrorCode::ValidationFailed,
            PolicyErrorCode::Conflict => ApiErrorCode::Conflict,
        };
        return Err(ApiFailure::new(
            api_code,
            "Relay authorization failed.",
            request_id,
            None,
        ));
    }

    Ok(relay_id)
}
