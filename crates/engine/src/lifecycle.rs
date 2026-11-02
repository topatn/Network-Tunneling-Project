//! Engine lifecycle coordinator: state machine (`contracts.md` §12), control-plane API and relay
//! handshake integration (`T-070`, `FR-005`, `TEST-002`), public endpoint mapping resolution,
//! players-seen flow-summary consumption (`T-073`, `FR-012`), managed Minecraft: Java Edition
//! server lifecycle and confirmed-presence integration (`T-076`, `SRV-005`, `SRV-006`, `SRV-007`,
//! `SRV-011`), and independent multi-layer UI status events (`contracts.md` §13, `FR-010`,
//! `UX-002`).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use gtt_profiles::Profile;
use gtt_protocol::{
    decode_control_frame, encode_control_frame, ControlMessage, EngineSessionState,
    FlowSummaryMessage, GoodbyeMessage, HelloAckMapping, HelloAckMessage, HelloMessage, Protocol,
    RejectCode, RejectMessage, SequenceMessage, SessionLimits, StartTunnelResponse,
    TokenRefreshAckMessage, TokenRefreshMessage, CURRENT_PROTOCOL_VERSION,
};
use gtt_server_manager::{
    AcquisitionRecord, ArtifactSource, ConfirmedPresenceSnapshot, GeneratedConfig,
    ManagedServerError, ManagedServerState, ManagedServerStatusSnapshot, ManagedServerSupervisor,
    ProvisionableSet, ServerLaunchSpec,
};

use crate::api_client::{
    ApiClientError, BackoffPolicy, ControlPlaneClient, HttpTransport, IdempotencyKey,
    TokenRefresher,
};
use crate::diagnostics::DiagnosticsBundle;
use crate::probe::{probe_local_targets, LocalProbeResult};
use crate::session::EngineTunnelSession;
use crate::tailscale::{
    TailscaleBindMode, TailscaleDetector, TailscaleError, TailscaleNodeShareForwarder,
    TailscaleNodeStatus, TailscaleShareEndpoint,
};
use crate::target_policy::{
    configure_mappings, MappingTargetInput, MappingValidationFailure, ValidatedLocalTargetTable,
};

/// Mandatory UI disclosure label for the players-seen indicator (`FR-012`, `UX-002`,
/// `contracts.md` §10).
///
/// Traffic observed at the relay is never merged with or presented as confirmed in-game presence.
pub const OBSERVED_TRAFFIC_LABEL: &str = "Observed relay traffic (not confirmed in-game presence)";

/// Maximum number of buffered UI events in [`EngineLifecycle`] (`AGENTS.md` §4).
pub const MAX_BUFFERED_ENGINE_EVENTS: usize = 512;

/// Resolved public endpoint per enabled mapping after tunnel start (`FR-005`, `contracts.md` §§5.8,
/// 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMappingEndpoint {
    /// Wire mapping index assigned in `hello_ack` (`contracts.md` §10).
    pub index: u16,
    /// Profile mapping key (`"game"`, `"query"`, etc.).
    pub mapping_key: String,
    /// Transport protocol (`tcp` or `udp`).
    pub protocol: Protocol,
    /// Public relay hostname (`relay.host`).
    pub relay_host: String,
    /// Public port reserved on the relay (`public_port`).
    pub public_port: u16,
}

impl PublicMappingEndpoint {
    /// Format as `host:port` for display and copy-to-clipboard (`FR-005`).
    pub fn display_address(&self) -> String {
        format!("{}:{}", self.relay_host, self.public_port)
    }
}

/// Snapshot of observed guest traffic from relay `flow_summary` messages (`T-073`, `FR-012`,
/// `contracts.md` §§10, 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayersSeenSnapshot {
    /// Count of distinct guest source addresses observed by the relay (`FR-012`).
    pub distinct_sources: u32,
    /// Currently active guest TCP streams on the relay.
    pub tcp_flows: u32,
    /// Currently active guest UDP flows on the relay.
    pub udp_flows: u32,
    /// Mandatory disclosure label stating this metric is observed relay traffic, not confirmed
    /// in-game presence (`FR-012`, `UX-002`).
    pub label: &'static str,
}

impl Default for PlayersSeenSnapshot {
    fn default() -> Self {
        Self {
            distinct_sources: 0,
            tcp_flows: 0,
            udp_flows: 0,
            label: OBSERVED_TRAFFIC_LABEL,
        }
    }
}

/// Complete multi-layer status snapshot returned by [`EngineLifecycle::get_status`]
/// (`contracts.md` §13, `architecture.md` §12, `FR-010`, `UX-002`, `SRV-006`, `SRV-011`).
///
/// The five indicators (`tunnel_state`, `local_probes`, `players_seen`, `managed_server`, and
/// `confirmed_presence`) are stored and updated independently and are never merged into a single
/// status value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatusSnapshot {
    /// Active tunnel UUID, if started.
    pub tunnel_id: Option<String>,
    /// Active allocation UUID, if started.
    pub allocation_id: Option<String>,
    /// Layer 1: Engine tunnel session state (`Idle`, `Connecting`, `Authenticating`, `Active`,
    /// `Degraded`, `Reconnecting`, `Failed`, `Stopped`).
    pub tunnel_state: EngineSessionState,
    /// Public relay address (`relay.host:public_port`) for each enabled mapping (`FR-005`).
    pub public_endpoints: Vec<PublicMappingEndpoint>,
    /// Most recent sampled `ping`/`pong` RTT in milliseconds.
    pub last_rtt_ms: Option<u64>,
    /// Layer 2: Independent local server reachability results per mapping (`FR-011`).
    pub local_probes: Vec<LocalProbeResult>,
    /// Layer 3: Independent observed guest traffic count (`FR-012`).
    pub players_seen: PlayersSeenSnapshot,
    /// Layer 4: Independent managed game server lifecycle status (`SRV-006`).
    pub managed_server: ManagedServerStatusSnapshot,
    /// Layer 5: Opt-in confirmed in-game presence reported by a managed Java server (`SRV-011`).
    pub confirmed_presence: ConfirmedPresenceSnapshot,
}

/// Entry returned by [`EngineLifecycle::list_provisionable`] (`contracts.md` §13, `SRV-003`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionableProfileEntry {
    /// Profile identifier (e.g. `"minecraft-java"`).
    pub profile_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Whether `SRV-003` allows provisioning this profile.
    pub provisionable: bool,
    /// Verified acquisition record if provisionable, or actionable exclusion reason if not.
    pub detail: String,
}

