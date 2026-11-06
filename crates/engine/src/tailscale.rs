//! Tailscale Tailnet Node Share integration for Free-tier self-hosted game servers
//! (`ADR-022`, `context.md` §§1, 7, `contracts.md` §§12, 13, `FR-005`, `GAME-002`, `GAME-008`,
//! `SEC-006`, `SRV-005`, `PRIV-002`).
//!
//! ## Architecture (`ADR-022`)
//!
//! 1. **Two-tier connectivity model:**
//!    * **Free tier (Tailscale Node Share):** The host runs the game server locally on their PC
//!      (bound strictly to `127.0.0.1`, `SRV-005`) and shares their Tailscale node with friends via
//!      Tailscale's Tailnet Node Sharing. `gtt-engine` discovers the local Tailscale daemon state
//!      via `tailscale status --json`, binds a TCP forwarder on the host's Tailscale CGNAT address
//!      (`100.64.0.0/10`, RFC 6598) for enabled profile mappings only (`tcp:25565` for Minecraft:
//!      Java Edition; RCON and Query remain off by default per `GAME-002` and `GAME-008`), and
//!      forwards traffic to `127.0.0.1:25565`. No `gtt-control-plane` allocation or operator relay
//!      bandwidth is consumed.
//!    * **Paid tier (GTT Managed Relay):** The host tunnels through GTT relays (`ADR-007` G2) so
//!      guests can join via a public `relay.host:public_port` address with zero client setup.
//! 2. **Multi-layer status continuity (`FR-010`, `UX-002`, `SRV-006`, `SRV-011`):**
//!    Because `gtt-engine` mediates connections between the Tailscale interface (`100.x.y.z:25565`)
//!    and the loopback Minecraft server (`127.0.0.1:25565`), all five status layers (`tunnel_state`,
//!    `local_probes`, `players_seen`, `managed_server`, and `confirmed_presence`) work identically
//!    in both Tailscale Node Share mode and GTT Relay mode.
//! 3. **Privacy (`PRIV-002`, `INV-5`):**
//!    Peer Tailscale IP addresses are tracked only in a bounded in-memory set (`MAX_TAILSCALE_PEERS_TRACKED`)
//!    to compute `distinct_sources` and are never written to logs, disk, or error strings.

use std::collections::HashSet;
use std::fmt;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gtt_protocol::Protocol;
use serde::Deserialize;

use crate::lifecycle::{PlayersSeenSnapshot, PublicMappingEndpoint, OBSERVED_TRAFFIC_LABEL};
use crate::target_policy::ValidatedLocalTargetTable;
use crate::tcp::{
    bridge_engine_tcp_streams, DEFAULT_ENGINE_TCP_IO_TIMEOUT, DEFAULT_LOCAL_DIAL_TIMEOUT,
};

/// Maximum accepted byte length of `tailscale status --json` output (`256 KiB`, `AGENTS.md` §4).
pub const MAX_TAILSCALE_STATUS_JSON_BYTES: usize = 256 * 1024;

/// Maximum distinct peer source IPs tracked in memory per Tailscale session (`AGENTS.md` §4).
pub const MAX_TAILSCALE_PEERS_TRACKED: usize = 256;

/// Default timeout when invoking the local `tailscale` CLI (`5 seconds`, `AGENTS.md` §4).
pub const DEFAULT_TAILSCALE_CLI_TIMEOUT: Duration = Duration::from_secs(5);

/// Tailscale Admin Console URL where the host generates a Node Share invite link (`ADR-022`).
pub const TAILSCALE_ADMIN_MACHINES_URL: &str = "https://login.tailscale.com/admin/machines";

/// Mandatory security disclosure surfaced when using OS-level Tailscale Node Sharing (`GAME-008`,
/// `SEC-006`, `ADR-022`).
pub const TAILSCALE_NODE_SHARE_SECURITY_NOTE: &str = "GTT keeps the Minecraft server bound to 127.0.0.1 and exposes only enabled profile mappings on your Tailscale IP. Because Tailscale node sharing occurs at the OS network interface level, restrict shared-node access to TCP port 25565 in your Tailnet ACLs/Grants if other services on this PC listen on 0.0.0.0.";

/// Returns `true` if `ip` is within the Tailscale CGNAT IPv4 range (`100.64.0.0/10`, RFC 6598).
pub const fn is_tailscale_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
}

/// Returns `true` if `ip` is within the Tailscale ULA IPv6 prefix (`fd7a:115c:a1e0::/48`).
pub const fn is_tailscale_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

