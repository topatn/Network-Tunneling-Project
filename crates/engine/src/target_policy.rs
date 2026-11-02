//! Local target validation policy and dial target resolution (`T-071`, `FR-003`, `SEC-006`,
//! `GAME-008`, `contracts.md` §§11, 13).
//!
//! ## Policy rules (`contracts.md` §13, `SEC-006`)
//!
//! 1. **Explicit per-mapping target:** Every configured mapping must specify an explicit IP address
//!    (or `"localhost"`) and non-zero port (`1..=65535`), and its `mapping_key` must exist in the
//!    loaded [`gtt_profiles::Profile`].
//! 2. **Loopback allowed by default:** `127.0.0.0/8`, `::1`, and `"localhost"` are allowed without
//!    additional confirmation.
//! 3. **Non-loopback requires explicit per-mapping confirmation:** Any non-loopback unicast IP
//!    requires `confirm_non_loopback == true` on that specific mapping entry.
//! 4. **Forbidden address categories:** Unspecified (`0.0.0.0`, `::`), multicast (`224.0.0.0/4`,
//!    `ff00::/8`), broadcast (`255.255.255.255`), and link-local (`169.254.0.0/16`, `fe80::/10`)
//!    addresses — including IPv4-mapped IPv6 equivalents (`::ffff:x.x.x.x`) — are unconditionally
//!    rejected.
//! 5. **Sensitive-port deny-list:** Commonly sensitive local service ports ([`SENSITIVE_LOCAL_PORTS`],
//!    such as SSH `22`, SMB `445`, RDP `3389`, PostgreSQL `5432`, Redis `6379`, Docker `2375`) are
//!    rejected unless the loaded game profile explicitly declares that port as `default_port` for
//!    that mapping.
//! 6. **No relay influence over local targets (`SEC-006`, `INV-1`):** `hello_ack` from the relay
//!    supplies `(index, mapping_key, protocol, public_port)` only. Binding relay wire indexes never
//!    alters `local_addr` or enables a disabled mapping.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use gtt_profiles::{Profile, Protocol as ProfileProtocol};
use gtt_protocol::{is_valid_identifier, HelloAckMapping, Protocol, TcpOpenStatus};

use crate::tcp::{LocalMappingConfig, DEFAULT_ENGINE_TCP_IO_TIMEOUT, DEFAULT_LOCAL_DIAL_TIMEOUT};

/// Deny-list of commonly sensitive local service ports (`contracts.md` §13, `SEC-006`).
///
/// Rejected unless the loaded [`Profile`] explicitly declares the port as `default_port` on that
/// mapping.
pub const SENSITIVE_LOCAL_PORTS: &[u16] = &[
    22,    // SSH
    23,    // Telnet
    25,    // SMTP
    53,    // DNS
    80,    // HTTP
    88,    // Kerberos
    110,   // POP3
    135,   // MSRPC
    137,   // NetBIOS Name Service
    138,   // NetBIOS Datagram Service
    139,   // NetBIOS Session Service
    143,   // IMAP
    389,   // LDAP
    443,   // HTTPS
    445,   // SMB
    636,   // LDAPS
    1433,  // MSSQL
    2049,  // NFS
    2375,  // Docker daemon (unencrypted)
    3306,  // MySQL
    3389,  // RDP
    5432,  // PostgreSQL
    5900,  // VNC
    5985,  // WinRM HTTP
    5986,  // WinRM HTTPS
    6379,  // Redis
    6443,  // Kubernetes API
    27017, // MongoDB
];

/// Host-supplied local target configuration input for a single mapping (`contracts.md` §13
/// `configure_mappings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingTargetInput {
    /// Profile mapping key (`^[a-z][a-z0-9-]{0,31}$`).
    pub mapping_key: String,
    /// Local host string (`"127.0.0.1"`, `"::1"`, `"localhost"`, or a LAN IP).
    pub local_host: String,
    /// Local port (`1..=65535`).
    pub local_port: u16,
    /// Whether this mapping is enabled by the host (`GAME-008`).
    pub enabled: bool,
    /// Explicit host confirmation when `local_host` is a non-loopback unicast address
    /// (`contracts.md` §13).
    pub confirm_non_loopback: bool,
}

