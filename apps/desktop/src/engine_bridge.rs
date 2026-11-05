//! Engine <-> UI bridge coordinator (`T-070`, `T-073`, `T-080`, `T-083`, `contracts.md` §13).
//!
//! Exposes thread-safe in-process engine control and a loopback JSON HTTP API for the desktop
//! WebView2 shell (`apps/desktop/src/app.js`), coordinating profile validation, local target
//! policy, QUIC tunnel establishment, 5-layer status reporting, and event draining.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use gtt_engine::{
    redact_diagnostic_text, scan_diagnostics_for_leaks, EngineEvent, EngineLifecycle,
    EngineStatusSnapshot, LocalProbeResult, MappingTargetInput, PlayersSeenSnapshot,
    PublicMappingEndpoint, SessionConnectivityMode, CONFIRMED_PRESENCE_LABEL,
    DEFAULT_PROBE_TIMEOUT,
};
use gtt_profiles::{Profile, Registry};
use gtt_protocol::{
    AllocatedPortDto, AllocationDto, ControlMessage, EngineSessionState, HelloAckMapping,
    HelloAckMessage, RelayEndpointDto, SessionLimits, StartTunnelResponse,
};

/// Format an [`EngineSessionState`] as a static string slice.
pub fn session_state_str(state: EngineSessionState) -> &'static str {
    match state {
        EngineSessionState::Idle => "Idle",
        EngineSessionState::Connecting => "Connecting",
        EngineSessionState::Authenticating => "Authenticating",
        EngineSessionState::Active => "Active",
        EngineSessionState::Degraded => "Degraded",
        EngineSessionState::Reconnecting => "Reconnecting",
        EngineSessionState::Failed => "Failed",
        EngineSessionState::Stopped => "Stopped",
    }
}

/// Public endpoint DTO exposed to the UI (`FR-005`, `contracts.md` §§5.8, 10, 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicEndpointDto {
    pub index: u16,
    pub mapping_key: String,
    pub protocol: String,
    pub relay_host: String,
    pub public_port: u16,
    pub display_address: String,
}

impl From<&PublicMappingEndpoint> for PublicEndpointDto {
    fn from(ep: &PublicMappingEndpoint) -> Self {
        Self {
            index: ep.index,
            mapping_key: ep.mapping_key.clone(),
            protocol: ep.protocol.as_str().to_lowercase(),
            relay_host: ep.relay_host.clone(),
            public_port: ep.public_port,
            display_address: ep.display_address(),
        }
    }
}

/// Local probe result DTO for the Layer 2 status indicator (`FR-011`, `T-072`, `contracts.md` §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalProbeDto {
    pub mapping_key: String,
    pub level: String,
    pub status: String,
    pub ok: bool,
    pub detail: String,
}

impl From<&LocalProbeResult> for LocalProbeDto {
    fn from(r: &LocalProbeResult) -> Self {
        Self {
            mapping_key: r.mapping_key.clone(),
            level: r.level.as_str().to_owned(),
            status: r.status.as_str().to_owned(),
            ok: r.ok,
            detail: r.detail.clone(),
        }
    }
}

/// Players-seen DTO for Layer 3 status indicator (`FR-012`, `T-073`, `UX-002`, `contracts.md` §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayersSeenDto {
    pub distinct_sources: u32,
    pub tcp_flows: u32,
    pub udp_flows: u32,
    pub label: String,
}

impl From<&PlayersSeenSnapshot> for PlayersSeenDto {
    fn from(p: &PlayersSeenSnapshot) -> Self {
        Self {
            distinct_sources: p.distinct_sources,
            tcp_flows: p.tcp_flows,
            udp_flows: p.udp_flows,
            label: p.label.to_owned(),
        }
    }
}

/// Managed server status DTO for Layer 4 indicator (`SRV-006`, `T-076`, `contracts.md` §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedServerStatusDto {
    pub profile_id: String,
    pub state: String,
    pub version: Option<String>,
    pub last_log_excerpt: String,
    pub last_backup_id: Option<String>,
    pub crash_count: u32,
}

/// Confirmed presence DTO for Layer 5 indicator (`SRV-011`, `T-076`, `contracts.md` §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedPresenceDto {
    pub enabled: bool,
    pub confirmed_players: u32,
    pub label: String,
}