/// Reported `BackendState` from `tailscale status --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailscaleBackendState {
    /// Tailscale daemon is authenticated and active (`"Running"`).
    Running,
    /// Tailscale requires interactive login (`"NeedsLogin"`).
    NeedsLogin,
    /// Tailscale requires machine authorization (`"NeedsMachineAuth"`).
    NeedsMachineAuth,
    /// Tailscale daemon is stopped (`"Stopped"`).
    Stopped,
    /// Tailscale daemon is starting (`"Starting"`).
    Starting,
    /// Tailscale daemon has not initialized state (`"NoState"`).
    NoState,
    /// Unrecognized backend state string.
    Other(String),
}

impl TailscaleBackendState {
    /// Parse a `BackendState` string from `tailscale status --json`.
    pub fn from_wire(raw: &str) -> Self {
        match raw.trim() {
            "Running" => Self::Running,
            "NeedsLogin" => Self::NeedsLogin,
            "NeedsMachineAuth" => Self::NeedsMachineAuth,
            "Stopped" => Self::Stopped,
            "Starting" => Self::Starting,
            "NoState" => Self::NoState,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Machine-readable wire representation.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "Running",
            Self::NeedsLogin => "NeedsLogin",
            Self::NeedsMachineAuth => "NeedsMachineAuth",
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::NoState => "NoState",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Parsed local node status from `tailscale status --json` (`ADR-022`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleNodeStatus {
    /// Current Tailscale daemon backend state.
    pub backend_state: TailscaleBackendState,
    /// Whether `Self.Online` is `true`.
    pub online: bool,
    /// Validated `100.64.0.0/10` Tailscale IPv4 address for this machine.
    pub tailscale_ipv4: Option<Ipv4Addr>,
    /// Validated `fd7a:115c:a1e0::/48` Tailscale IPv6 address for this machine, if present.
    pub tailscale_ipv6: Option<Ipv6Addr>,
    /// Normalized MagicDNS hostname (without trailing dot), e.g. `"host-pc.tailnet.ts.net"`.
    pub dns_name: Option<String>,
    /// Machine hostname reported by Tailscale (`Self.HostName`).
    pub host_name: String,
    /// Tailscale Admin Console URL for sharing this node with friends.
    pub share_admin_url: &'static str,
    /// Security disclosure for OS-level Tailscale node sharing (`GAME-008`, `SEC-006`).
    pub security_note: &'static str,
}

impl TailscaleNodeStatus {
    /// Verify that the local Tailscale node is running, online, and has a valid `100.64.0.0/10`
    /// IPv4 address suitable for hosting a Node Share game server (`ADR-022`).
    pub fn require_ready_for_node_share(&self) -> Result<Ipv4Addr, TailscaleError> {
        match &self.backend_state {
            TailscaleBackendState::Running => {}
            TailscaleBackendState::NeedsLogin | TailscaleBackendState::NeedsMachineAuth => {
                return Err(TailscaleError::NotAuthenticated {
                    state: self.backend_state.as_str().to_owned(),
                });
            }
            TailscaleBackendState::Stopped => {
                return Err(TailscaleError::DaemonStopped);
            }
            other => {
                return Err(TailscaleError::BackendNotReady {
                    state: other.as_str().to_owned(),
                });
            }
        }

        if !self.online {
            return Err(TailscaleError::NodeOffline);
        }

        self.tailscale_ipv4
            .ok_or(TailscaleError::MissingTailscaleIpv4)
    }
}

/// Errors produced when detecting Tailscale or starting a Tailscale Node Share session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailscaleError {
    /// Tailscale CLI executable was not found on `PATH` or standard install locations.
    CliNotFound,
    /// Executing `tailscale status --json` failed or timed out.
    CliExecutionFailed {
        /// Diagnostic failure description.
        reason: String,
    },
    /// `tailscale status --json` output exceeded [`MAX_TAILSCALE_STATUS_JSON_BYTES`].
    StatusOutputTooLarge {
        /// Observed byte count.
        bytes: usize,
    },
    /// `tailscale status --json` output was not valid JSON.
    InvalidStatusJson {
        /// Parse failure detail.
        reason: String,
    },
    /// Tailscale is installed but the user is not logged in (`NeedsLogin` / `NeedsMachineAuth`).
    NotAuthenticated {
        /// Reported backend state.
        state: String,
    },
    /// Tailscale daemon is stopped (`Stopped`).
    DaemonStopped,
    /// Tailscale backend is in a non-running state (`Starting`, `NoState`, etc.).
    BackendNotReady {
        /// Reported backend state.
        state: String,
    },
    /// Tailscale node reports `Self.Online == false`.
    NodeOffline,
    /// No valid `100.64.0.0/10` Tailscale IPv4 address was present in `Self.TailscaleIPs`.
    MissingTailscaleIpv4,
    /// No TCP mappings are enabled in the local target policy table.
    NoEnabledTcpMappings,
    /// Binding the TCP forwarder listener on the Tailscale interface failed.
    ListenerBindFailed {
        /// Mapping key that failed to bind.
        mapping_key: String,
        /// Target bind socket address.
        bind_addr: SocketAddr,
        /// OS error string.
        reason: String,
    },
    /// Engine session state machine rejected the transition.
    InvalidSessionState {
        /// Diagnostic transition error.
        reason: String,
    },
}

impl TailscaleError {
    /// Machine-readable lowercase error code (`contracts.md` §§3, 13, `AGENTS.md` §4).
    pub fn code(&self) -> &'static str {
        match self {
            Self::CliNotFound => "tailscale_cli_not_found",
            Self::CliExecutionFailed { .. } => "tailscale_cli_failed",
            Self::StatusOutputTooLarge { .. } => "tailscale_status_too_large",
            Self::InvalidStatusJson { .. } => "tailscale_status_invalid_json",
            Self::NotAuthenticated { .. } => "tailscale_not_authenticated",
            Self::DaemonStopped => "tailscale_daemon_stopped",
            Self::BackendNotReady { .. } => "tailscale_backend_not_ready",
            Self::NodeOffline => "tailscale_node_offline",
            Self::MissingTailscaleIpv4 => "tailscale_missing_ipv4",
            Self::NoEnabledTcpMappings => "tailscale_no_enabled_tcp_mappings",
            Self::ListenerBindFailed { .. } => "tailscale_listener_bind_failed",
            Self::InvalidSessionState { .. } => "invalid_state_transition",
        }
    }
}

impl fmt::Display for TailscaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliNotFound => write!(
                f,
                "Tailscale CLI ('tailscale.exe') was not found; install Tailscale to use Free-tier Node Share"
            ),
            Self::CliExecutionFailed { reason } => {
                write!(f, "failed to run 'tailscale status --json': {reason}")
            }
            Self::StatusOutputTooLarge { bytes } => write!(
                f,
                "'tailscale status --json' output ({bytes} bytes) exceeds maximum ({MAX_TAILSCALE_STATUS_JSON_BYTES} bytes)"
            ),
            Self::InvalidStatusJson { reason } => {
                write!(f, "invalid 'tailscale status --json' payload: {reason}")
            }
            Self::NotAuthenticated { state } => write!(
                f,
                "Tailscale requires authentication (BackendState={state}); sign in to your Tailnet first"
            ),
            Self::DaemonStopped => {
                write!(f, "Tailscale daemon is stopped; start Tailscale and try again")
            }
            Self::BackendNotReady { state } => {
                write!(f, "Tailscale backend is not ready (BackendState={state})")
            }
            Self::NodeOffline => {
                write!(f, "Tailscale node is offline; check your internet connection and Tailscale status")
            }
            Self::MissingTailscaleIpv4 => write!(
                f,
                "Tailscale node does not have a valid 100.64.0.0/10 IPv4 address assigned"
            ),
            Self::NoEnabledTcpMappings => {
                write!(f, "no enabled TCP mappings found to expose over Tailscale Node Share")
            }
            Self::ListenerBindFailed {
                mapping_key,
                bind_addr,
                reason,
            } => write!(
                f,
                "failed to bind Tailscale listener for mapping '{mapping_key}' on {bind_addr}: {reason}"
            ),
            Self::InvalidSessionState { reason } => {
                write!(f, "cannot start Tailscale Node Share from current session state: {reason}")
            }
        }
    }
}