/// Specific reason a [`MappingTargetInput`] failed [`validate_mapping_target`] (`T-071`, `FR-003`,
/// `SEC-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetPolicyViolation {
    /// `mapping_key` does not match `^[a-z][a-z0-9-]{0,31}$`.
    InvalidMappingKeyFormat,
    /// `mapping_key` is not declared in the selected game profile.
    UnknownMappingKey,
    /// `mapping_key` was provided more than once in the same configuration set.
    DuplicateMappingKey,
    /// `local_host` could not be parsed as an IP literal or `"localhost"`.
    InvalidHostAddress,
    /// `local_port` was `0`.
    InvalidPortZero,
    /// `local_host` is unspecified (`0.0.0.0`, `::`), multicast, broadcast, or link-local
    /// (`169.254.0.0/16`, `fe80::/10`).
    ForbiddenAddressCategory {
        /// Human-readable category name (`"unspecified"`, `"multicast"`, `"broadcast"`, `"link_local"`).
        category: &'static str,
    },
    /// `local_host` is a non-loopback unicast address without `confirm_non_loopback == true`.
    NonLoopbackRequiresConfirmation,
    /// `local_port` is on [`SENSITIVE_LOCAL_PORTS`] and not explicitly allowed by the profile.
    SensitivePortDenied {
        /// The denied port number.
        port: u16,
    },
    /// The validated table was rejected when building the engine session (for example, a
    /// duplicate wire index). Not tied to a single mapping, so `mapping_key` is empty.
    SessionSetupRejected {
        /// Static reason from the session constructor (no user data, `PRIV-002`).
        reason: &'static str,
    },
}

impl fmt::Display for TargetPolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMappingKeyFormat => {
                f.write_str("mapping_key must match ^[a-z][a-z0-9-]{0,31}$")
            }
            Self::UnknownMappingKey => f.write_str("mapping_key does not exist in game profile"),
            Self::DuplicateMappingKey => f.write_str("duplicate mapping_key in configuration"),
            Self::InvalidHostAddress => {
                f.write_str("local_host must be a valid IP address or 'localhost'")
            }
            Self::InvalidPortZero => f.write_str("local_port must be between 1 and 65535"),
            Self::ForbiddenAddressCategory { category } => {
                write!(f, "{category} addresses are not permitted as local targets")
            }
            Self::NonLoopbackRequiresConfirmation => f.write_str(
                "non-loopback local target requires explicit user confirmation per mapping",
            ),
            Self::SensitivePortDenied { port } => {
                write!(
                    f,
                    "port {port} is on the sensitive local service deny-list and is not allowed by this profile"
                )
            }
            Self::SessionSetupRejected { reason } => {
                write!(
                    f,
                    "engine session setup rejected the configuration: {reason}"
                )
            }
        }
    }
}

/// Per-mapping validation error returned by [`configure_mappings`] (`contracts.md` §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingValidationFailure {
    /// Mapping key that failed validation.
    pub mapping_key: String,
    /// Policy violation reason.
    pub violation: TargetPolicyViolation,
}

impl fmt::Display for MappingValidationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.mapping_key, self.violation)
    }
}

/// Validated, immutable local target entry for a single profile mapping (`T-071`, `SEC-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMappingTarget {
    /// Profile mapping key.
    mapping_key: String,
    /// Wire protocol from the profile (`tcp` or `udp`).
    protocol: Protocol,
    /// Validated local socket address to dial.
    local_addr: SocketAddr,
    /// Whether this mapping is enabled (`GAME-008`).
    enabled: bool,
    /// Optional wire index assigned when `hello_ack` is received.
    wire_index: Option<u16>,
}

impl ValidatedMappingTarget {
    /// Profile mapping key.
    pub fn mapping_key(&self) -> &str {
        &self.mapping_key
    }

    /// Transport protocol (`tcp` or `udp`).
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Validated local target [`SocketAddr`].
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Whether the mapping is enabled (`GAME-008`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Assigned wire index from `hello_ack`, if bound.
    pub fn wire_index(&self) -> Option<u16> {
        self.wire_index
    }
}

/// Reason a relay-requested dial by `mapping_index` was refused (`contracts.md` §11, `SEC-006`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialRefusalReason {
    /// `mapping_index` does not exist in the engine's local configuration.
    UnknownMappingIndex {
        /// Requested wire index.
        mapping_index: u16,
    },
    /// Mapping exists in the engine's configuration but is disabled (`GAME-008`).
    MappingDisabled {
        /// Requested wire index.
        mapping_index: u16,
    },
    /// Mapping exists and is enabled, but uses a different transport protocol.
    ProtocolMismatch {
        /// Requested wire index.
        mapping_index: u16,
    },
}

impl DialRefusalReason {
    /// Wire status byte to send on a refused `tcp_open` stream (`0x01`, `contracts.md` §11).
    pub fn to_tcp_open_status(self) -> TcpOpenStatus {
        TcpOpenStatus::Refused
    }
}

/// Validated table of local targets owned exclusively by the host engine (`T-071`, `SEC-006`).
///
/// Local socket addresses can only be set through [`configure_mappings`]; no method accepts or
/// modifies local addresses from network frames.
#[derive(Debug, Clone)]
pub struct ValidatedLocalTargetTable {
    profile_id: String,
    profile_version: i32,
    by_key: HashMap<String, ValidatedMappingTarget>,
    key_order: Vec<String>,
    by_wire_index: HashMap<u16, String>,
}