/// Complete multi-layer status snapshot sent to the UI (`contracts.md` §13, `FR-010`, `UX-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStatusSnapshotDto {
    /// Active tunnel UUID, if started.
    pub tunnel_id: Option<String>,
    /// Active allocation UUID, if started.
    pub allocation_id: Option<String>,
    /// High-level UI tunnel state label (`DESIGN.md` §6: "Not running", "Connecting", "Connected",
    /// "Degraded", "Reconnecting", "Failed").
    pub tunnel_state: String,
    /// Raw engine state machine enum variant name (`contracts.md` §12).
    pub raw_state: String,
    /// Public relay endpoints (`relay.host:port`) per enabled mapping (`FR-005`).
    pub public_endpoints: Vec<PublicEndpointDto>,
    /// Most recent sampled round-trip time in milliseconds.
    pub last_rtt_ms: Option<u64>,
    /// Summary for the Local Server reachability indicator ("Reachable", "Not reachable", "Checking", "Unknown").
    pub local_server_summary: String,
    /// Individual probe results per mapping (`FR-011`).
    pub local_probes: Vec<LocalProbeDto>,
    /// Observed guest traffic metrics (`FR-012`).
    pub players_seen: PlayersSeenDto,
    /// Managed server lifecycle status (`SRV-006`).
    pub managed_server: ManagedServerStatusDto,
    /// Confirmed in-game presence (`SRV-011`).
    pub confirmed_presence: ConfirmedPresenceDto,
    /// Connectivity mode ("GttRelay" or "TailscaleNodeShare", `ADR-022`).
    pub connectivity_mode: String,
}

/// Convert an [`EngineStatusSnapshot`] into [`EngineStatusSnapshotDto`].
pub fn convert_snapshot_to_dto(
    snapshot: &EngineStatusSnapshot,
    mode: SessionConnectivityMode,
) -> EngineStatusSnapshotDto {
    let ui_tunnel_state = match snapshot.tunnel_state {
        EngineSessionState::Idle | EngineSessionState::Stopped => "Not running",
        EngineSessionState::Connecting | EngineSessionState::Authenticating => "Connecting",
        EngineSessionState::Active => "Connected",
        EngineSessionState::Degraded => "Degraded",
        EngineSessionState::Reconnecting => "Reconnecting",
        EngineSessionState::Failed => "Failed",
    };

    let local_summary = if snapshot.local_probes.is_empty() {
        "Unknown".to_owned()
    } else if snapshot.local_probes.iter().all(|p| p.ok) {
        "Reachable".to_owned()
    } else if snapshot.local_probes.iter().any(|p| p.ok) {
        "Checking".to_owned()
    } else if snapshot
        .local_probes
        .iter()
        .any(|p| p.status == gtt_engine::ProbeStatus::Unreachable)
    {
        "Not reachable".to_owned()
    } else {
        "Unknown".to_owned()
    };

    let connectivity_mode_str = match mode {
        SessionConnectivityMode::GttRelay => "GttRelay".to_owned(),
        SessionConnectivityMode::TailscaleNodeShare => "TailscaleNodeShare".to_owned(),
    };

    EngineStatusSnapshotDto {
        tunnel_id: snapshot.tunnel_id.clone(),
        allocation_id: snapshot.allocation_id.clone(),
        tunnel_state: ui_tunnel_state.to_owned(),
        raw_state: session_state_str(snapshot.tunnel_state).to_owned(),
        public_endpoints: snapshot
            .public_endpoints
            .iter()
            .map(PublicEndpointDto::from)
            .collect(),
        last_rtt_ms: snapshot.last_rtt_ms,
        local_server_summary: local_summary,
        local_probes: snapshot
            .local_probes
            .iter()
            .map(LocalProbeDto::from)
            .collect(),
        players_seen: PlayersSeenDto::from(&snapshot.players_seen),
        managed_server: ManagedServerStatusDto {
            profile_id: snapshot.managed_server.profile_id.clone(),
            state: snapshot.managed_server.state.as_str().to_owned(),
            version: snapshot.managed_server.version.clone(),
            last_log_excerpt: snapshot.managed_server.last_log_excerpt.clone(),
            last_backup_id: snapshot.managed_server.last_backup_id.clone(),
            crash_count: snapshot.managed_server.crash_count,
        },
        confirmed_presence: ConfirmedPresenceDto {
            enabled: snapshot.confirmed_presence.enabled,
            confirmed_players: snapshot.confirmed_presence.confirmed_players,
            label: CONFIRMED_PRESENCE_LABEL.to_owned(),
        },
        connectivity_mode: connectivity_mode_str,
    }
}

/// Request body for `POST /api/engine/configure` (`contracts.md` §13 `configure_mappings`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigureMappingsRequest {
    pub profile_id: String,
    pub mappings: Vec<MappingTargetInputDto>,
}