impl std::error::Error for TailscaleError {}

#[derive(Deserialize)]
struct RawTailscaleStatus {
    #[serde(rename = "BackendState", default)]
    backend_state: String,
    #[serde(rename = "Self", default)]
    self_node: Option<RawTailscaleSelfNode>,
}

#[derive(Deserialize)]
struct RawTailscaleSelfNode {
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Vec<String>,
}

/// Parse `tailscale status --json` output into a validated [`TailscaleNodeStatus`] (`ADR-022`).
///
/// Enforces [`MAX_TAILSCALE_STATUS_JSON_BYTES`] before deserializing (`AGENTS.md` §4) and accepts
/// only RFC 6598 (`100.64.0.0/10`) IPv4 addresses and `fd7a:115c:a1e0::/48` IPv6 addresses.
pub fn parse_tailscale_status_json(raw_json: &str) -> Result<TailscaleNodeStatus, TailscaleError> {
    if raw_json.len() > MAX_TAILSCALE_STATUS_JSON_BYTES {
        return Err(TailscaleError::StatusOutputTooLarge {
            bytes: raw_json.len(),
        });
    }

    let parsed: RawTailscaleStatus =
        serde_json::from_str(raw_json).map_err(|e| TailscaleError::InvalidStatusJson {
            reason: e.to_string(),
        })?;

    let backend_state = TailscaleBackendState::from_wire(&parsed.backend_state);
    let Some(self_node) = parsed.self_node else {
        return Ok(TailscaleNodeStatus {
            backend_state,
            online: false,
            tailscale_ipv4: None,
            tailscale_ipv6: None,
            dns_name: None,
            host_name: String::new(),
            share_admin_url: TAILSCALE_ADMIN_MACHINES_URL,
            security_note: TAILSCALE_NODE_SHARE_SECURITY_NOTE,
        });
    };

    let mut tailscale_ipv4 = None;
    let mut tailscale_ipv6 = None;
    for ip_str in &self_node.tailscale_ips {
        if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
            match ip {
                IpAddr::V4(v4) if tailscale_ipv4.is_none() && is_tailscale_ipv4(v4) => {
                    tailscale_ipv4 = Some(v4);
                }
                IpAddr::V6(v6) if tailscale_ipv6.is_none() && is_tailscale_ipv6(v6) => {
                    tailscale_ipv6 = Some(v6);
                }
                _ => {}
            }
        }
    }

    let trimmed_dns = self_node.dns_name.trim().trim_end_matches('.');
    let dns_name = if trimmed_dns.is_empty() {
        None
    } else {
        Some(trimmed_dns.to_owned())
    };

    Ok(TailscaleNodeStatus {
        backend_state,
        online: self_node.online,
        tailscale_ipv4,
        tailscale_ipv6,
        dns_name,
        host_name: self_node.host_name.trim().to_owned(),
        share_admin_url: TAILSCALE_ADMIN_MACHINES_URL,
        security_note: TAILSCALE_NODE_SHARE_SECURITY_NOTE,
    })
}

