//! Tunnel client: control-plane HTTP API client (T-047), QUIC connection, stream and datagram
//! handling, local dialing, and health checks.
//!
//! Implements the control-plane client retry/timeout contract in `contracts.md` §4 (`T-047`),
//! the engine state machine in `contracts.md` §12 (`T-070`), and the local target policy in
//! `contracts.md` §13 (`T-071`).

pub mod api_client;
pub mod diagnostics;
pub mod lifecycle;
pub mod probe;
pub mod session;
pub mod tailscale;
pub mod target_policy;
pub mod tcp;
pub mod udp;

pub use api_client::{
    compute_retry_delay, is_request_retryable, is_status_retryable, parse_retry_after,
    AllocationTokenResponse, ApiClientError, BackoffPolicy, ClientConfig, ControlPlaneClient,
    DeterministicBackoff, ExecutedResponse, HttpMethod, HttpRequest, HttpResponse, HttpTransport,
    IdempotencyKey, NoTokenRefresher, TokenRefreshError, TokenRefresher, TransportError,
    DEFAULT_BASE_BACKOFF, DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_BACKOFF,
    DEFAULT_MAX_RESPONSE_BODY_BYTES, DEFAULT_MAX_RETRY_AFTER, DEFAULT_REQUEST_TIMEOUT,
    MAX_ATTEMPTS, MAX_BACKOFF_CEILING, MAX_BASE_URL_LEN, MAX_CONNECT_TIMEOUT,
    MAX_IDEMPOTENCY_KEY_LEN, MAX_REQUEST_TIMEOUT, MAX_RESPONSE_BODY_CEILING,
    MAX_RETRY_AFTER_CEILING, MIN_IDEMPOTENCY_KEY_LEN,
};
pub use diagnostics::{
    redact_diagnostic_text, scan_diagnostics_for_leaks, DiagnosticsBundle,
    DiagnosticsMappingSummary, DiagnosticsProbeSummary, DiagnosticsStatusSummary,
    DIAGNOSTICS_PRIVACY_NOTICE, DIAGNOSTICS_SCHEMA_VERSION, MAX_DIAGNOSTIC_EVENTS,
    MAX_DIAGNOSTIC_LINE_CHARS, MAX_DIAGNOSTIC_LOG_LINES, REDACTED_INVITATION_MARKER,
    REDACTED_IP_MARKER, REDACTED_PAYLOAD_MARKER, REDACTED_SECRET_MARKER, REDACTED_TOKEN_MARKER,
};
pub use gtt_server_manager::{
    parse_java_major_version, parse_msmp_player_notification, validate_java_version_output,
    AcquisitionRecord, ArtifactSource, ConfirmedPresenceSnapshot, DataPaths, Digest,
    GeneratedConfig, ManagedServerError, ManagedServerState, ManagedServerStatusSnapshot,
    ManagedServerSupervisor, MsmpPlayerNotification, ProvisionableSet, ServerConfigRequest,
    ServerLaunchSpec, CONFIRMED_PRESENCE_LABEL, DEFAULT_MAX_CRASH_RESTARTS,
    MINECRAFT_JAVA_EULA_MARKER, MINECRAFT_JAVA_PROFILE_ID, MINECRAFT_JAVA_READY_MARKER,
};
pub use lifecycle::{
    EngineEvent, EngineLifecycle, EngineStatusSnapshot, PlayersSeenSnapshot,
    ProvisionableProfileEntry, PublicMappingEndpoint, SessionConnectivityMode,
    MAX_BUFFERED_ENGINE_EVENTS, OBSERVED_TRAFFIC_LABEL,
};
pub use probe::{
    build_mc_java_slp_handshake_and_status_request, build_mc_java_slp_ping_request,
    build_mc_java_slp_status_response, decode_mc_varint, encode_mc_varint,
    parse_mc_java_slp_status_response, probe_local_targets, probe_minecraft_java_slp,
    probe_tcp_connect, probe_udp_mapping, probe_validated_target, LocalProbeResult,
    MinecraftJavaSlpProbeReport, MinecraftJavaStatusPlayers, MinecraftJavaStatusResponse,
    MinecraftJavaStatusVersion, ProbeLevel, ProbeStatus, UdpProbeKind, DEFAULT_PROBE_TIMEOUT,
    MAX_MC_SLP_HOST_BYTES, MAX_MC_SLP_STATUS_BYTES, MC_JAVA_QUERY_HANDSHAKE_REQUEST,
    RAKNET_OFFLINE_MAGIC, SOURCE_A2S_INFO_REQUEST,
};
pub use session::{
    EngineTunnelSession, DEFAULT_DEGRADED_RTT_THRESHOLD_MS, DEFAULT_MAX_RECONNECT_ATTEMPTS,
};
pub use tailscale::{
    is_tailscale_ipv4, is_tailscale_ipv6, parse_tailscale_status_json, CliTailscaleDetector,
    StaticTailscaleDetector, TailscaleBackendState, TailscaleBindMode, TailscaleDetector,
    TailscaleError, TailscaleForwarderMetrics, TailscaleNodeShareForwarder, TailscaleNodeStatus,
    TailscaleShareEndpoint, DEFAULT_TAILSCALE_CLI_TIMEOUT, MAX_TAILSCALE_PEERS_TRACKED,
    MAX_TAILSCALE_STATUS_JSON_BYTES, TAILSCALE_ADMIN_MACHINES_URL,
    TAILSCALE_NODE_SHARE_SECURITY_NOTE,
};
pub use target_policy::{
    configure_mappings, is_sensitive_local_port, validate_mapping_target, DialRefusalReason,
    MappingTargetInput, MappingValidationFailure, TargetPolicyViolation, ValidatedLocalTargetTable,
    ValidatedMappingTarget, SENSITIVE_LOCAL_PORTS,
};
pub use tcp::{
    bridge_engine_tcp_streams, engine_handle_tcp_stream, EngineTcpError, EngineTcpMetrics,
    EngineTcpSession, LocalMappingConfig, DEFAULT_ENGINE_TCP_IO_TIMEOUT,
    DEFAULT_LOCAL_DIAL_TIMEOUT, ENGINE_TCP_CHUNK_BYTES,
};
pub use udp::{
    EngineUdpDropReason, EngineUdpMetrics, EngineUdpSession, DEFAULT_ENGINE_UDP_FLOW_TIMEOUT_MS,
    DEFAULT_LOCAL_UDP_SOCKET_TIMEOUT,
};