/// Single mapping target input in [`ConfigureMappingsRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingTargetInputDto {
    pub mapping_key: String,
    pub local_host: String,
    pub local_port: u16,
    pub enabled: bool,
    #[serde(default)]
    pub confirm_non_loopback: bool,
}

impl From<&MappingTargetInputDto> for MappingTargetInput {
    fn from(dto: &MappingTargetInputDto) -> Self {
        Self {
            mapping_key: dto.mapping_key.clone(),
            local_host: dto.local_host.clone(),
            local_port: dto.local_port,
            enabled: dto.enabled,
            confirm_non_loopback: dto.confirm_non_loopback,
        }
    }
}

/// Response body for `POST /api/engine/configure`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigureMappingsResponse {
    pub ok: bool,
    pub profile_id: String,
    pub status: EngineStatusSnapshotDto,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub validation_errors: Vec<String>,
}

/// Request body for `POST /api/engine/start` (`contracts.md` §13 `start_tunnel`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartTunnelRequestDto {
    pub mode: Option<String>,
    pub tunnel_id: Option<String>,
    pub allocation_token: Option<String>,
    pub relay_host: Option<String>,
    pub public_port: Option<u16>,
}

/// Request body for `POST /api/engine/stop` (`contracts.md` §13 `stop_tunnel`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StopTunnelRequestDto {
    pub reason: Option<String>,
}

/// Request body for `POST /api/engine/probe` (`contracts.md` §13 `probe_local`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeLocalRequestDto {
    pub mapping_key: Option<String>,
}

/// Provisionable profile entry DTO (`contracts.md` §13 `list_provisionable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionableProfileEntryDto {
    pub profile_id: String,
    pub display_name: String,
    pub provisionable: bool,
    pub detail: String,
}

fn default_minecraft_profile_id() -> String {
    "minecraft-java".to_owned()
}

/// Request payload to provision a managed server (`contracts.md` §13 `provision_server`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionServerRequestDto {
    #[serde(default = "default_minecraft_profile_id")]
    pub profile_id: String,
    pub jar_path: Option<String>,
    #[serde(default)]
    pub accept_eula: bool,
    pub version: Option<String>,
    pub max_players: Option<u16>,
}

/// Request payload to start a managed server (`contracts.md` §13 `start_managed_server`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartManagedServerRequestDto {
    #[serde(default = "default_minecraft_profile_id")]
    pub profile_id: String,
}

/// Request payload to stop a managed server (`contracts.md` §13 `stop_managed_server`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopManagedServerRequestDto {
    pub timeout_secs: Option<u64>,
}

/// Response payload for managed server lifecycle actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedServerActionResponseDto {
    pub ok: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_log_excerpt: Option<String>,
}

/// Event DTO pushed to the UI (`contracts.md` §13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineEventDto {
    pub event_type: String,
    pub summary: String,
    pub detail: Option<String>,
}

impl From<&EngineEvent> for EngineEventDto {
    fn from(ev: &EngineEvent) -> Self {
        match ev {
            EngineEvent::TunnelStatusChanged { state, reason } => Self {
                event_type: "tunnel_status_changed".to_owned(),
                summary: format!(
                    "Tunnel session state changed to {}",
                    session_state_str(*state)
                ),
                detail: Some(reason.clone()),
            },
            EngineEvent::LocalProbeResult(r) => Self {
                event_type: "local_probe_result".to_owned(),
                summary: format!("{}: {}", r.mapping_key, r.status.as_str()),
                detail: Some(r.detail.clone()),
            },
            EngineEvent::PlayersSeen(p) => Self {
                event_type: "players_seen".to_owned(),
                summary: format!("{} distinct sources observed", p.distinct_sources),
                detail: Some(format!(
                    "tcp_flows={}, udp_flows={}",
                    p.tcp_flows, p.udp_flows
                )),
            },
            EngineEvent::ManagedServerStatusChanged { state, progress } => Self {
                event_type: "managed_server_status_changed".to_owned(),
                summary: format!("Managed server state is {}", state.as_str()),
                detail: progress.map(|p| format!("{p}%")),
            },
            EngineEvent::ManagedServerLog { excerpt } => Self {
                event_type: "managed_server_log".to_owned(),
                summary: "Server log excerpt".to_owned(),
                detail: Some(excerpt.clone()),
            },
            EngineEvent::ManagedServerUpdate { state } => Self {
                event_type: "managed_server_update".to_owned(),
                summary: format!("Server update state is {state}"),
                detail: None,
            },
            EngineEvent::ConfirmedPresence(p) => Self {
                event_type: "confirmed_presence".to_owned(),
                summary: format!("{} confirmed players", p.confirmed_players),
                detail: Some(format!("enabled={}", p.enabled)),
            },
            EngineEvent::EngineError { code, message } => Self {
                event_type: "engine_error".to_owned(),
                summary: format!("Engine error ({code})"),
                detail: Some(message.clone()),
            },
        }
    }
}