/// Abstraction for querying local Tailscale status (`tailscale status --json`).
pub trait TailscaleDetector {
    /// Query the current local Tailscale node status.
    fn query_status(&self) -> Result<TailscaleNodeStatus, TailscaleError>;
}

/// Deterministic in-memory [`TailscaleDetector`] for unit and integration tests.
#[derive(Debug, Clone)]
pub struct StaticTailscaleDetector {
    result: Result<TailscaleNodeStatus, TailscaleError>,
}

impl StaticTailscaleDetector {
    /// Construct a detector from a raw `tailscale status --json` payload.
    pub fn from_json(raw_json: &str) -> Self {
        Self {
            result: parse_tailscale_status_json(raw_json),
        }
    }

    /// Construct a detector returning a pre-built [`TailscaleNodeStatus`].
    pub fn from_status(status: TailscaleNodeStatus) -> Self {
        Self { result: Ok(status) }
    }

    /// Construct a detector returning a specific [`TailscaleError`].
    pub fn from_error(err: TailscaleError) -> Self {
        Self { result: Err(err) }
    }
}

impl TailscaleDetector for StaticTailscaleDetector {
    fn query_status(&self) -> Result<TailscaleNodeStatus, TailscaleError> {
        self.result.clone()
    }
}

/// Production [`TailscaleDetector`] invoking `tailscale status --json` on the host PC.
#[derive(Debug, Clone)]
pub struct CliTailscaleDetector {
    cli_path: Option<String>,
}

impl Default for CliTailscaleDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CliTailscaleDetector {
    /// Construct a [`CliTailscaleDetector`] that locates `tailscale` on `PATH` or the standard
    /// Windows installation directory (`C:\Program Files\Tailscale\tailscale.exe`).
    pub fn new() -> Self {
        Self { cli_path: None }
    }

    /// Construct a [`CliTailscaleDetector`] using an explicit binary path.
    pub fn with_binary_path(path: impl Into<String>) -> Self {
        Self {
            cli_path: Some(path.into()),
        }
    }

    fn resolve_candidates(&self) -> Vec<String> {
        if let Some(custom) = &self.cli_path {
            return vec![custom.clone()];
        }
        let mut candidates = vec!["tailscale".to_owned(), "tailscale.exe".to_owned()];
        let win_default = r"C:\Program Files\Tailscale\tailscale.exe";
        if Path::new(win_default).is_file() {
            candidates.insert(0, win_default.to_owned());
        }
        candidates
    }
}