impl ValidatedLocalTargetTable {
    /// Profile ID this target table was validated against.
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Profile version this target table was validated against.
    pub fn profile_version(&self) -> i32 {
        self.profile_version
    }

    /// Look up a validated target by `mapping_key`.
    pub fn get_by_key(&self, mapping_key: &str) -> Option<&ValidatedMappingTarget> {
        self.by_key.get(mapping_key)
    }

    /// Iterate over all validated targets in declaration order.
    pub fn targets(&self) -> Vec<&ValidatedMappingTarget> {
        self.key_order
            .iter()
            .filter_map(|k| self.by_key.get(k))
            .collect()
    }

    /// Bind wire `index` values from `hello_ack.mappings` (`contracts.md` §§10–11, `SEC-006`).
    ///
    /// **Security invariant (`SEC-006`):** This method only links `index -> mapping_key` for keys
    /// that already exist, are enabled in the host's local configuration, and match the configured
    /// `Protocol`. It **never** modifies `local_addr` or enables a disabled mapping.
    pub fn bind_hello_ack_mappings(&mut self, ack_mappings: &[HelloAckMapping]) {
        self.by_wire_index.clear();
        for target in self.by_key.values_mut() {
            target.wire_index = None;
        }

        for ack in ack_mappings {
            if let Some(target) = self.by_key.get_mut(&ack.mapping_key) {
                if target.enabled
                    && target.protocol == ack.protocol
                    && !self.by_wire_index.contains_key(&ack.index)
                {
                    target.wire_index = Some(ack.index);
                    self.by_wire_index
                        .insert(ack.index, ack.mapping_key.clone());
                }
            }
        }
    }

    /// Resolve a relay-requested `mapping_index` and `expected_protocol` to the host's configured
    /// local [`SocketAddr`] (`contracts.md` §11, `SEC-006`).
    pub fn resolve_dial_target(
        &self,
        mapping_index: u16,
        expected_protocol: Protocol,
    ) -> Result<SocketAddr, DialRefusalReason> {
        let Some(key) = self.by_wire_index.get(&mapping_index) else {
            return Err(DialRefusalReason::UnknownMappingIndex { mapping_index });
        };
        let Some(target) = self.by_key.get(key) else {
            return Err(DialRefusalReason::UnknownMappingIndex { mapping_index });
        };
        if !target.enabled {
            return Err(DialRefusalReason::MappingDisabled { mapping_index });
        }
        if target.protocol != expected_protocol {
            return Err(DialRefusalReason::ProtocolMismatch { mapping_index });
        }
        Ok(target.local_addr)
    }

    /// Export [`LocalMappingConfig`] entries for [`crate::session::EngineTunnelSession`].
    ///
    /// If `wire_index` has not yet been bound via `hello_ack`, declaration order (`0, 1, ...`) is
    /// used as the initial index.
    pub fn to_local_mapping_configs(&self) -> Vec<LocalMappingConfig> {
        let mut out = Vec::with_capacity(self.key_order.len());
        for (pos, key) in self.key_order.iter().enumerate() {
            if let Some(target) = self.by_key.get(key) {
                let index = target
                    .wire_index
                    .unwrap_or_else(|| u16::try_from(pos).unwrap_or(0));
                out.push(LocalMappingConfig {
                    index,
                    mapping_key: target.mapping_key.clone(),
                    protocol: target.protocol,
                    local_addr: target.local_addr,
                    enabled: target.enabled,
                    connect_timeout: DEFAULT_LOCAL_DIAL_TIMEOUT,
                    io_timeout: DEFAULT_ENGINE_TCP_IO_TIMEOUT,
                });
            }
        }
        out
    }
}

/// Return true if `port` is on [`SENSITIVE_LOCAL_PORTS`].
pub fn is_sensitive_local_port(port: u16) -> bool {
    SENSITIVE_LOCAL_PORTS.contains(&port)
}