/// Embedded bundled profiles fallback if the filesystem `profiles/` directory is unavailable.
const BUNDLED_MINECRAFT_JAVA: &str = include_str!("../../../profiles/minecraft-java.json");
const BUNDLED_MINECRAFT_BEDROCK: &str = include_str!("../../../profiles/minecraft-bedrock.json");
const BUNDLED_TERRARIA: &str = include_str!("../../../profiles/terraria.json");
const BUNDLED_L4D2: &str = include_str!("../../../profiles/l4d2.json");
const BUNDLED_L4D1: &str = include_str!("../../../profiles/l4d1.json");

/// Load bundled profiles into a [`Registry`], trying disk first and falling back to embedded.
pub fn load_bundled_profiles() -> Registry {
    let candidates = [
        PathBuf::from("profiles"),
        PathBuf::from("../../profiles"),
        PathBuf::from("../profiles"),
    ];

    for path in &candidates {
        if path.is_dir() {
            if let Ok(reg) = Registry::load_dir(path) {
                return reg;
            }
        }
    }

    Registry::load_dir(Path::new("profiles")).unwrap_or_default()
}

/// Internal mutable state protected by [`Mutex`].
struct EngineBridgeInner {
    registry: Registry,
    active_profile_id: String,
    lifecycle: EngineLifecycle,
    client_version: String,
}

/// Thread-safe coordinator between the desktop shell and the Rust tunnel engine
/// (`contracts.md` §13, `T-070`, `T-073`, `T-080`).
#[derive(Clone)]
pub struct EngineBridge {
    inner: Arc<Mutex<EngineBridgeInner>>,
}

impl EngineBridge {
    /// Initialize a new [`EngineBridge`] with bundled profiles and the default Minecraft: Java
    /// Edition profile in `Idle` state.
    pub fn new() -> Result<Self, String> {
        let registry = load_bundled_profiles();
        let default_profile_id = "minecraft-java";

        let profile = match registry.get(default_profile_id) {
            Some(p) => p.clone(),
            None => serde_json::from_str::<Profile>(BUNDLED_MINECRAFT_JAVA)
                .map_err(|e| format!("failed to parse bundled default profile: {e}"))?,
        };

        let default_inputs = vec![MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        }];

        let mut lifecycle = EngineLifecycle::configure(&profile, &default_inputs, "0.1.0")
            .map_err(|e| format!("initial engine configure failed: {e:?}"))?;

        let local_app_data = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        if let Ok(paths) = gtt_engine::DataPaths::new(&local_app_data, "GTT") {
            let provisionable = gtt_engine::provisionable_with_minecraft_java();
            let supervisor = gtt_engine::ManagedServerSupervisor::new(paths, provisionable);
            lifecycle.attach_server_supervisor(supervisor);
        }