impl TailscaleDetector for CliTailscaleDetector {
    fn query_status(&self) -> Result<TailscaleNodeStatus, TailscaleError> {
        let mut last_not_found = true;
        let mut last_err_reason = String::new();

        for bin in self.resolve_candidates() {
            let mut cmd = Command::new(&bin);
            cmd.args(["status", "--json"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(err) => {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        last_not_found = false;
                        last_err_reason = err.to_string();
                    }
                    continue;
                }
            };

            let mut stdout_pipe = child.stdout.take();
            let stdout_handle = thread::spawn(move || -> Result<Vec<u8>, std::io::Error> {
                let mut buf = Vec::new();
                if let Some(mut pipe) = stdout_pipe.take() {
                    let mut limited =
                        (&mut pipe).take((MAX_TAILSCALE_STATUS_JSON_BYTES as u64) + 1);
                    limited.read_to_end(&mut buf)?;
                }
                Ok(buf)
            });

            let deadline = std::time::Instant::now() + DEFAULT_TAILSCALE_CLI_TIMEOUT;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(s)) => break s,
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(TailscaleError::CliExecutionFailed {
                                reason: format!(
                                    "tailscale status --json timed out after {DEFAULT_TAILSCALE_CLI_TIMEOUT:?}"
                                ),
                            });
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => {
                        return Err(TailscaleError::CliExecutionFailed {
                            reason: err.to_string(),
                        });
                    }
                }
            };

            let raw_bytes = match stdout_handle.join() {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(err)) => {
                    return Err(TailscaleError::CliExecutionFailed {
                        reason: err.to_string(),
                    });
                }
                Err(_) => {
                    return Err(TailscaleError::CliExecutionFailed {
                        reason: "stdout reader thread panicked".to_owned(),
                    });
                }
            };

            if raw_bytes.len() > MAX_TAILSCALE_STATUS_JSON_BYTES {
                return Err(TailscaleError::StatusOutputTooLarge {
                    bytes: raw_bytes.len(),
                });
            }

            if !status.success() && raw_bytes.is_empty() {
                return Err(TailscaleError::CliExecutionFailed {
                    reason: format!("tailscale exited with status {status}"),
                });
            }

            let raw_json =
                String::from_utf8(raw_bytes).map_err(|e| TailscaleError::InvalidStatusJson {
                    reason: format!("non-UTF-8 output: {e}"),
                })?;

            return parse_tailscale_status_json(&raw_json);
        }

        if last_not_found {
            Err(TailscaleError::CliNotFound)
        } else {
            Err(TailscaleError::CliExecutionFailed {
                reason: last_err_reason,
            })
        }
    }
}

/// Bind address selection for [`TailscaleNodeShareForwarder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailscaleBindMode {
    /// Production mode: bind the forwarder listener directly on the host's `100.64.0.0/10`
    /// Tailscale IPv4 address and the mapping's configured port (e.g. `100.x.y.z:25565`).
    TailscaleInterface,
    /// Integration-test mode: bind the listener on a local loopback IP and port (`127.0.0.1:0` or
    /// a fixed test port) so automated tests can exercise full TCP forwarding on CI runners without
    /// a live Tailscale virtual NIC, while still reporting the validated `100.64.0.0/10` Tailscale
    /// address in [`PublicMappingEndpoint`].
    LoopbackForTest {
        /// Loopback IP to bind (`127.0.0.1`).
        bind_ip: Ipv4Addr,
        /// Port to bind (`0` for OS-assigned ephemeral port).
        bind_port: u16,
    },
}

/// Bound endpoint metadata for an active Tailscale Node Share mapping (`ADR-022`, `FR-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleShareEndpoint {
    /// Wire index (`0..=7`).
    pub index: u16,
    /// Profile mapping key (e.g. `"game"`).
    pub mapping_key: String,
    /// Transport protocol (`Protocol::Tcp`).
    pub protocol: Protocol,
    /// Host's `100.64.0.0/10` Tailscale IPv4 address.
    pub tailscale_ipv4: Ipv4Addr,
    /// Optional MagicDNS hostname (e.g. `"host-pc.tailnet.ts.net"`).
    pub dns_name: Option<String>,
    /// Exposed TCP port on the Tailscale node (e.g. `25565`).
    pub public_port: u16,
    /// Actual local socket address bound by the forwarder listener.
    pub bound_listener_addr: SocketAddr,
}

impl TailscaleShareEndpoint {
    /// Format as `<tailscale_ipv4>:<port>` (e.g. `"100.101.102.103:25565"`).
    pub fn display_tailscale_ip_address(&self) -> String {
        format!("{}:{}", self.tailscale_ipv4, self.public_port)
    }

    /// Format as `<dns_name>:<port>` if MagicDNS is enabled on the tailnet.
    pub fn display_magic_dns_address(&self) -> Option<String> {
        self.dns_name
            .as_ref()
            .map(|dns| format!("{dns}:{}", self.public_port))
    }