/// Validate a single [`MappingTargetInput`] against `profile` and the local target policy
/// (`contracts.md` §13, `FR-003`, `SEC-006`).
pub fn validate_mapping_target(
    profile: &Profile,
    input: &MappingTargetInput,
) -> Result<ValidatedMappingTarget, TargetPolicyViolation> {
    if !is_valid_identifier(&input.mapping_key) {
        return Err(TargetPolicyViolation::InvalidMappingKeyFormat);
    }

    let Some(profile_mapping) = profile.mapping(&input.mapping_key) else {
        return Err(TargetPolicyViolation::UnknownMappingKey);
    };

    if input.local_port == 0 {
        return Err(TargetPolicyViolation::InvalidPortZero);
    }

    let ip = parse_local_host_ip(&input.local_host)?;
    validate_ip_policy(ip, input.confirm_non_loopback)?;

    // Sensitive-port deny-list check: rejected unless the profile explicitly declares that port.
    if is_sensitive_local_port(input.local_port) && profile_mapping.default_port != input.local_port
    {
        return Err(TargetPolicyViolation::SensitivePortDenied {
            port: input.local_port,
        });
    }

    let protocol = match profile_mapping.protocol {
        ProfileProtocol::Tcp => Protocol::Tcp,
        ProfileProtocol::Udp => Protocol::Udp,
    };

    Ok(ValidatedMappingTarget {
        mapping_key: input.mapping_key.clone(),
        protocol,
        local_addr: SocketAddr::new(ip, input.local_port),
        enabled: input.enabled,
        wire_index: None,
    })
}

/// Validate a full list of [`MappingTargetInput`] entries against `profile` and construct a
/// [`ValidatedLocalTargetTable`] (`contracts.md` §13 `configure_mappings`, `T-071`).
///
/// Returns `Err(failures)` with every validation failure if any entry violates the policy.
pub fn configure_mappings(
    profile: &Profile,
    inputs: &[MappingTargetInput],
) -> Result<ValidatedLocalTargetTable, Vec<MappingValidationFailure>> {
    let mut failures = Vec::new();
    let mut seen_keys = HashSet::new();
    let mut by_key = HashMap::new();
    let mut key_order = Vec::new();

    for input in inputs {
        if !seen_keys.insert(input.mapping_key.clone()) {
            failures.push(MappingValidationFailure {
                mapping_key: input.mapping_key.clone(),
                violation: TargetPolicyViolation::DuplicateMappingKey,
            });
            continue;
        }

        match validate_mapping_target(profile, input) {
            Ok(mut validated) => {
                let idx = u16::try_from(key_order.len()).unwrap_or(0);
                validated.wire_index = Some(idx);
                key_order.push(input.mapping_key.clone());
                by_key.insert(input.mapping_key.clone(), validated);
            }
            Err(violation) => {
                failures.push(MappingValidationFailure {
                    mapping_key: input.mapping_key.clone(),
                    violation,
                });
            }
        }
    }

    if !failures.is_empty() {
        return Err(failures);
    }

    let mut by_wire_index = HashMap::new();
    for (idx, key) in key_order.iter().enumerate() {
        if let Some(target) = by_key.get(key) {
            if target.enabled {
                if let Ok(wire_idx) = u16::try_from(idx) {
                    by_wire_index.insert(wire_idx, key.clone());
                }
            }
        }
    }

    Ok(ValidatedLocalTargetTable {
        profile_id: profile.id.clone(),
        profile_version: profile.version,
        by_key,
        key_order,
        by_wire_index,
    })
}

fn parse_local_host_ip(host: &str) -> Result<IpAddr, TargetPolicyViolation> {
    let trimmed = host.trim();
    if trimmed.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    trimmed
        .parse::<IpAddr>()
        .map_err(|_| TargetPolicyViolation::InvalidHostAddress)
}

fn validate_ip_policy(ip: IpAddr, confirm_non_loopback: bool) -> Result<(), TargetPolicyViolation> {
    // Unwrap IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) so they cannot bypass IPv4 checks.
    let canonical_ip = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        IpAddr::V4(v4) => IpAddr::V4(v4),
    };

    if canonical_ip.is_unspecified() {
        return Err(TargetPolicyViolation::ForbiddenAddressCategory {
            category: "unspecified",
        });
    }
    if canonical_ip.is_multicast() {
        return Err(TargetPolicyViolation::ForbiddenAddressCategory {
            category: "multicast",
        });
    }

    match canonical_ip {
        IpAddr::V4(v4) => {
            if v4.is_broadcast() {
                return Err(TargetPolicyViolation::ForbiddenAddressCategory {
                    category: "broadcast",
                });
            }
            if v4.is_link_local() {
                return Err(TargetPolicyViolation::ForbiddenAddressCategory {
                    category: "link_local",
                });
            }
            if v4.is_loopback() {
                return Ok(());
            }
        }
        IpAddr::V6(v6) => {
            if is_ipv6_unicast_link_local(v6) {
                return Err(TargetPolicyViolation::ForbiddenAddressCategory {
                    category: "link_local",
                });
            }
            if v6.is_loopback() {
                return Ok(());
            }
        }
    }

    if !confirm_non_loopback {
        return Err(TargetPolicyViolation::NonLoopbackRequiresConfirmation);
    }

    Ok(())
}

fn is_ipv6_unicast_link_local(v6: Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}