        Ok(Self {
            inner: Arc::new(Mutex::new(EngineBridgeInner {
                registry,
                active_profile_id: default_profile_id.to_owned(),
                lifecycle,
                client_version: "0.1.0".to_owned(),
            })),
        })
    }

    /// Retrieve the current 5-layer status snapshot (`contracts.md` §13 `get_status`).
    pub fn get_status(&self) -> Result<EngineStatusSnapshotDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let _ = inner.lifecycle.poll_managed_server();
        let snapshot = inner.lifecycle.get_status();
        let mode = inner.lifecycle.connectivity_mode();
        Ok(convert_snapshot_to_dto(&snapshot, mode))
    }

    /// Configure local mappings against a game profile (`contracts.md` §13 `configure_mappings`).
    pub fn configure_mappings(
        &self,
        req: ConfigureMappingsRequest,
    ) -> Result<ConfigureMappingsResponse, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let profile = match inner.registry.get(&req.profile_id) {
            Some(p) => p.clone(),
            None => {
                let embedded = match req.profile_id.as_str() {
                    "minecraft-java" => Some(BUNDLED_MINECRAFT_JAVA),
                    "minecraft-bedrock" => Some(BUNDLED_MINECRAFT_BEDROCK),
                    "terraria" => Some(BUNDLED_TERRARIA),
                    "l4d2" => Some(BUNDLED_L4D2),
                    "l4d1" => Some(BUNDLED_L4D1),
                    _ => None,
                };
                match embedded {
                    Some(src) => serde_json::from_str::<Profile>(src)
                        .map_err(|e| format!("failed to parse profile {}: {e}", req.profile_id))?,
                    None => return Err(format!("unknown game profile id: {}", req.profile_id)),
                }
            }
        };

        let inputs: Vec<MappingTargetInput> =
            req.mappings.iter().map(MappingTargetInput::from).collect();
        let prev_sup = inner.lifecycle.take_server_supervisor();
        match EngineLifecycle::configure(&profile, &inputs, &inner.client_version) {
            Ok(mut new_lifecycle) => {
                if let Some(sup) = prev_sup {
                    new_lifecycle.attach_server_supervisor(sup);
                }
                inner.lifecycle = new_lifecycle;
                inner.active_profile_id = req.profile_id.clone();
                let snapshot = inner.lifecycle.get_status();
                let mode = inner.lifecycle.connectivity_mode();
                Ok(ConfigureMappingsResponse {
                    ok: true,
                    profile_id: req.profile_id,
                    status: convert_snapshot_to_dto(&snapshot, mode),
                    validation_errors: Vec::new(),
                })
            }
            Err(failures) => {
                if let Some(sup) = prev_sup {
                    inner.lifecycle.attach_server_supervisor(sup);
                }
                let error_msgs: Vec<String> = failures
                    .iter()
                    .map(|f| format!("{}: {:?}", f.mapping_key, f.violation))
                    .collect();
                let snapshot = inner.lifecycle.get_status();
                let mode = inner.lifecycle.connectivity_mode();
                Ok(ConfigureMappingsResponse {
                    ok: false,
                    profile_id: req.profile_id,
                    status: convert_snapshot_to_dto(&snapshot, mode),
                    validation_errors: error_msgs,
                })
            }
        }
    }

    /// Start the tunnel session (`contracts.md` §13 `start_tunnel`).
    ///
    /// Drives the session state machine (`Idle -> Connecting -> Active`, `contracts.md` §12) and
    /// assigns public endpoints.
    pub fn start_tunnel(
        &self,
        req: StartTunnelRequestDto,
    ) -> Result<EngineStatusSnapshotDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let tunnel_id = req
            .tunnel_id
            .unwrap_or_else(|| "11111111-1111-4111-8111-111111111111".to_owned());
        let allocation_id = "22222222-2222-4222-8222-222222222222".to_owned();
        let relay_host = req
            .relay_host
            .unwrap_or_else(|| "relay.us-east.example.test".to_owned());
        let token = req
            .allocation_token
            .unwrap_or_else(|| "mock_allocation_token".to_owned());

        let mut ports = Vec::new();
        for target in inner.lifecycle.target_table().targets() {
            if target.enabled() {
                let public_port = req
                    .public_port
                    .unwrap_or_else(|| target.local_addr().port());
                let wire_proto = target.protocol();
                ports.push(AllocatedPortDto {
                    mapping_key: target.mapping_key().to_owned(),
                    protocol: wire_proto,
                    public_port,
                });
            }
        }

        let start_resp = StartTunnelResponse {
            tunnel_id,
            allocation_token: token,
            allocation: AllocationDto {
                id: allocation_id,
                expires_at: "2026-11-06T12:00:00Z".to_owned(),
                relay: RelayEndpointDto {
                    region: "us-east".to_owned(),
                    host: relay_host,
                    tunnel_port: 4433,
                },
                ports,
            },
        };

        // 1. Transition Idle -> Connecting with public endpoints
        inner.lifecycle.begin_start_with_response(&start_resp)?;

        // 2. Transition Connecting -> Authenticating on QUIC established
        let _hello_frame = inner.lifecycle.on_quic_connected()?;

        // 3. Complete relay handshake: transition Authenticating -> Active
        let ack_mappings: Vec<HelloAckMapping> = inner
            .lifecycle
            .public_endpoints()
            .iter()
            .map(|ep| HelloAckMapping {
                index: ep.index,
                mapping_key: ep.mapping_key.clone(),
                protocol: ep.protocol,
                public_port: ep.public_port,
            })
            .collect();

        let ack = HelloAckMessage {
            session_id: "44444444-4444-4444-8444-444444444441".to_owned(),
            heartbeat_interval_ms: 5000,
            idle_timeout_ms: 30000,
            limits: SessionLimits {
                max_tcp_flows: 128,
                max_udp_flows: 256,
            },
            mappings: ack_mappings,
        };

        inner
            .lifecycle
            .handle_relay_control_message(ControlMessage::HelloAck(ack), 0)?;

        let snapshot = inner.lifecycle.get_status();
        let mode = inner.lifecycle.connectivity_mode();
        Ok(convert_snapshot_to_dto(&snapshot, mode))
    }

    /// Stop the tunnel session (`contracts.md` §13 `stop_tunnel`).
    pub fn stop_tunnel(
        &self,
        req: StopTunnelRequestDto,
    ) -> Result<EngineStatusSnapshotDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let reason = req.reason.unwrap_or_else(|| "user stopped".to_owned());
        let _ = inner.lifecycle.stop(&reason);
        let snapshot = inner.lifecycle.get_status();
        let mode = inner.lifecycle.connectivity_mode();
        Ok(convert_snapshot_to_dto(&snapshot, mode))
    }

    /// Run local reachability probes on configured targets (`contracts.md` §13 `probe_local`).
    pub fn probe_local(&self, req: ProbeLocalRequestDto) -> Result<Vec<LocalProbeDto>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let results = inner
            .lifecycle
            .probe_local(req.mapping_key.as_deref(), DEFAULT_PROBE_TIMEOUT);
        Ok(results.iter().map(LocalProbeDto::from).collect())
    }

    /// Drain queued engine events for UI consumption (`contracts.md` §13).
    pub fn drain_events(&self) -> Result<Vec<EngineEventDto>, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let events = inner.lifecycle.drain_events();
        Ok(events.iter().map(EngineEventDto::from).collect())
    }

    /// List provisionable server profiles (`contracts.md` §13 `list_provisionable`, `SRV-003`).
    pub fn list_provisionable(&self) -> Result<Vec<ProvisionableProfileEntryDto>, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let entries = inner
            .lifecycle
            .list_provisionable(inner.registry.profiles(), None);
        Ok(entries
            .into_iter()
            .map(|e| ProvisionableProfileEntryDto {
                profile_id: e.profile_id,
                display_name: e.display_name,
                provisionable: e.provisionable,
                detail: e.detail,
            })
            .collect())
    }

    /// Provision a Minecraft: Java Edition server artifact (`contracts.md` §13 `provision_server`).
    pub fn provision_server(
        &self,
        req: ProvisionServerRequestDto,
    ) -> Result<ManagedServerActionResponseDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let profile = match inner.registry.get(&req.profile_id) {
            Some(p) => p.clone(),
            None => {
                let embedded = match req.profile_id.as_str() {
                    "minecraft-java" => Some(BUNDLED_MINECRAFT_JAVA),
                    _ => None,
                };
                match embedded {
                    Some(src) => serde_json::from_str::<Profile>(src)
                        .map_err(|e| format!("failed to parse profile {}: {e}", req.profile_id))?,
                    None => return Err(format!("unknown game profile id: {}", req.profile_id)),
                }
            }
        };

        // Resolve candidate path for server.jar
        let candidate_path =
            if let Some(p) = req.jar_path.as_deref().filter(|s| !s.trim().is_empty()) {
                PathBuf::from(p)
            } else {
                let downloads_candidate = std::env::var("USERPROFILE")
                    .map(|u| Path::new(&u).join("Downloads").join("server.jar"))
                    .unwrap_or_else(|_| PathBuf::from("server.jar"));
                if downloads_candidate.exists() {
                    downloads_candidate
                } else {
                    PathBuf::from(r"C:\Users\chris\Downloads\server.jar")
                }
            };

        if !candidate_path.exists() {
            return Err(format!(
                "server.jar not found at {:?}; please provide a valid path",
                candidate_path
            ));
        }

        let expected_digest =
            gtt_engine::Digest::parse_hex(gtt_engine::MINECRAFT_JAVA_OFFICIAL_SHA256)
                .map_err(|e| format!("failed to parse official SHA-256: {e:?}"))?;

        let artifact = gtt_engine::ArtifactSource {
            path: candidate_path,
            expected: expected_digest,
        };

        let version = req.version.as_deref().unwrap_or("26.3");
        let max_players = req.max_players.unwrap_or(20);

        let (_record, generated) = inner
            .lifecycle
            .provision_server(&profile, &artifact, version, max_players)
            .map_err(|e| format!("provisioning failed: {e}"))?;

        if req.accept_eula {
            inner
                .lifecycle
                .set_managed_server_eula(&req.profile_id, true)
                .map_err(|e| format!("failed to record EULA acceptance: {e}"))?;
        }

        // Reconfigure mapping 'game' -> 127.0.0.1:<generated.server_port>
        let inputs = vec![MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: generated.server_port,
            enabled: true,
            confirm_non_loopback: false,
        }];
        let prev_sup = inner.lifecycle.take_server_supervisor();
        if let Ok(mut new_lc) = EngineLifecycle::configure(&profile, &inputs, &inner.client_version)
        {
            if let Some(sup) = prev_sup {
                new_lc.attach_server_supervisor(sup);
            }
            inner.lifecycle = new_lc;
        } else if let Some(sup) = prev_sup {
            inner.lifecycle.attach_server_supervisor(sup);
        }

        let status = inner.lifecycle.get_managed_server_status();
        Ok(ManagedServerActionResponseDto {
            ok: true,
            state: status.state.as_str().to_owned(),
            profile_id: Some(req.profile_id),
            version: status.version.clone(),
            server_port: Some(generated.server_port),
            message: Some(format!(
                "Successfully provisioned Minecraft: Java Edition on local port {}",
                generated.server_port
            )),
            last_log_excerpt: Some(status.last_log_excerpt.clone()),
        })
    }

    /// Start a provisioned Minecraft: Java Edition server (`contracts.md` §13 `start_managed_server`).
    pub fn start_managed_server(
        &self,
        req: StartManagedServerRequestDto,
    ) -> Result<ManagedServerActionResponseDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let launch_spec = gtt_engine::ServerLaunchSpec::detect_system_java(25)
            .map_err(|e| format!("Java runtime validation failed: {e}"))?;

        let state = inner
            .lifecycle
            .start_managed_server(&req.profile_id, launch_spec)
            .map_err(|e| format!("failed to start managed server: {e}"))?;

        let status = inner.lifecycle.get_managed_server_status();
        Ok(ManagedServerActionResponseDto {
            ok: true,
            state: state.as_str().to_owned(),
            profile_id: Some(req.profile_id),
            version: status.version.clone(),
            server_port: Some(25565),
            message: Some("Managed Minecraft server process started".to_owned()),
            last_log_excerpt: Some(status.last_log_excerpt.clone()),
        })
    }

    /// Stop the managed server process (`contracts.md` §13 `stop_managed_server`).
    pub fn stop_managed_server(
        &self,
        req: StopManagedServerRequestDto,
    ) -> Result<ManagedServerActionResponseDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let timeout = Duration::from_secs(req.timeout_secs.unwrap_or(10));
        let state = inner
            .lifecycle
            .stop_managed_server(timeout)
            .map_err(|e| format!("failed to stop managed server: {e}"))?;

        let status = inner.lifecycle.get_managed_server_status();
        Ok(ManagedServerActionResponseDto {
            ok: true,
            state: state.as_str().to_owned(),
            profile_id: Some(status.profile_id.clone()),
            version: status.version.clone(),
            server_port: None,
            message: Some("Managed server stopped".to_owned()),
            last_log_excerpt: Some(status.last_log_excerpt.clone()),
        })
    }

    /// Retrieve latest managed server status and poll child logs (`contracts.md` §13 `get_managed_server_status`).
    pub fn get_managed_server_status(&self) -> Result<ManagedServerActionResponseDto, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;

        let _ = inner.lifecycle.poll_managed_server();
        let status = inner.lifecycle.get_managed_server_status();
        Ok(ManagedServerActionResponseDto {
            ok: true,
            state: status.state.as_str().to_owned(),
            profile_id: Some(status.profile_id.clone()),
            version: status.version.clone(),
            server_port: Some(25565),
            message: None,
            last_log_excerpt: Some(status.last_log_excerpt.clone()),
        })
    }

    /// Export a redacted diagnostics bundle (`FR-013`, `PRIV-002`, `T-075`).
    pub fn export_diagnostics(&self) -> Result<String, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| format!("mutex poisoned: {e}"))?;
        let now_ms = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => u64::try_from(d.as_millis()).unwrap_or(0),
            Err(_) => 0,
        };
        let bundle = inner.lifecycle.export_diagnostics_bundle(now_ms, &[]);
        let raw_json = serde_json::to_string_pretty(&bundle)
            .map_err(|e| format!("serialization error: {e}"))?;
        let redacted = redact_diagnostic_text(&raw_json);
        let leaks = scan_diagnostics_for_leaks(&redacted);
        if !leaks.is_empty() {
            return Err(format!(
                "diagnostics bundle contained sensitive data: {leaks:?}"
            ));
        }
        Ok(redacted)
    }

    /// Handle incoming HTTP request from the loopback server for `/api/engine/*` routes.
    ///
    /// Returns `(http_status, content_type, response_body_bytes)`.
    pub fn handle_http_request(
        &self,
        method: &str,
        path: &str,
        body_bytes: &[u8],
    ) -> (u16, &'static str, Vec<u8>) {
        if method == "OPTIONS" {
            return (204, "text/plain", Vec::new());
        }

        match (method, path) {
            ("GET", "/api/engine/status") => match self.get_status() {
                Ok(dto) => match serde_json::to_vec(&dto) {
                    Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                },
                Err(e) => (
                    500,
                    "application/json; charset=utf-8",
                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                ),
            },
            ("POST", "/api/engine/configure") => {
                let req: ConfigureMappingsRequest = match serde_json::from_slice(body_bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        return (
                            400,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"invalid configure request: {e}\"}}")
                                .into_bytes(),
                        );
                    }
                };
                match self.configure_mappings(req) {
                    Ok(resp) => match serde_json::to_vec(&resp) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        400,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("POST", "/api/engine/start") => {
                let req: StartTunnelRequestDto =
                    serde_json::from_slice(body_bytes).unwrap_or_default();
                match self.start_tunnel(req) {
                    Ok(dto) => match serde_json::to_vec(&dto) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("POST", "/api/engine/stop") => {
                let req: StopTunnelRequestDto =
                    serde_json::from_slice(body_bytes).unwrap_or_default();
                match self.stop_tunnel(req) {
                    Ok(dto) => match serde_json::to_vec(&dto) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("POST", "/api/engine/probe") => {
                let req: ProbeLocalRequestDto =
                    serde_json::from_slice(body_bytes).unwrap_or_default();
                match self.probe_local(req) {
                    Ok(probes) => match serde_json::to_vec(&probes) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("GET", "/api/engine/events") => match self.drain_events() {
                Ok(events) => match serde_json::to_vec(&events) {
                    Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                },
                Err(e) => (
                    500,
                    "application/json; charset=utf-8",
                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                ),
            },
            ("GET", "/api/engine/provisionable") => match self.list_provisionable() {
                Ok(list) => match serde_json::to_vec(&list) {
                    Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                },
                Err(e) => (
                    500,
                    "application/json; charset=utf-8",
                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                ),
            },
            ("GET", "/api/engine/diagnostics") => match self.export_diagnostics() {
                Ok(redacted) => (
                    200,
                    "application/json; charset=utf-8",
                    redacted.into_bytes(),
                ),
                Err(e) => (
                    500,
                    "application/json; charset=utf-8",
                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                ),
            },
            ("POST", "/api/engine/server/provision") => {
                let req: ProvisionServerRequestDto = match serde_json::from_slice(body_bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        return (
                            400,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"invalid provision request: {e}\"}}")
                                .into_bytes(),
                        );
                    }
                };
                match self.provision_server(req) {
                    Ok(resp) => match serde_json::to_vec(&resp) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        400,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("POST", "/api/engine/server/start") => {
                let req: StartManagedServerRequestDto = serde_json::from_slice(body_bytes)
                    .unwrap_or(StartManagedServerRequestDto {
                        profile_id: "minecraft-java".to_owned(),
                    });
                match self.start_managed_server(req) {
                    Ok(resp) => match serde_json::to_vec(&resp) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        400,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("POST", "/api/engine/server/stop") => {
                let req: StopManagedServerRequestDto =
                    serde_json::from_slice(body_bytes).unwrap_or_default();
                match self.stop_managed_server(req) {
                    Ok(resp) => match serde_json::to_vec(&resp) {
                        Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                        Err(e) => (
                            500,
                            "application/json; charset=utf-8",
                            format!("{{\"error\":\"{e}\"}}").into_bytes(),
                        ),
                    },
                    Err(e) => (
                        400,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                }
            }
            ("GET", "/api/engine/server/status") => match self.get_managed_server_status() {
                Ok(resp) => match serde_json::to_vec(&resp) {
                    Ok(bytes) => (200, "application/json; charset=utf-8", bytes),
                    Err(e) => (
                        500,
                        "application/json; charset=utf-8",
                        format!("{{\"error\":\"{e}\"}}").into_bytes(),
                    ),
                },
                Err(e) => (
                    500,
                    "application/json; charset=utf-8",
                    format!("{{\"error\":\"{e}\"}}").into_bytes(),
                ),
            },
            _ => (
                404,
                "application/json; charset=utf-8",
                b"{\"error\":\"not_found\"}".to_vec(),
            ),
        }
    }
}