    /// Convert to a [`PublicMappingEndpoint`] for [` crate::lifecycle::EngineStatusSnapshot`].
    pub fn to_public_mapping_endpoint(&self) -> PublicMappingEndpoint {
        PublicMappingEndpoint {
            index: self.index,
            mapping_key: self.mapping_key.clone(),
            protocol: self.protocol,
            relay_host: self.tailscale_ipv4.to_string(),
            public_port: self.public_port,
        }
    }
}

/// Live traffic counters tracked by [`TailscaleNodeShareForwarder`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TailscaleForwarderMetrics {
    /// Currently open TCP streams from Tailscale peers to the local game server.
    pub active_tcp_flows: u32,
    /// Count of distinct peer IP addresses observed during this session (capped at
    /// [`MAX_TAILSCALE_PEERS_TRACKED`]).
    pub distinct_sources: u32,
    /// Total connections that successfully dialed the local game server.
    pub total_connected: u64,
    /// Total connections refused (local server unreachable or flow cap reached).
    pub total_refused: u64,
    /// Total bytes forwarded from Tailscale guests to the local game server.
    pub bytes_to_local: u64,
    /// Total bytes forwarded from the local game server to Tailscale guests.
    pub bytes_from_local: u64,
}

#[derive(Debug, Default)]
struct SharedTailscaleMetrics {
    active_tcp_flows: u32,
    distinct_peers: HashSet<IpAddr>,
    total_connected: u64,
    total_refused: u64,
    bytes_to_local: u64,
    bytes_from_local: u64,
}

/// Host-local TCP forwarder that binds enabled profile mappings on the host's Tailscale IP and
/// bridges connections to the loopback game server (`127.0.0.1:25565`, `ADR-022`, `SEC-006`,
/// `SRV-005`).
pub struct TailscaleNodeShareForwarder {
    endpoints: Vec<TailscaleShareEndpoint>,
    stop_flag: Arc<AtomicBool>,
    metrics: Arc<Mutex<SharedTailscaleMetrics>>,
    accept_threads: Vec<JoinHandle<()>>,
}

impl fmt::Debug for TailscaleNodeShareForwarder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailscaleNodeShareForwarder")
            .field("endpoints", &self.endpoints)
            .field("stopped", &self.stop_flag.load(Ordering::SeqCst))
            .field("metrics", &self.metrics())
            .finish()
    }
}