/// Events pushed from the engine to the desktop UI (`contracts.md` §13, `T-070`, `T-073`, `T-076`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// `tunnel_status_changed{state, reason}` (`contracts.md` §12, §13).
    TunnelStatusChanged {
        /// New engine session state.
        state: EngineSessionState,
        /// Diagnostic transition reason (no secrets or payload bytes, `PRIV-002`).
        reason: String,
    },
    /// `local_probe_result{mapping_key, level, ok, detail}` (`FR-011`, `T-072`).
    LocalProbeResult(LocalProbeResult),
    /// `players_seen{tcp_flows, udp_flows, distinct_sources, label}` (`FR-012`, `T-073`).
    PlayersSeen(PlayersSeenSnapshot),
    /// `managed_server_status_changed{state, progress}` (`SRV-006`, `T-076`, `contracts.md` §13).
    ManagedServerStatusChanged {
        /// New managed server state.
        state: ManagedServerState,
        /// Optional progress percentage (`0..=100`).
        progress: Option<u8>,
    },
    /// `managed_server_log{excerpt}` (`T-076`, `contracts.md` §13).
    ManagedServerLog {
        /// Bounded log excerpt from the managed server.
        excerpt: String,
    },
    /// `managed_server_update{state}` (`SRV-008`, `SRV-009`, `T-076`, `contracts.md` §13).
    ManagedServerUpdate {
        /// Update lifecycle stage (`"verifying"`, `"applying"`, `"completed"`, `"rolled_back"`, `"failed"`).
        state: String,
    },
    /// `confirmed_presence{confirmed_players, label}` (`SRV-011`, `T-076`, `research.md` RQ-13.4).
    ///
    /// Strictly separate from [`EngineEvent::PlayersSeen`]: never merges MSMP join/leave events
    /// into the relay flow-based traffic count.
    ConfirmedPresence(ConfirmedPresenceSnapshot),
    /// `engine_error{code, message}` (`FR-015`, `UX-003`).
    EngineError {
        /// Lowercase machine-readable error code.
        code: String,
        /// Human-readable diagnostic message.
        message: String,
    },
}

/// Connectivity tier and transport mode for the current engine session (`ADR-022`,
/// `context.md` §§1, 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionConnectivityMode {
    /// Paid tier: tunnel traffic through a GTT relay (`ADR-007` G2). Guests connect to the
    /// relay-allocated `relay.host:public_port` address with zero client setup.
    GttRelay,
    /// Free tier: expose enabled profile mappings on the host's local Tailscale Tailnet interface
    /// (`100.64.0.0/10`) for Tailscale Node Sharing (`ADR-022`). Runs purely locally in
    /// `gtt-engine` without requiring a `gtt-control-plane` allocation.
    TailscaleNodeShare,
}

/// Lifecycle controller coordinating local target configuration (`T-071`), reachability probes
/// (`T-072`), control-plane start/stop/refresh calls (`T-047`), relay QUIC control stream messages
/// (`T-060`, `T-063`), Tailscale Node Share local forwarding (`ADR-022`), managed Minecraft: Java
/// Edition server supervision (`T-076`), and UI event emission (`T-070`, `T-073`, `T-076`).
#[derive(Debug)]
pub struct EngineLifecycle {
    client_version: String,
    target_table: ValidatedLocalTargetTable,
    session: EngineTunnelSession,
    connectivity_mode: SessionConnectivityMode,
    tunnel_id: Option<String>,
    allocation_id: Option<String>,
    relay_host: Option<String>,
    relay_tunnel_port: Option<u16>,
    allocation_token: Option<String>,
    public_endpoints: Vec<PublicMappingEndpoint>,
    local_probes: Vec<LocalProbeResult>,
    players_seen: PlayersSeenSnapshot,
    managed_server_status: ManagedServerStatusSnapshot,
    confirmed_presence: ConfirmedPresenceSnapshot,
    server_supervisor: Option<ManagedServerSupervisor>,
    tailscale_status: Option<TailscaleNodeStatus>,
    tailscale_forwarder: Option<TailscaleNodeShareForwarder>,
    events: VecDeque<EngineEvent>,
}

impl EngineLifecycle {
    /// Configure local mappings against `profile` (`contracts.md` §13 `configure_mappings`) and
    /// initialize an [`EngineLifecycle`] in [`EngineSessionState::Idle`].
    pub fn configure(
        profile: &Profile,
        inputs: &[MappingTargetInput],
        client_version: impl Into<String>,
    ) -> Result<Self, Vec<MappingValidationFailure>> {
        let target_table = configure_mappings(profile, inputs)?;
        let local_configs = target_table.to_local_mapping_configs();
        let session = EngineTunnelSession::new(local_configs, 64, 64).map_err(|reason| {
            vec![MappingValidationFailure {
                mapping_key: String::new(),
                violation: crate::target_policy::TargetPolicyViolation::SessionSetupRejected {
                    reason,
                },
            }]
        })?;

        Ok(Self {
            client_version: client_version.into(),
            target_table,
            session,
            connectivity_mode: SessionConnectivityMode::GttRelay,
            tunnel_id: None,
            allocation_id: None,
            relay_host: None,
            relay_tunnel_port: None,
            allocation_token: None,
            public_endpoints: Vec::new(),
            local_probes: Vec::new(),
            players_seen: PlayersSeenSnapshot::default(),
            managed_server_status: ManagedServerStatusSnapshot {
                profile_id: profile.id.clone(),
                ..ManagedServerStatusSnapshot::default()
            },
            confirmed_presence: ConfirmedPresenceSnapshot::default(),
            server_supervisor: None,
            tailscale_status: None,
            tailscale_forwarder: None,
            events: VecDeque::new(),
        })
    }

    /// Current connectivity mode (`GttRelay` vs `TailscaleNodeShare`, `ADR-022`).
    pub fn connectivity_mode(&self) -> SessionConnectivityMode {
        self.connectivity_mode
    }

    /// Latest detected local Tailscale node status, if queried (`ADR-022`).
    pub fn tailscale_status(&self) -> Option<&TailscaleNodeStatus> {
        self.tailscale_status.as_ref()
    }