impl TailscaleNodeShareForwarder {
    /// Start TCP forwarder listeners for all enabled TCP mappings in `target_table` (`SEC-006`,
    /// `GAME-008`).
    ///
    /// Optional mappings that are disabled (`rcon`, `query`) are never bound or exposed.
    pub fn start(
        target_table: &ValidatedLocalTargetTable,
        node_status: &TailscaleNodeStatus,
        bind_mode: TailscaleBindMode,
        max_tcp_flows: u16,
    ) -> Result<Self, TailscaleError> {
        let tailscale_ipv4 = node_status.require_ready_for_node_share()?;
        let max_flows = u32::from(max_tcp_flows.clamp(1, 4096));

        let enabled_tcp: Vec<_> = target_table
            .targets()
            .into_iter()
            .enumerate()
            .filter(|(_, m)| m.enabled() && m.protocol() == Protocol::Tcp)
            .collect();

        if enabled_tcp.is_empty() {
            return Err(TailscaleError::NoEnabledTcpMappings);
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let metrics = Arc::new(Mutex::new(SharedTailscaleMetrics::default()));
        let mut endpoints = Vec::with_capacity(enabled_tcp.len());
        let mut accept_threads = Vec::with_capacity(enabled_tcp.len());

        for (idx, mapping) in enabled_tcp {
            let local_target_addr = mapping.local_addr();
            let desired_bind_addr = match bind_mode {
                TailscaleBindMode::TailscaleInterface => {
                    SocketAddr::new(IpAddr::V4(tailscale_ipv4), local_target_addr.port())
                }
                TailscaleBindMode::LoopbackForTest { bind_ip, bind_port } => {
                    SocketAddr::new(IpAddr::V4(bind_ip), bind_port)
                }
            };

            let listener = TcpListener::bind(desired_bind_addr).map_err(|err| {
                stop_flag.store(true, Ordering::SeqCst);
                TailscaleError::ListenerBindFailed {
                    mapping_key: mapping.mapping_key().to_owned(),
                    bind_addr: desired_bind_addr,
                    reason: err.to_string(),
                }
            })?;

            let bound_addr =
                listener
                    .local_addr()
                    .map_err(|err| TailscaleError::ListenerBindFailed {
                        mapping_key: mapping.mapping_key().to_owned(),
                        bind_addr: desired_bind_addr,
                        reason: err.to_string(),
                    })?;

            let _ = listener.set_nonblocking(true);

            let public_port = match bind_mode {
                TailscaleBindMode::TailscaleInterface => local_target_addr.port(),
                TailscaleBindMode::LoopbackForTest { .. } => bound_addr.port(),
            };

            endpoints.push(TailscaleShareEndpoint {
                index: mapping
                    .wire_index()
                    .unwrap_or_else(|| u16::try_from(idx).unwrap_or(0)),
                mapping_key: mapping.mapping_key().to_owned(),
                protocol: Protocol::Tcp,
                tailscale_ipv4,
                dns_name: node_status.dns_name.clone(),
                public_port,
                bound_listener_addr: bound_addr,
            });

            let thread_stop = Arc::clone(&stop_flag);
            let thread_metrics = Arc::clone(&metrics);
            let handle = thread::spawn(move || {
                run_tailscale_accept_loop(
                    listener,
                    local_target_addr,
                    max_flows,
                    thread_stop,
                    thread_metrics,
                );
            });
            accept_threads.push(handle);
        }

        Ok(Self {
            endpoints,
            stop_flag,
            metrics,
            accept_threads,
        })
    }

    /// Borrow the bound Tailscale Node Share endpoints.
    pub fn endpoints(&self) -> &[TailscaleShareEndpoint] {
        &self.endpoints
    }

    /// Read a snapshot of current forwarder metrics.
    pub fn metrics(&self) -> TailscaleForwarderMetrics {
        let Ok(guard) = self.metrics.lock() else {
            return TailscaleForwarderMetrics::default();
        };
        TailscaleForwarderMetrics {
            active_tcp_flows: guard.active_tcp_flows,
            distinct_sources: u32::try_from(guard.distinct_peers.len()).unwrap_or(u32::MAX),
            total_connected: guard.total_connected,
            total_refused: guard.total_refused,
            bytes_to_local: guard.bytes_to_local,
            bytes_from_local: guard.bytes_from_local,
        }
    }

    /// Build a [`PlayersSeenSnapshot`] from current Tailscale forwarder counters (`FR-012`,
    /// `UX-002`).
    pub fn players_seen_snapshot(&self) -> PlayersSeenSnapshot {
        let m = self.metrics();
        PlayersSeenSnapshot {
            distinct_sources: m.distinct_sources,
            tcp_flows: m.active_tcp_flows,
            udp_flows: 0,
            label: OBSERVED_TRAFFIC_LABEL,
        }
    }

    /// Stop all Tailscale Node Share listener threads cleanly.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        for ep in &self.endpoints {
            let _ = TcpStream::connect_timeout(&ep.bound_listener_addr, Duration::from_millis(50));
        }
        for handle in self.accept_threads.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for TailscaleNodeShareForwarder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tailscale_accept_loop(
    listener: TcpListener,
    local_target_addr: SocketAddr,
    max_flows: u32,
    stop_flag: Arc<AtomicBool>,
    metrics: Arc<Mutex<SharedTailscaleMetrics>>,
) {
    while !stop_flag.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((guest_stream, peer_addr)) => {
                if stop_flag.load(Ordering::SeqCst) {
                    let _ = guest_stream.shutdown(Shutdown::Both);
                    break;
                }

                let mut allow = false;
                if let Ok(mut guard) = metrics.lock() {
                    if guard.active_tcp_flows < max_flows {
                        guard.active_tcp_flows = guard.active_tcp_flows.saturating_add(1);
                        allow = true;
                    } else {
                        guard.total_refused = guard.total_refused.saturating_add(1);
                    }
                }

                if !allow {
                    let _ = guest_stream.shutdown(Shutdown::Both);
                    continue;
                }

                let worker_metrics = Arc::clone(&metrics);
                let peer_ip = peer_addr.ip();
                thread::spawn(move || {
                    let _ = guest_stream.set_nonblocking(false);
                    let _ = guest_stream.set_read_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));
                    let _ = guest_stream.set_write_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));

                    let local_stream = match TcpStream::connect_timeout(
                        &local_target_addr,
                        DEFAULT_LOCAL_DIAL_TIMEOUT,
                    ) {
                        Ok(s) => s,
                        Err(_) => {
                            let _ = guest_stream.shutdown(Shutdown::Both);
                            if let Ok(mut guard) = worker_metrics.lock() {
                                guard.active_tcp_flows = guard.active_tcp_flows.saturating_sub(1);
                                guard.total_refused = guard.total_refused.saturating_add(1);
                            }
                            return;
                        }
                    };

                    let _ = local_stream.set_read_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));
                    let _ = local_stream.set_write_timeout(Some(DEFAULT_ENGINE_TCP_IO_TIMEOUT));

                    if let Ok(mut guard) = worker_metrics.lock() {
                        guard.total_connected = guard.total_connected.saturating_add(1);
                        if guard.distinct_peers.len() < MAX_TAILSCALE_PEERS_TRACKED {
                            guard.distinct_peers.insert(peer_ip);
                        }
                    }

                    let (to_local, from_local) =
                        bridge_engine_tcp_streams(guest_stream, local_stream).unwrap_or((0, 0));

                    if let Ok(mut guard) = worker_metrics.lock() {
                        guard.active_tcp_flows = guard.active_tcp_flows.saturating_sub(1);
                        guard.bytes_to_local = guard.bytes_to_local.saturating_add(to_local);
                        guard.bytes_from_local = guard.bytes_from_local.saturating_add(from_local);
                    }
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tailscale_cgnat_ipv4_and_ula_ipv6_ranges() {
        assert!(is_tailscale_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_tailscale_ipv4(Ipv4Addr::new(100, 101, 102, 103)));
        assert!(is_tailscale_ipv4(Ipv4Addr::new(100, 127, 255, 254)));

        assert!(!is_tailscale_ipv4(Ipv4Addr::new(100, 63, 255, 255)));
        assert!(!is_tailscale_ipv4(Ipv4Addr::new(100, 128, 0, 0)));
        assert!(!is_tailscale_ipv4(Ipv4Addr::new(192, 168, 1, 10)));
        assert!(!is_tailscale_ipv4(Ipv4Addr::new(127, 0, 0, 1)));

        let ts_v6: Ipv6Addr = "fd7a:115c:a1e0::1".parse().unwrap_or(Ipv6Addr::LOCALHOST);
        assert!(is_tailscale_ipv6(ts_v6));
        assert!(!is_tailscale_ipv6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn parses_running_tailscale_status_json_and_extracts_addresses() {
        let json = r#"{
            "BackendState": "Running",
            "Self": {
                "HostName": "chris-gaming-pc",
                "DNSName": "chris-gaming-pc.tail1234.ts.net.",
                "Online": true,
                "TailscaleIPs": [
                    "100.98.42.17",
                    "fd7a:115c:a1e0::abcd:1234"
                ]
            }
        }"#;

        let status = parse_tailscale_status_json(json);
        assert!(status.is_ok());
        let Some(status) = status.ok() else {
            return;
        };

        assert_eq!(status.backend_state, TailscaleBackendState::Running);
        assert!(status.online);
        assert_eq!(status.tailscale_ipv4, Some(Ipv4Addr::new(100, 98, 42, 17)));
        assert_eq!(
            status.dns_name.as_deref(),
            Some("chris-gaming-pc.tail1234.ts.net")
        );
        assert_eq!(status.host_name, "chris-gaming-pc");
        assert_eq!(
            status.require_ready_for_node_share(),
            Ok(Ipv4Addr::new(100, 98, 42, 17))
        );
    }

    #[test]
    fn rejects_needs_login_stopped_offline_and_non_cgnat_ips() {
        let needs_login = r#"{"BackendState": "NeedsLogin", "Self": {"Online": false}}"#;
        let res =
            parse_tailscale_status_json(needs_login).and_then(|s| s.require_ready_for_node_share());
        assert_eq!(
            res,
            Err(TailscaleError::NotAuthenticated {
                state: "NeedsLogin".to_owned()
            })
        );

        let stopped = r#"{"BackendState": "Stopped", "Self": {"Online": false}}"#;
        let res =
            parse_tailscale_status_json(stopped).and_then(|s| s.require_ready_for_node_share());
        assert_eq!(res, Err(TailscaleError::DaemonStopped));

        let offline = r#"{
            "BackendState": "Running",
            "Self": {
                "HostName": "pc",
                "Online": false,
                "TailscaleIPs": ["100.70.1.2"]
            }
        }"#;
        let res =
            parse_tailscale_status_json(offline).and_then(|s| s.require_ready_for_node_share());
        assert_eq!(res, Err(TailscaleError::NodeOffline));

        let non_cgnat = r#"{
            "BackendState": "Running",
            "Self": {
                "HostName": "pc",
                "Online": true,
                "TailscaleIPs": ["192.168.1.50"]
            }
        }"#;
        let res =
            parse_tailscale_status_json(non_cgnat).and_then(|s| s.require_ready_for_node_share());
        assert_eq!(res, Err(TailscaleError::MissingTailscaleIpv4));
    }
}