    /// Active Tailscale Node Share endpoints, if started in `TailscaleNodeShare` mode (`ADR-022`).
    pub fn tailscale_share_endpoints(&self) -> &[TailscaleShareEndpoint] {
        self.tailscale_forwarder
            .as_ref()
            .map(|f| f.endpoints())
            .unwrap_or(&[])
    }

    /// Attach a host-local [`ManagedServerSupervisor`] to this engine lifecycle (`T-076`,
    /// `SRV-005`, `SRV-006`, `SRV-007`).
    pub fn attach_server_supervisor(&mut self, supervisor: ManagedServerSupervisor) {
        self.managed_server_status = supervisor.status().clone();
        self.confirmed_presence = supervisor.confirmed_presence().clone();
        self.server_supervisor = Some(supervisor);
    }

    /// Borrow the attached [`ManagedServerSupervisor`], if any.
    pub fn server_supervisor(&self) -> Option<&ManagedServerSupervisor> {
        self.server_supervisor.as_ref()
    }

    /// Borrow the validated local target table (`T-071`).
    pub fn target_table(&self) -> &ValidatedLocalTargetTable {
        &self.target_table
    }

    /// Borrow the underlying [`EngineTunnelSession`].
    pub fn session(&self) -> &EngineTunnelSession {
        &self.session
    }

    /// Mutably borrow the underlying [`EngineTunnelSession`].
    pub fn session_mut(&mut self) -> &mut EngineTunnelSession {
        &mut self.session
    }

    /// Current engine session state (`contracts.md` §12).
    pub fn state(&self) -> EngineSessionState {
        self.session.state()
    }

    /// Resolved public endpoints (`relay.host:public_port`) for each enabled mapping (`FR-005`).
    pub fn public_endpoints(&self) -> &[PublicMappingEndpoint] {
        &self.public_endpoints
    }

    /// Latest players-seen snapshot (`FR-012`, `T-073`).
    pub fn players_seen(&self) -> &PlayersSeenSnapshot {
        &self.players_seen
    }

    /// Latest local reachability probe results (`FR-011`, `T-072`).
    pub fn local_probes(&self) -> &[LocalProbeResult] {
        &self.local_probes
    }

    /// Latest fourth-layer managed server status snapshot (`SRV-006`, `T-076`, `contracts.md` §13
    /// `get_managed_server_status`).
    pub fn get_managed_server_status(&self) -> &ManagedServerStatusSnapshot {
        &self.managed_server_status
    }

    /// Latest fifth-layer confirmed in-game presence snapshot (`SRV-011`, `T-076`).
    pub fn confirmed_presence(&self) -> &ConfirmedPresenceSnapshot {
        &self.confirmed_presence
    }

    /// Return the current five-source status snapshot (`contracts.md` §13 `get_status`,
    /// `architecture.md` §12, `FR-010`, `UX-002`, `SRV-006`, `SRV-011`).
    pub fn get_status(&self) -> EngineStatusSnapshot {
        let players_seen = match &self.tailscale_forwarder {
            Some(fwd) => fwd.players_seen_snapshot(),
            None => self.players_seen.clone(),
        };
        EngineStatusSnapshot {
            tunnel_id: self.tunnel_id.clone(),
            allocation_id: self.allocation_id.clone(),
            tunnel_state: self.session.state(),
            public_endpoints: self.public_endpoints.clone(),
            last_rtt_ms: self.session.last_rtt_ms(),
            local_probes: self.local_probes.clone(),
            players_seen,
            managed_server: self.managed_server_status.clone(),
            confirmed_presence: self.confirmed_presence.clone(),
        }
    }

    /// Drain all pending [`EngineEvent`] items in FIFO order.
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        self.events.drain(..).collect()
    }

    /// Detect the host's local Tailscale node status (`tailscale status --json`, `ADR-022`,
    /// `contracts.md` §13 `detect_tailscale`).
    pub fn detect_tailscale<D: TailscaleDetector>(
        &mut self,
        detector: &D,
    ) -> Result<TailscaleNodeStatus, TailscaleError> {
        match detector.query_status() {
            Ok(status) => {
                self.tailscale_status = Some(status.clone());
                Ok(status)
            }
            Err(err) => {
                self.push_event(EngineEvent::EngineError {
                    code: err.code().to_owned(),
                    message: err.to_string(),
                });
                Err(err)
            }
        }
    }

    /// Start a Free-tier Tailscale Node Share session on the host's `100.64.0.0/10` Tailscale
    /// interface (`ADR-022`, `contracts.md` §13 `start_tailscale_share`, `FR-005`, `SEC-006`).
    ///
    /// Runs purely locally in `gtt-engine` without calling `gtt-control-plane`. Keeps the managed
    /// or local Minecraft: Java Edition server on `127.0.0.1:25565` (`SRV-005`) and binds TCP
    /// forwarders only for enabled profile mappings (`GAME-002`, `GAME-008`).
    pub fn start_via_tailscale<D: TailscaleDetector>(
        &mut self,
        detector: &D,
    ) -> Result<Vec<TailscaleShareEndpoint>, TailscaleError> {
        self.start_via_tailscale_with_bind_mode(detector, TailscaleBindMode::TailscaleInterface)
    }

    /// Start a Free-tier Tailscale Node Share session with an explicit [`TailscaleBindMode`]
    /// (`ADR-022`).
    pub fn start_via_tailscale_with_bind_mode<D: TailscaleDetector>(
        &mut self,
        detector: &D,
        bind_mode: TailscaleBindMode,
    ) -> Result<Vec<TailscaleShareEndpoint>, TailscaleError> {
        let status = self.detect_tailscale(detector)?;
        if let Err(err) = status.require_ready_for_node_share() {
            self.push_event(EngineEvent::EngineError {
                code: err.code().to_owned(),
                message: err.to_string(),
            });
            return Err(err);
        }

        let forwarder =
            match TailscaleNodeShareForwarder::start(&self.target_table, &status, bind_mode, 64) {
                Ok(fwd) => fwd,
                Err(err) => {
                    self.push_event(EngineEvent::EngineError {
                        code: err.code().to_owned(),
                        message: err.to_string(),
                    });
                    return Err(err);
                }
            };

        let connecting = self
            .session
            .start()
            .map_err(|e| TailscaleError::InvalidSessionState {
                reason: e.to_string(),
            })?;
        self.push_event(EngineEvent::TunnelStatusChanged {
            state: connecting,
            reason: "tailscale_node_share_starting".to_owned(),
        });

        let _ = self.session.on_quic_established().map_err(|e| {
            TailscaleError::InvalidSessionState {
                reason: e.to_string(),
            }
        })?;

        let endpoints = forwarder.endpoints().to_vec();
        let ack_mappings: Vec<HelloAckMapping> = endpoints
            .iter()
            .map(|ep| HelloAckMapping {
                index: ep.index,
                mapping_key: ep.mapping_key.clone(),
                protocol: ep.protocol,
                public_port: ep.public_port,
            })
            .collect();

        self.target_table.bind_hello_ack_mappings(&ack_mappings);

        let active = self
            .session
            .on_hello_ack(
                HelloAckMessage {
                    session_id: "00000000-0000-4000-8000-000000000000".to_owned(),
                    heartbeat_interval_ms: 5_000,
                    idle_timeout_ms: 15_000,
                    limits: SessionLimits {
                        max_tcp_flows: 64,
                        max_udp_flows: 64,
                    },
                    mappings: ack_mappings,
                },
                0,
            )
            .map_err(|e| TailscaleError::InvalidSessionState {
                reason: e.to_string(),
            })?;

        self.connectivity_mode = SessionConnectivityMode::TailscaleNodeShare;
        self.tunnel_id = None;
        self.allocation_id = None;
        self.relay_host = status.tailscale_ipv4.map(|ip| ip.to_string());
        self.relay_tunnel_port = None;
        self.allocation_token = None;
        self.public_endpoints = endpoints
            .iter()
            .map(TailscaleShareEndpoint::to_public_mapping_endpoint)
            .collect();
        self.tailscale_forwarder = Some(forwarder);

        self.push_event(EngineEvent::TunnelStatusChanged {
            state: active,
            reason: "tailscale_node_share_active".to_owned(),
        });

        Ok(endpoints)
    }

    /// Poll the active Tailscale Node Share forwarder for observed peer traffic and emit
    /// [`EngineEvent::PlayersSeen`] when counters change (`ADR-022`, `FR-012`, `UX-002`).
    pub fn poll_tailscale_players_seen(&mut self) -> PlayersSeenSnapshot {
        let Some(fwd) = self.tailscale_forwarder.as_ref() else {
            return self.players_seen.clone();
        };
        let next = fwd.players_seen_snapshot();
        if next != self.players_seen {
            self.players_seen = next.clone();
            self.push_event(EngineEvent::PlayersSeen(next.clone()));
        }
        next
    }

    /// Call `POST /v1/tunnels/{id}/start` via [`ControlPlaneClient::start_tunnel`] and transition
    /// `Idle -> Connecting` (`T-070`, `FR-005`).
    pub fn start_via_api<T, B, R>(
        &mut self,
        api_client: &mut ControlPlaneClient<T, B, R>,
        tunnel_id: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Result<StartTunnelResponse, ApiClientError>
    where
        T: HttpTransport,
        B: BackoffPolicy,
        R: TokenRefresher,
    {
        match api_client.start_tunnel(tunnel_id, idempotency_key) {
            Ok(start_resp) => {
                if let Err(err) = self.begin_start_with_response(&start_resp) {
                    self.push_event(EngineEvent::EngineError {
                        code: "invalid_state_transition".to_owned(),
                        message: err,
                    });
                }
                Ok(start_resp)
            }
            Err(api_err) => {
                self.push_event(EngineEvent::EngineError {
                    code: "start_tunnel_failed".to_owned(),
                    message: api_err.to_string(),
                });
                Err(api_err)
            }
        }
    }

    /// Begin a tunnel session from a [`StartTunnelResponse`] (`Idle -> Connecting`,
    /// `contracts.md` §§12, 13 `start_tunnel`).
    ///
    /// Records the relay endpoint and initial public port assignments (`FR-005`).
    pub fn begin_start_with_response(
        &mut self,
        start_response: &StartTunnelResponse,
    ) -> Result<EngineSessionState, String> {
        let next = self
            .session
            .start()
            .map_err(|e| format!("failed to start tunnel session: {e}"))?;

        if let Some(mut fwd) = self.tailscale_forwarder.take() {
            fwd.stop();
        }
        self.connectivity_mode = SessionConnectivityMode::GttRelay;
        self.tunnel_id = Some(start_response.tunnel_id.clone());
        self.allocation_id = Some(start_response.allocation.id.clone());
        self.relay_host = Some(start_response.allocation.relay.host.clone());
        self.relay_tunnel_port = Some(start_response.allocation.relay.tunnel_port);
        self.allocation_token = Some(start_response.allocation_token.clone());

        // Populate initial public endpoints from the control-plane allocation (`FR-005`).
        self.public_endpoints = start_response
            .allocation
            .ports
            .iter()
            .enumerate()
            .map(|(idx, p)| PublicMappingEndpoint {
                index: u16::try_from(idx).unwrap_or(0),
                mapping_key: p.mapping_key.clone(),
                protocol: p.protocol,
                relay_host: start_response.allocation.relay.host.clone(),
                public_port: p.public_port,
            })
            .collect();

        self.push_event(EngineEvent::TunnelStatusChanged {
            state: next,
            reason: "start_with_token".to_owned(),
        });
        Ok(next)
    }

    /// Record that the outbound QUIC connection to the relay completed TLS handshake
    /// (`Connecting -> Authenticating`) and build the length-prefixed `hello` control frame to
    /// send on the control stream (`contracts.md` §§10, 12).
    pub fn on_quic_connected(&mut self) -> Result<Vec<u8>, String> {
        if self.session.state() == EngineSessionState::Connecting {
            let next = self
                .session
                .on_quic_established()
                .map_err(|e| format!("invalid transition on QUIC established: {e}"))?;
            self.push_event(EngineEvent::TunnelStatusChanged {
                state: next,
                reason: "quic_established".to_owned(),
            });
        } else if self.session.state() != EngineSessionState::Reconnecting {
            return Err(format!(
                "cannot send hello from state {:?}",
                self.session.state()
            ));
        }

        let token = self
            .allocation_token
            .as_ref()
            .ok_or_else(|| "missing allocation_token for hello".to_owned())?
            .clone();

        let hello = ControlMessage::Hello(HelloMessage {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            token,
            client_version: self.client_version.clone(),
        });

        encode_control_frame(&hello).map_err(|e| format!("failed to encode hello frame: {e}"))
    }

    /// Process a length-prefixed control frame received from the relay on the control stream
    /// (`contracts.md` §§10, 12, 13, `T-070`, `T-073`).
    ///
    /// Returns an optional length-prefixed response frame (such as `pong` when the relay sends
    /// `ping`).
    pub fn handle_relay_control_frame(
        &mut self,
        raw_frame: &[u8],
        now_ms: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        let (msg, consumed) = decode_control_frame(raw_frame)
            .map_err(|e| format!("invalid control frame from relay: {e}"))?;
        if consumed != raw_frame.len() {
            return Err("trailing bytes in relay control frame".to_owned());
        }
        self.handle_relay_control_message(msg, now_ms)
    }

    /// Process a decoded [`ControlMessage`] from the relay (`T-070`, `T-073`).
    pub fn handle_relay_control_message(
        &mut self,
        msg: ControlMessage,
        now_ms: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        match msg {
            ControlMessage::HelloAck(ack) => {
                self.apply_hello_ack(ack, now_ms)?;
                Ok(None)
            }
            ControlMessage::Reject(reject) => {
                self.apply_reject(reject)?;
                Ok(None)
            }
            ControlMessage::Ping(ping) => {
                let pong = ControlMessage::Pong(SequenceMessage { seq: ping.seq });
                let encoded = encode_control_frame(&pong)
                    .map_err(|e| format!("failed to encode pong: {e}"))?;
                Ok(Some(encoded))
            }
            ControlMessage::Pong(pong) => {
                let prev = self.session.state();
                let _ = self.session.on_pong(pong, now_ms);
                let next = self.session.state();
                if next != prev {
                    let reason = if next == EngineSessionState::Degraded {
                        "high_rtt"
                    } else {
                        "rtt_recovered"
                    };
                    self.push_event(EngineEvent::TunnelStatusChanged {
                        state: next,
                        reason: reason.to_owned(),
                    });
                }
                Ok(None)
            }
            ControlMessage::TokenRefreshAck(TokenRefreshAckMessage { .. }) => Ok(None),
            ControlMessage::FlowSummary(summary) => {
                self.apply_flow_summary(summary);
                Ok(None)
            }
            ControlMessage::Goodbye(GoodbyeMessage { reason }) => {
                if matches!(
                    self.session.state(),
                    EngineSessionState::Active | EngineSessionState::Degraded
                ) {
                    let next = self
                        .session
                        .on_connection_lost()
                        .map_err(|e| e.to_string())?;
                    self.push_event(EngineEvent::TunnelStatusChanged {
                        state: next,
                        reason: format!("relay_goodbye:{reason}"),
                    });
                }
                Ok(None)
            }
            ControlMessage::Hello(_) | ControlMessage::TokenRefresh(_) => {
                Err("unexpected client-to-relay control message received from relay".to_owned())
            }
        }
    }

    /// Refresh the allocation token via `POST /v1/tunnels/{id}/token` and encode a `token_refresh`
    /// control frame (`contracts.md` §§5.9, 10).
    pub fn refresh_token_via_api<T, B, R>(
        &mut self,
        api_client: &mut ControlPlaneClient<T, B, R>,
    ) -> Result<Vec<u8>, String>
    where
        T: HttpTransport,
        B: BackoffPolicy,
        R: TokenRefresher,
    {
        let tid = self
            .tunnel_id
            .as_deref()
            .ok_or_else(|| "tunnel_id is not set".to_owned())?
            .to_owned();
        let token_resp = api_client
            .refresh_allocation_token(&tid)
            .map_err(|e| format!("failed to refresh allocation token: {e}"))?;
        self.allocation_token = Some(token_resp.allocation_token.clone());

        encode_control_frame(&ControlMessage::TokenRefresh(TokenRefreshMessage {
            token: token_resp.allocation_token,
        }))
        .map_err(|e| format!("failed to encode token_refresh: {e}"))
    }

    /// Evaluate heartbeat/idle timeouts at `now_ms` (`NET-005`, `contracts.md` §12).
    pub fn tick_liveness(&mut self, now_ms: u64) -> EngineSessionState {
        let prev = self.session.state();
        let next = self.session.evaluate_liveness(now_ms);
        if next != prev {
            let reason = match next {
                EngineSessionState::Degraded => "heartbeat_missed",
                EngineSessionState::Reconnecting => "idle_timeout",
                _ => "liveness_tick",
            };
            self.push_event(EngineEvent::TunnelStatusChanged {
                state: next,
                reason: reason.to_owned(),
            });
        }
        next
    }

    /// Notify the engine that the QUIC connection dropped (`Active | Degraded -> Reconnecting`,
    /// `contracts.md` §12).
    pub fn on_connection_lost(&mut self, reason: &str) -> Result<EngineSessionState, String> {
        let next = self
            .session
            .on_connection_lost()
            .map_err(|e| format!("invalid transition on connection lost: {e}"))?;
        self.push_event(EngineEvent::TunnelStatusChanged {
            state: next,
            reason: reason.to_owned(),
        });
        Ok(next)
    }

    /// Record a failed transport reconnection attempt while in `Reconnecting`. Transitions to
    /// `Failed` once retries are exhausted (`contracts.md` §12).
    pub fn on_reconnect_attempt_failed(&mut self, detail: &str) -> EngineSessionState {
        let prev = self.session.state();
        let next = self.session.record_reconnect_failure();
        if next != prev && next == EngineSessionState::Failed {
            self.push_event(EngineEvent::TunnelStatusChanged {
                state: next,
                reason: "retries_exhausted".to_owned(),
            });
            self.push_event(EngineEvent::EngineError {
                code: "reconnect_retries_exhausted".to_owned(),
                message: detail.to_owned(),
            });
        }
        next
    }

    /// Run local reachability probes (`contracts.md` §13 `probe_local`, `T-072`, `FR-011`).
    ///
    /// Updates the local-server reachability indicator and emits [`EngineEvent::LocalProbeResult`]
    /// without altering `tunnel_state` or `players_seen` (`FR-010`, `UX-002`).
    pub fn probe_local(
        &mut self,
        mapping_key: Option<&str>,
        timeout: Duration,
    ) -> Vec<LocalProbeResult> {
        let results = probe_local_targets(&self.target_table, mapping_key, timeout);
        for res in &results {
            if let Some(existing) = self
                .local_probes
                .iter_mut()
                .find(|p| p.mapping_key == res.mapping_key)
            {
                *existing = res.clone();
            } else {
                self.local_probes.push(res.clone());
            }
            self.push_event(EngineEvent::LocalProbeResult(res.clone()));
        }
        results
    }

    /// User-requested tunnel stop (`any -> Stopped`, `contracts.md` §§12, 13 `stop_tunnel`,
    /// `FR-006`, `NET-008`, `SRV-007`).
    ///
    /// Closes all active TCP and UDP flows immediately, stops any running managed game server
    /// (`SRV-007`: stop the server when the tunnel stops), and returns the encoded `goodbye`
    /// frame to send to the relay.
    pub fn stop(&mut self, reason: &str) -> Result<Vec<u8>, String> {
        if let Some(mut fwd) = self.tailscale_forwarder.take() {
            fwd.stop();
        }

        if let Some(sup) = self.server_supervisor.as_ref() {
            if matches!(
                sup.status().state,
                ManagedServerState::Starting | ManagedServerState::Running
            ) {
                let _ = self.stop_managed_server(Duration::from_secs(5));
            }
        }

        let next = self
            .session
            .stop()
            .map_err(|e| format!("cannot transition to Stopped: {e}"))?;

        self.players_seen = PlayersSeenSnapshot::default();
        self.push_event(EngineEvent::TunnelStatusChanged {
            state: next,
            reason: reason.to_owned(),
        });

        encode_control_frame(&ControlMessage::Goodbye(GoodbyeMessage {
            reason: reason.to_owned(),
        }))
        .map_err(|e| format!("failed to encode goodbye frame: {e}"))
    }

    /// Stop the tunnel locally and notify the control plane via `POST /v1/tunnels/{id}/stop`
    /// (`FR-006`, `T-070`, `SRV-007`).
    pub fn stop_via_api<T, B, R>(
        &mut self,
        api_client: &mut ControlPlaneClient<T, B, R>,
        idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<Vec<u8>, String>
    where
        T: HttpTransport,
        B: BackoffPolicy,
        R: TokenRefresher,
    {
        let goodbye = self.stop("user_stop")?;
        if let Some(tid) = self.tunnel_id.clone() {
            if let Err(err) = api_client.stop_tunnel(&tid, idempotency_key) {
                self.push_event(EngineEvent::EngineError {
                    code: "stop_tunnel_api_failed".to_owned(),
                    message: err.to_string(),
                });
                return Err(err.to_string());
            }
        }
        Ok(goodbye)
    }

    /// List which profiles `SRV-003` allows provisioning for, and why others are excluded
    /// (`contracts.md` §13 `list_provisionable`, `SRV-003`).
    pub fn list_provisionable(
        &self,
        profiles: &[Profile],
        override_set: Option<&ProvisionableSet>,
    ) -> Vec<ProvisionableProfileEntry> {
        let default_empty = ProvisionableSet::none();
        let set = override_set
            .or_else(|| {
                self.server_supervisor
                    .as_ref()
                    .map(|s| s.provisionable_set())
            })
            .unwrap_or(&default_empty);

        profiles
            .iter()
            .map(|p| match set.require(p) {
                Ok(record) => ProvisionableProfileEntry {
                    profile_id: p.id.clone(),
                    display_name: p.display_name.clone(),
                    provisionable: true,
                    detail: record.to_string(),
                },
                Err(err) => ProvisionableProfileEntry {
                    profile_id: p.id.clone(),
                    display_name: p.display_name.clone(),
                    provisionable: false,
                    detail: err.to_string(),
                },
            })
            .collect()
    }

    /// Provision a Minecraft: Java Edition server (`contracts.md` §13 `provision_server`,
    /// `T-076`, `SRV-001`..`SRV-006`, `SRV-010`, `SRV-015`).
    ///
    /// Keeps all local server addresses, ports, and paths strictly in-process (`SRV-005`, `INV-1`),
    /// emits `ManagedServerStatusChanged` (`Verifying` -> `Configuring` or `Failed`) and
    /// `ManagedServerLog`, and never modifies `tunnel_state`, `local_probes`, or `players_seen`.
    pub fn provision_server(
        &mut self,
        profile: &Profile,
        artifact: &ArtifactSource,
        version: &str,
        max_players: u16,
    ) -> Result<(AcquisitionRecord, GeneratedConfig), ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            let err = ManagedServerError::ConfigurationInvalid {
                reason: "no ManagedServerSupervisor attached to EngineLifecycle".to_owned(),
            };
            self.emit_managed_server_error(&err);
            return Err(err);
        };

        self.push_event(EngineEvent::ManagedServerStatusChanged {
            state: ManagedServerState::Verifying,
            progress: Some(25),
        });

        let result = sup.provision_minecraft_java(profile, artifact, version, max_players);
        self.managed_server_status = sup.status().clone();
        let excerpt = self.managed_server_status.last_log_excerpt.clone();
        self.server_supervisor = Some(sup);

        match result {
            Ok(pair) => {
                self.push_event(EngineEvent::ManagedServerStatusChanged {
                    state: self.managed_server_status.state,
                    progress: Some(100),
                });
                if !excerpt.is_empty() {
                    self.push_event(EngineEvent::ManagedServerLog { excerpt });
                }
                Ok(pair)
            }
            Err(err) => {
                self.push_event(EngineEvent::ManagedServerStatusChanged {
                    state: self.managed_server_status.state,
                    progress: None,
                });
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Start a provisioned Minecraft: Java Edition server (`contracts.md` §13
    /// `start_managed_server`, `T-076`, `SRV-006`, `SRV-007`, `SRV-015`, `SRV-018`).
    pub fn start_managed_server(
        &mut self,
        profile_id: &str,
        launch_spec: ServerLaunchSpec,
    ) -> Result<ManagedServerState, ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            let err = ManagedServerError::ConfigurationInvalid {
                reason: "no ManagedServerSupervisor attached to EngineLifecycle".to_owned(),
            };
            self.emit_managed_server_error(&err);
            return Err(err);
        };

        let res = sup.start_server(profile_id, launch_spec);
        self.managed_server_status = sup.status().clone();
        self.confirmed_presence = sup.confirmed_presence().clone();
        self.server_supervisor = Some(sup);

        match res {
            Ok(state) => {
                self.push_event(EngineEvent::ManagedServerStatusChanged {
                    state,
                    progress: None,
                });
                Ok(state)
            }
            Err(err) => {
                self.push_event(EngineEvent::ManagedServerStatusChanged {
                    state: self.managed_server_status.state,
                    progress: None,
                });
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Poll the running managed server process for readiness (`Done (`), MSMP confirmed-presence
    /// notifications (`SRV-011`), refused-before-ready exits (`SRV-007` (a)), and bounded crash
    /// restarts (`SRV-007`).
    pub fn poll_managed_server(&mut self) -> Result<ManagedServerState, ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            return Ok(self.managed_server_status.state);
        };

        let prev_state = self.managed_server_status.state;
        let prev_excerpt = self.managed_server_status.last_log_excerpt.clone();
        let prev_presence = self.confirmed_presence.clone();

        let res = sup.poll_process();
        let next_status = sup.status().clone();
        let next_presence = sup.confirmed_presence().clone();
        self.managed_server_status = next_status.clone();
        self.confirmed_presence = next_presence.clone();
        self.server_supervisor = Some(sup);

        if next_status.last_log_excerpt != prev_excerpt && !next_status.last_log_excerpt.is_empty()
        {
            self.push_event(EngineEvent::ManagedServerLog {
                excerpt: next_status.last_log_excerpt.clone(),
            });
        }

        if next_presence != prev_presence {
            self.push_event(EngineEvent::ConfirmedPresence(next_presence));
        }

        match res {
            Ok(state) => {
                if state != prev_state {
                    self.push_event(EngineEvent::ManagedServerStatusChanged {
                        state,
                        progress: None,
                    });
                }
                Ok(state)
            }
            Err(err) => {
                if self.managed_server_status.state != prev_state {
                    self.push_event(EngineEvent::ManagedServerStatusChanged {
                        state: self.managed_server_status.state,
                        progress: None,
                    });
                }
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Ingest a raw log or MSMP JSON-RPC 2.0 notification line into the managed server supervisor
    /// (`SRV-007`, `SRV-011`, `T-076`).
    ///
    /// Emits `ManagedServerLog`, `ManagedServerStatusChanged` (when `Done (` transitions
    /// `Starting -> Running`), and `ConfirmedPresence` (when MSMP `players/joined` or
    /// `players/left` changes the confirmed player count) without touching `players_seen`,
    /// `local_probes`, or `tunnel_state`.
    pub fn ingest_managed_server_line(&mut self, line: &str) {
        let Some(mut sup) = self.server_supervisor.take() else {
            return;
        };

        let prev_state = self.managed_server_status.state;
        let prev_presence = self.confirmed_presence.clone();

        sup.ingest_log_line(line);

        let next_status = sup.status().clone();
        let next_presence = sup.confirmed_presence().clone();
        self.managed_server_status = next_status.clone();
        self.confirmed_presence = next_presence.clone();
        self.server_supervisor = Some(sup);

        if next_status.state != prev_state {
            self.push_event(EngineEvent::ManagedServerStatusChanged {
                state: next_status.state,
                progress: None,
            });
        }
        if !next_status.last_log_excerpt.is_empty() {
            self.push_event(EngineEvent::ManagedServerLog {
                excerpt: next_status.last_log_excerpt,
            });
        }
        if next_presence != prev_presence {
            self.push_event(EngineEvent::ConfirmedPresence(next_presence));
        }
    }

    /// Gracefully stop the managed server via `stop\n` on its held stdin pipe (`contracts.md` §13
    /// `stop_managed_server`, `SRV-007`).
    pub fn stop_managed_server(
        &mut self,
        timeout: Duration,
    ) -> Result<ManagedServerState, ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            return Ok(self.managed_server_status.state);
        };

        let prev_state = self.managed_server_status.state;
        let prev_excerpt = self.managed_server_status.last_log_excerpt.clone();
        let prev_presence = self.confirmed_presence.clone();

        let res = sup.stop_server(timeout);
        let next_status = sup.status().clone();
        let next_presence = sup.confirmed_presence().clone();
        self.managed_server_status = next_status.clone();
        self.confirmed_presence = next_presence.clone();
        self.server_supervisor = Some(sup);

        if next_status.last_log_excerpt != prev_excerpt && !next_status.last_log_excerpt.is_empty()
        {
            self.push_event(EngineEvent::ManagedServerLog {
                excerpt: next_status.last_log_excerpt,
            });
        }
        if next_presence != prev_presence {
            self.push_event(EngineEvent::ConfirmedPresence(next_presence));
        }

        match res {
            Ok(()) => {
                if self.managed_server_status.state != prev_state {
                    self.push_event(EngineEvent::ManagedServerStatusChanged {
                        state: self.managed_server_status.state,
                        progress: None,
                    });
                }
                Ok(self.managed_server_status.state)
            }
            Err(err) => {
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Back up the managed server's world directory (`contracts.md` §13 `backup_world`, `SRV-008`).
    pub fn backup_world(
        &mut self,
        profile_id: &str,
        backup_id: &str,
    ) -> Result<PathBuf, ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            let err = ManagedServerError::ConfigurationInvalid {
                reason: "no ManagedServerSupervisor attached to EngineLifecycle".to_owned(),
            };
            self.emit_managed_server_error(&err);
            return Err(err);
        };

        let res = sup.backup_world(profile_id, backup_id);
        self.managed_server_status = sup.status().clone();
        self.server_supervisor = Some(sup);

        match res {
            Ok(path) => {
                self.push_event(EngineEvent::ManagedServerLog {
                    excerpt: self.managed_server_status.last_log_excerpt.clone(),
                });
                Ok(path)
            }
            Err(err) => {
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Restore a world backup (`contracts.md` §13 `restore_world`, `SRV-008`).
    pub fn restore_world(
        &mut self,
        profile_id: &str,
        backup_id: &str,
    ) -> Result<PathBuf, ManagedServerError> {
        let Some(mut sup) = self.server_supervisor.take() else {
            let err = ManagedServerError::ConfigurationInvalid {
                reason: "no ManagedServerSupervisor attached to EngineLifecycle".to_owned(),
            };
            self.emit_managed_server_error(&err);
            return Err(err);
        };

        let res = sup.restore_world(profile_id, backup_id);
        self.managed_server_status = sup.status().clone();
        self.server_supervisor = Some(sup);

        match res {
            Ok(path) => {
                self.push_event(EngineEvent::ManagedServerLog {
                    excerpt: self.managed_server_status.last_log_excerpt.clone(),
                });
                Ok(path)
            }
            Err(err) => {
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    /// Update the managed server artifact (`contracts.md` §13 `update_managed_server`, `SRV-008`,
    /// `SRV-009`).
    ///
    /// Refuses without confirmation while the tunnel session is active (`Connecting`,
    /// `Authenticating`, `Active`, `Degraded`, or `Reconnecting`).
    pub fn update_managed_server(
        &mut self,
        profile_id: &str,
        new_artifact: &ArtifactSource,
        new_version: &str,
        confirm_disconnect: bool,
    ) -> Result<(), ManagedServerError> {
        let tunnel_is_active = matches!(
            self.session.state(),
            EngineSessionState::Connecting
                | EngineSessionState::Authenticating
                | EngineSessionState::Active
                | EngineSessionState::Degraded
                | EngineSessionState::Reconnecting
        );

        let Some(mut sup) = self.server_supervisor.take() else {
            let err = ManagedServerError::ConfigurationInvalid {
                reason: "no ManagedServerSupervisor attached to EngineLifecycle".to_owned(),
            };
            self.emit_managed_server_error(&err);
            return Err(err);
        };

        self.push_event(EngineEvent::ManagedServerUpdate {
            state: "verifying".to_owned(),
        });

        let res = sup.update_server_artifact(
            profile_id,
            new_artifact,
            new_version,
            tunnel_is_active,
            confirm_disconnect,
        );
        self.managed_server_status = sup.status().clone();
        self.server_supervisor = Some(sup);

        match res {
            Ok(()) => {
                self.push_event(EngineEvent::ManagedServerStatusChanged {
                    state: self.managed_server_status.state,
                    progress: Some(100),
                });
                self.push_event(EngineEvent::ManagedServerUpdate {
                    state: "completed".to_owned(),
                });
                Ok(())
            }
            Err(err) => {
                self.push_event(EngineEvent::ManagedServerUpdate {
                    state: "failed".to_owned(),
                });
                self.emit_managed_server_error(&err);
                Err(err)
            }
        }
    }

    fn emit_managed_server_error(&mut self, err: &ManagedServerError) {
        self.push_event(EngineEvent::EngineError {
            code: err.code().to_owned(),
            message: err.to_string(),
        });
    }

    fn apply_hello_ack(&mut self, ack: HelloAckMessage, now_ms: u64) -> Result<(), String> {
        let was_reconnecting = self.session.state() == EngineSessionState::Reconnecting;

        // Bind wire mapping indexes in the target policy table without touching local addresses (`SEC-006`).
        self.target_table.bind_hello_ack_mappings(&ack.mappings);

        // Update public endpoints with authoritative wire indexes and public ports (`FR-005`).
        let relay_host = self
            .relay_host
            .clone()
            .unwrap_or_else(|| "relay.invalid".to_owned());
        self.public_endpoints =
            filter_enabled_public_endpoints(&self.target_table, &ack.mappings, &relay_host);

        let next = self
            .session
            .on_hello_ack(ack, now_ms)
            .map_err(|e| format!("invalid transition on hello_ack: {e}"))?;

        let reason = if was_reconnecting {
            "reconnect_succeeded"
        } else {
            "hello_ack"
        };
        self.push_event(EngineEvent::TunnelStatusChanged {
            state: next,
            reason: reason.to_owned(),
        });
        Ok(())
    }

    fn apply_reject(&mut self, reject: RejectMessage) -> Result<(), String> {
        let code = reject.code;
        let next = self
            .session
            .on_reject(code)
            .map_err(|e| format!("invalid transition on reject: {e}"))?;

        self.push_event(EngineEvent::TunnelStatusChanged {
            state: next,
            reason: format!("reject:{}", reject_code_str(code)),
        });

        if next == EngineSessionState::Failed {
            self.push_event(EngineEvent::EngineError {
                code: reject_code_str(code).to_owned(),
                message: format!(
                    "relay rejected session with terminal code {}",
                    reject_code_str(code)
                ),
            });
        }
        Ok(())
    }

    fn apply_flow_summary(&mut self, summary: FlowSummaryMessage) {
        let snapshot = PlayersSeenSnapshot {
            distinct_sources: summary.distinct_sources,
            tcp_flows: summary.tcp_flows,
            udp_flows: summary.udp_flows,
            label: OBSERVED_TRAFFIC_LABEL,
        };
        self.players_seen = snapshot.clone();
        self.push_event(EngineEvent::PlayersSeen(snapshot));
    }

    fn push_event(&mut self, event: EngineEvent) {
        if self.events.len() >= MAX_BUFFERED_ENGINE_EVENTS {
            let _ = self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Export a redacted [`DiagnosticsBundle`] capturing the current 5-layer status snapshot,
    /// recent engine events, and caller-supplied log lines (`T-075`, `FR-013`, `PRIV-002`).
    pub fn export_diagnostics_bundle(
        &self,
        generated_at_epoch_ms: u64,
        raw_logs: &[&str],
    ) -> DiagnosticsBundle {
        let snapshot = self.get_status();
        let events: Vec<EngineEvent> = self.events.iter().cloned().collect();
        DiagnosticsBundle::build(
            generated_at_epoch_ms,
            self.connectivity_mode,
            &snapshot,
            &events,
            raw_logs,
        )
    }
}

fn filter_enabled_public_endpoints(
    table: &ValidatedLocalTargetTable,
    ack_mappings: &[HelloAckMapping],
    relay_host: &str,
) -> Vec<PublicMappingEndpoint> {
    let mut endpoints = Vec::with_capacity(ack_mappings.len());
    for ack in ack_mappings {
        if let Some(local) = table.get_by_key(&ack.mapping_key) {
            if local.enabled() && local.protocol() == ack.protocol {
                endpoints.push(PublicMappingEndpoint {
                    index: ack.index,
                    mapping_key: ack.mapping_key.clone(),
                    protocol: ack.protocol,
                    relay_host: relay_host.to_owned(),
                    public_port: ack.public_port,
                });
            }
        }
    }
    endpoints
}

fn reject_code_str(code: RejectCode) -> &'static str {
    match code {
        RejectCode::TokenInvalid => "token_invalid",
        RejectCode::TokenExpired => "token_expired",
        RejectCode::AllocationNotFound => "allocation_not_found",
        RejectCode::VersionUnsupported => "version_unsupported",
        RejectCode::Capacity => "capacity",
        RejectCode::RelayDraining => "relay_draining",
    }
}
