//! Managed server process supervision, state machine, and confirmed-presence parser for
//! Minecraft: Java Edition (`T-076`, `SRV-005`, `SRV-006`, `SRV-007`, `SRV-011`, `SRV-015`,
//! `SRV-018`, `ADR-013`, `ADR-015`, `contracts.md` §13, `architecture.md` §12).
//!
//! # Architectural Boundaries
//!
//! * **Strictly host-local (`SRV-005`, `INV-1`).** Managed server configuration, data paths,
//!   process supervision, and local target ports exist only inside the desktop application process
//!   tree. Nothing in this module ever touches the network, the control plane, or the relay.
//! * **Fourth independent status indicator (`SRV-006`, `FR-010`, `UX-002`).** [`ManagedServerState`]
//!   is stored and emitted independently of the tunnel session state, local reachability probes,
//!   and observed relay traffic (`players_seen`).
//! * **Fifth opt-in indicator for confirmed presence (`SRV-011`, `research.md` RQ-13.4).** When the
//!   Minecraft Server Management Protocol (MSMP) is enabled on a managed Java server, its
//!   `minecraft:notification/players/joined` and `minecraft:notification/players/left` JSON-RPC 2.0
//!   notifications are surfaced via [`ConfirmedPresenceSnapshot`] with the mandatory disclosure
//!   label [`CONFIRMED_PRESENCE_LABEL`] (`"Confirmed by the game server"`), and are never merged
//!   with the relay flow-based `players_seen` counter.
//! * **Windows graceful stop & process-tree cleanup (`SRV-007`, `research.md` RQ-13.2c/d).** Holds
//!   a writable `stdin` pipe for the server's lifetime, sends `stop\n` on graceful stop, treats any
//!   process exit before the `Done (` readiness line as [`ManagedServerState::Failed`] (`RefusedBeforeReady`)
//!   rather than a clean exit or retryable crash, and terminates the Windows process tree (`taskkill /T /F`)
//!   if the process does not exit within the stop timeout or when dropped on application exit.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use gtt_profiles::Profile;

use crate::config::{generate, ConfigError, GeneratedConfig, ServerConfigRequest};
use crate::paths::{DataPaths, PathError};
use crate::provision::{AcquisitionRecord, ProvisionError, ProvisionableSet};
use crate::verify::{verify, ArtifactSource, VerifyError};

/// Mandatory UI disclosure label for the fifth status source (`SRV-011`, `architecture.md` §12,
/// `requirements.md` `SRV-011`).
///
/// Confirmed presence reported directly by a managed Minecraft: Java Edition server via the
/// Server Management Protocol is never merged with the relay traffic-based `players_seen` count.
pub const CONFIRMED_PRESENCE_LABEL: &str = "Confirmed by the game server";

/// Profile identifier for the first-cut managed server game (`ADR-013`).
pub const MINECRAFT_JAVA_PROFILE_ID: &str = "minecraft-java";

/// Readiness log marker emitted by Minecraft: Java Edition once listening (`SRV-007`,
/// `research.md` §5 RQ-13.2c).
pub const MINECRAFT_JAVA_READY_MARKER: &str = "Done (";

/// EULA refusal marker emitted by Minecraft: Java Edition when `eula=true` is not present
/// (`SRV-007`, `SRV-015`, `research.md` §5 RQ-13.2c).
pub const MINECRAFT_JAVA_EULA_MARKER: &str = "You need to agree to the EULA";

/// Default maximum consecutive unexpected crashes after readiness before automatic restart stops
/// (`SRV-007`).
pub const DEFAULT_MAX_CRASH_RESTARTS: u32 = 3;

/// Maximum bytes retained in a log excerpt surfaced to the UI (`AGENTS.md` §4).
pub const MAX_LOG_EXCERPT_BYTES: usize = 4096;

/// Maximum number of recent log lines retained in memory per managed server (`AGENTS.md` §4).
pub const MAX_RETAINED_LOG_LINES: usize = 64;

/// Maximum number of distinct confirmed players tracked in [`ConfirmedPresenceSnapshot`]
/// (`AGENTS.md` §4).
pub const MAX_CONFIRMED_PLAYERS: usize = 256;

/// Fourth independent status indicator: managed game server lifecycle state (`SRV-006`,
/// `architecture.md` §12, `contracts.md` §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedServerState {
    /// No managed server has been provisioned in the per-user data directory.
    NotInstalled,
    /// Server or runtime artifact is being downloaded.
    Downloading,
    /// Downloaded artifact is being checked against its published digest (`SRV-002`).
    Verifying,
    /// `server.properties` and directory structure are being prepared (`SRV-004`, `SRV-010`).
    Configuring,
    /// Child process has been spawned; waiting for the [`MINECRAFT_JAVA_READY_MARKER`] (`Done (`)
    /// readiness line (`SRV-007`).
    Starting,
    /// Readiness line observed and child process is alive (`SRV-006`, `SRV-007`).
    Running,
    /// Process exited unexpectedly *after* reaching `Running` and is eligible for bounded
    /// automatic restart (`SRV-007`).
    Crashed,
    /// Server artifact is being updated while preserving world data (`SRV-008`, `SRV-009`).
    Updating,
    /// Server failed before readiness (e.g. EULA not accepted, refused start with exit 0) or
    /// exhausted its crash-restart budget (`SRV-007`).
    Failed,
}

impl ManagedServerState {
    /// Lowercase wire/UI identifier matching `architecture.md` §12 and `contracts.md` §13.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Configuring => "configuring",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Crashed => "crashed",
            Self::Updating => "updating",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for ManagedServerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Machine-readable engine error codes for managed server operations (`contracts.md` §13,
/// `UX-003`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedServerError {
    /// Profile is not in the verified [`ProvisionableSet`] (`SRV-003`).
    NotProvisionable {
        /// Profile identifier.
        profile_id: String,
        /// Actionable explanation.
        reason: String,
    },
    /// Artifact download failed (`contracts.md` §13).
    DownloadFailed {
        /// Reason for failure.
        reason: String,
    },
    /// Artifact digest check failed (`SRV-002`, `contracts.md` §13).
    VerificationFailed {
        /// Underlying verification error message.
        reason: String,
    },
    /// Host has not yet accepted the Minecraft EULA in `eula.txt` (`SRV-015`, `contracts.md` §13).
    LicenseTermsNotAccepted {
        /// Path to `eula.txt` that the host must edit deliberately.
        eula_path: String,
    },
    /// Disk budget or write failure under the per-user directory (`SRV-001`, `SRV-010`,
    /// `contracts.md` §13).
    DiskExhaustion {
        /// Underlying storage error description.
        reason: String,
    },
    /// Process exited before emitting the readiness line (`SRV-007` requirement (a): a refused
    /// start exits 0 and must never be treated as a clean exit or retried blindly).
    RefusedBeforeReady {
        /// Process exit code, if any.
        exit_code: Option<i32>,
        /// Last log excerpt explaining why startup was refused.
        log_excerpt: String,
    },
    /// Crash restart budget reached (`SRV-007`, `contracts.md` §13).
    RestartLimitReached {
        /// Number of consecutive crashes after readiness.
        attempts: u32,
        /// Last log excerpt from the crashed process.
        log_excerpt: String,
    },
    /// Java runtime selection failed (`SRV-018`).
    JavaRuntimeRejected {
        /// Required Java major version from version metadata.
        required_major: u16,
        /// Found Java major version, if any.
        found_major: Option<u16>,
        /// Actionable explanation.
        reason: String,
    },
    /// Update refused because a tunnel is currently active and confirmation was not supplied
    /// (`SRV-009`, `contracts.md` §13 `update_managed_server`).
    UpdateRequiresActiveTunnelConfirmation,
    /// Path confinement or configuration generation failed (`SRV-004`, `SRV-010`).
    ConfigurationInvalid {
        /// Underlying error reason.
        reason: String,
    },
    /// Process spawn or I/O operation failed.
    ProcessIo {
        /// Underlying error reason.
        reason: String,
    },
}

impl ManagedServerError {
    /// Stable lowercase engine error code for `engine_error{code, message}` (`contracts.md` §13,
    /// `UX-003`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotProvisionable { .. } => "server_not_provisionable",
            Self::DownloadFailed { .. } => "server_download_failed",
            Self::VerificationFailed { .. } => "server_verification_failed",
            Self::LicenseTermsNotAccepted { .. } => "license_terms_not_accepted",
            Self::DiskExhaustion { .. } => "server_disk_exhaustion",
            Self::RefusedBeforeReady { .. } => "server_refused_before_ready",
            Self::RestartLimitReached { .. } => "server_restart_limit_reached",
            Self::JavaRuntimeRejected { .. } => "server_java_runtime_rejected",
            Self::UpdateRequiresActiveTunnelConfirmation => "server_update_requires_confirmation",
            Self::ConfigurationInvalid { .. } => "server_configuration_invalid",
            Self::ProcessIo { .. } => "server_process_io_error",
        }
    }
}

impl fmt::Display for ManagedServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotProvisionable { profile_id, reason } => {
                write!(f, "profile {profile_id:?} cannot be provisioned: {reason}")
            }
            Self::DownloadFailed { reason } => {
                write!(f, "failed to download server artifact: {reason}")
            }
            Self::VerificationFailed { reason } => {
                write!(f, "server artifact verification failed: {reason}")
            }
            Self::LicenseTermsNotAccepted { eula_path } => write!(
                f,
                "Minecraft EULA has not been accepted by the host; set eula=true in {eula_path} (SRV-015)"
            ),
            Self::DiskExhaustion { reason } => {
                write!(f, "insufficient disk space or storage failure: {reason}")
            }
            Self::RefusedBeforeReady {
                exit_code,
                log_excerpt,
            } => write!(
                f,
                "managed server exited before reaching readiness (exit_code={exit_code:?}): {log_excerpt}"
            ),
            Self::RestartLimitReached {
                attempts,
                log_excerpt,
            } => write!(
                f,
                "managed server crashed {attempts} time(s) and reached the automatic restart limit: {log_excerpt}"
            ),
            Self::JavaRuntimeRejected {
                required_major,
                found_major,
                reason,
            } => write!(
                f,
                "Java {required_major} is required (found {found_major:?}): {reason}"
            ),
            Self::UpdateRequiresActiveTunnelConfirmation => write!(
                f,
                "updating a managed server while a tunnel is active will disconnect guests and requires explicit host confirmation (SRV-009)"
            ),
            Self::ConfigurationInvalid { reason } => {
                write!(f, "invalid managed server configuration: {reason}")
            }
            Self::ProcessIo { reason } => {
                write!(f, "managed server process I/O error: {reason}")
            }
        }
    }
}

impl std::error::Error for ManagedServerError {}

impl From<ProvisionError> for ManagedServerError {
    fn from(err: ProvisionError) -> Self {
        match err {
            ProvisionError::NotProvisionable { profile_id, reason } => {
                Self::NotProvisionable { profile_id, reason }
            }
        }
    }
}

impl From<VerifyError> for ManagedServerError {
    fn from(err: VerifyError) -> Self {
        Self::VerificationFailed {
            reason: err.to_string(),
        }
    }
}

impl From<ConfigError> for ManagedServerError {
    fn from(err: ConfigError) -> Self {
        Self::ConfigurationInvalid {
            reason: err.to_string(),
        }
    }
}

impl From<PathError> for ManagedServerError {
    fn from(err: PathError) -> Self {
        Self::ConfigurationInvalid {
            reason: err.to_string(),
        }
    }
}

/// Snapshot of the managed server status indicator returned by `get_managed_server_status`
/// (`contracts.md` §13, `SRV-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedServerStatusSnapshot {
    /// Profile identifier (`"minecraft-java"`).
    pub profile_id: String,
    /// Independent managed-server lifecycle state (`SRV-006`).
    pub state: ManagedServerState,
    /// Installed server version string, if provisioned.
    pub version: Option<String>,
    /// Most recent log excerpt from the server process (`contracts.md` §13).
    pub last_log_excerpt: String,
    /// Latest world backup identifier, if a backup has been taken (`SRV-008`, `contracts.md` §13).
    pub last_backup_id: Option<String>,
    /// Number of consecutive crashes observed since the server last reached `Running`.
    pub crash_count: u32,
}

impl Default for ManagedServerStatusSnapshot {
    fn default() -> Self {
        Self {
            profile_id: MINECRAFT_JAVA_PROFILE_ID.to_owned(),
            state: ManagedServerState::NotInstalled,
            version: None,
            last_log_excerpt: String::new(),
            last_backup_id: None,
            crash_count: 0,
        }
    }
}

/// Fifth opt-in status source: confirmed in-game player presence reported by a managed
/// Minecraft: Java Edition server over the Server Management Protocol (`SRV-011`,
/// `research.md` RQ-13.4, `architecture.md` §12).
///
/// This snapshot is **never** merged into the relay traffic-based `players_seen` indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPresenceSnapshot {
    /// Whether MSMP confirmed-presence reporting is active for this managed server.
    pub enabled: bool,
    /// Count of players currently confirmed in-game by the server (`SRV-011`).
    pub confirmed_players: u32,
    /// Mandatory disclosure label (`"Confirmed by the game server"`).
    pub label: &'static str,
}

impl Default for ConfirmedPresenceSnapshot {
    fn default() -> Self {
        Self {
            enabled: false,
            confirmed_players: 0,
            label: CONFIRMED_PRESENCE_LABEL,
        }
    }
}

/// Parsed MSMP player join/leave notification (`SRV-011`, `research.md` RQ-13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsmpPlayerNotification {
    /// `minecraft:notification/players/joined`
    Joined {
        /// Unique player identifier (UUID or name from the notification payload).
        player_id: String,
    },
    /// `minecraft:notification/players/left`
    Left {
        /// Unique player identifier (UUID or name from the notification payload).
        player_id: String,
    },
}

/// Parse a Minecraft Server Management Protocol (MSMP) JSON-RPC 2.0 notification line
/// (`minecraft:notification/players/joined` or `minecraft:notification/players/left`,
/// `research.md` RQ-13.4, `SRV-011`).
///
/// Bounded and panic-free: ignores inputs over 16 KiB or non-matching JSON.
pub fn parse_msmp_player_notification(raw_json: &str) -> Option<MsmpPlayerNotification> {
    if raw_json.len() > 16 * 1024 {
        return None;
    }
    let trimmed = raw_json.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let is_join = trimmed.contains("\"minecraft:notification/players/joined\"");
    let is_leave = trimmed.contains("\"minecraft:notification/players/left\"");
    if !is_join && !is_leave {
        return None;
    }

    let player_id = extract_json_string_field(trimmed, "id")
        .or_else(|| extract_json_string_field(trimmed, "uuid"))
        .or_else(|| extract_json_string_field(trimmed, "name"))?;

    if player_id.is_empty() || player_id.len() > 128 {
        return None;
    }

    if is_join {
        Some(MsmpPlayerNotification::Joined { player_id })
    } else {
        Some(MsmpPlayerNotification::Left { player_id })
    }
}

fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let mut search_from = 0;
    while let Some(rel_pos) = json.get(search_from..)?.find(&pattern) {
        let after_key = search_from + rel_pos + pattern.len();
        let rest = json.get(after_key..)?.trim_start();
        if let Some(after_colon) = rest.strip_prefix(':') {
            let val_rest = after_colon.trim_start();
            if let Some(after_quote) = val_rest.strip_prefix('"') {
                let mut out = String::new();
                for ch in after_quote.chars() {
                    if ch == '"' {
                        return Some(out);
                    }
                    if ch.is_control() || out.len() >= 128 {
                        break;
                    }
                    out.push(ch);
                }
            }
        }
        search_from = after_key;
    }
    None
}

/// Parse the major Java version from `java -version` output (`SRV-018`, `research.md` §5
/// RQ-13.2e).
///
/// Handles both modern `"25.0.4.1"` / `"21.0.2"` and legacy `"1.8.0_481"` version strings.
pub fn parse_java_major_version(version_output: &str) -> Option<u16> {
    for line in version_output.lines().take(16) {
        if !line.contains("version") {
            continue;
        }
        let mut parts = line.split('"');
        let _before = parts.next()?;
        let quoted = parts.next()?;
        let mut numbers = quoted.split(|c: char| !c.is_ascii_digit());
        let first_str = numbers.next()?;
        let first: u16 = first_str.parse().ok()?;
        if first == 1 {
            let second_str = numbers.next()?;
            let second: u16 = second_str.parse().ok()?;
            return Some(second);
        }
        if first > 0 {
            return Some(first);
        }
    }
    None
}

/// Validate a Java runtime version against `required_major` (`SRV-018`).
pub fn validate_java_version_output(
    version_output: &str,
    required_major: u16,
) -> Result<u16, ManagedServerError> {
    match parse_java_major_version(version_output) {
        Some(found) if found >= required_major => Ok(found),
        Some(found) => Err(ManagedServerError::JavaRuntimeRejected {
            required_major,
            found_major: Some(found),
            reason: format!(
                "Java runtime reported major version {found}, which is older than required Java {required_major}"
            ),
        }),
        None => Err(ManagedServerError::JavaRuntimeRejected {
            required_major,
            found_major: None,
            reason: "could not parse major version from `java -version` output".to_owned(),
        }),
    }
}

/// Command launch specification for starting the managed server child process (`SRV-007`).
///
/// Allows tests to launch a lightweight cross-platform helper process (e.g. `python` or `cmd`)
/// that speaks the exact Minecraft Java console protocol (`Done (` readiness line on stdout and
/// `stop\n` on stdin) while production launches the verified Java runtime (`java -jar server.jar nogui`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLaunchSpec {
    /// Executable path (e.g. per-user verified `java.exe`).
    pub program: PathBuf,
    /// Arguments passed to `program` (e.g. `["-jar", "server.jar", "nogui"]`).
    pub args: Vec<String>,
    /// Required Java major version from version metadata (`SRV-018`).
    pub required_java_major: u16,
    /// Validated Java major version of the selected runtime (`SRV-018`).
    pub selected_java_major: u16,
}

impl ServerLaunchSpec {
    /// Construct the standard Minecraft: Java Edition launch specification (`java -jar server.jar nogui`)
    /// after validating the runtime version (`SRV-018`).
    pub fn minecraft_java(
        java_executable: PathBuf,
        version_output: &str,
        required_java_major: u16,
    ) -> Result<Self, ManagedServerError> {
        let selected_java_major =
            validate_java_version_output(version_output, required_java_major)?;
        Ok(Self {
            program: java_executable,
            args: vec![
                "-jar".to_owned(),
                "server.jar".to_owned(),
                "nogui".to_owned(),
            ],
            required_java_major,
            selected_java_major,
        })
    }

    /// Discover and validate the system Java runtime on `PATH` (`SRV-018`).
    pub fn detect_system_java(required_java_major: u16) -> Result<Self, ManagedServerError> {
        let output = Command::new("java").arg("-version").output().map_err(|e| {
            ManagedServerError::JavaRuntimeRejected {
                required_major: required_java_major,
                found_major: None,
                reason: format!("could not execute `java -version`: {e}"),
            }
        })?;
        let version_str = String::from_utf8_lossy(&output.stderr);
        Self::minecraft_java(PathBuf::from("java"), &version_str, required_java_major)
    }
}

struct RunningServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    log_path: PathBuf,
    log_offset: u64,
    reached_ready: bool,
    launch_spec: ServerLaunchSpec,
}

/// Host-local supervisor for a managed Minecraft: Java Edition server (`T-076`, `SRV-001`..`SRV-011`,
/// `SRV-015`, `SRV-018`).
pub struct ManagedServerSupervisor {
    paths: DataPaths,
    provisionable: ProvisionableSet,
    status: ManagedServerStatusSnapshot,
    confirmed_presence: ConfirmedPresenceSnapshot,
    confirmed_player_ids: BTreeSet<String>,
    recent_logs: VecDeque<String>,
    max_crash_restarts: u32,
    running: Option<RunningServerProcess>,
    last_launch_spec: Option<ServerLaunchSpec>,
    local_server_port: Option<u16>,
}

impl fmt::Debug for ManagedServerSupervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedServerSupervisor")
            .field("root", &self.paths.root())
            .field("status", &self.status)
            .field("confirmed_presence", &self.confirmed_presence)
            .field("max_crash_restarts", &self.max_crash_restarts)
            .field("has_running_process", &self.running.is_some())
            .field("local_server_port", &self.local_server_port)
            .finish()
    }
}

impl ManagedServerSupervisor {
    /// Create a new [`ManagedServerSupervisor`] confined to `paths` (`SRV-010`) and gated by
    /// `provisionable` (`SRV-003`).
    pub fn new(paths: DataPaths, provisionable: ProvisionableSet) -> Self {
        Self {
            paths,
            provisionable,
            status: ManagedServerStatusSnapshot::default(),
            confirmed_presence: ConfirmedPresenceSnapshot::default(),
            confirmed_player_ids: BTreeSet::new(),
            recent_logs: VecDeque::new(),
            max_crash_restarts: DEFAULT_MAX_CRASH_RESTARTS,
            running: None,
            last_launch_spec: None,
            local_server_port: None,
        }
    }

    /// Borrow the underlying [`DataPaths`] (`SRV-010`).
    pub fn data_paths(&self) -> &DataPaths {
        &self.paths
    }

    /// Borrow the [`ProvisionableSet`] (`SRV-003`).
    pub fn provisionable_set(&self) -> &ProvisionableSet {
        &self.provisionable
    }

    /// Replace the [`ProvisionableSet`] (for example when an acquisition record is loaded).
    pub fn set_provisionable_set(&mut self, set: ProvisionableSet) {
        self.provisionable = set;
    }

    /// Set the maximum number of automatic crash restarts after readiness (`SRV-007`).
    pub fn set_max_crash_restarts(&mut self, max_restarts: u32) {
        self.max_crash_restarts = max_restarts;
    }

    /// Current fourth-layer managed server status snapshot (`SRV-006`, `contracts.md` §13
    /// `get_managed_server_status`).
    pub fn status(&self) -> &ManagedServerStatusSnapshot {
        &self.status
    }

    /// Current fifth-layer confirmed-presence snapshot (`SRV-011`, `architecture.md` §12).
    pub fn confirmed_presence(&self) -> &ConfirmedPresenceSnapshot {
        &self.confirmed_presence
    }

    /// Configured local server port from `server.properties` (`SRV-005`: kept strictly in-process,
    /// never sent to the control plane or relay).
    pub fn local_server_port(&self) -> Option<u16> {
        self.local_server_port
    }

    /// Resolve the confined directory for a profile (`servers/<profile_id>`, `SRV-010`).
    pub fn server_dir(&self, profile_id: &str) -> Result<PathBuf, ManagedServerError> {
        Ok(self.paths.resolve(&["servers", profile_id])?)
    }

    /// Provision a Minecraft: Java Edition server inside the confined per-user directory
    /// (`SRV-001`, `SRV-002`, `SRV-003`, `SRV-004`, `SRV-010`, `SRV-015`, `contracts.md` §13
    /// `provision_server`).
    ///
    /// # Security and Licensing Guarantees
    /// 1. Checks `SRV-003` via [`ProvisionableSet::require`]; refuses unverified profiles.
    /// 2. Checks `SRV-002` via [`verify`] **before** copying or writing anything executable.
    /// 3. Writes `server.properties` via [`generate`] with `enable-rcon=false` and
    ///    `enable-query=false` structurally enforced (`SRV-004`).
    /// 4. Writes `eula.txt` with `eula=false` if absent and **never** writes `eula=true` on the
    ///    host's behalf (`SRV-015`).
    pub fn provision_minecraft_java(
        &mut self,
        profile: &Profile,
        artifact: &ArtifactSource,
        version: &str,
        max_players: u16,
    ) -> Result<(AcquisitionRecord, GeneratedConfig), ManagedServerError> {
        let record = self.provisionable.require(profile)?.clone();

        self.status.profile_id = profile.id.clone();
        self.status.state = ManagedServerState::Verifying;

        if let Err(err) = verify(artifact) {
            self.status.state = ManagedServerState::Failed;
            let mapped = ManagedServerError::from(err);
            self.record_log_line(&mapped.to_string());
            return Err(mapped);
        }

        self.status.state = ManagedServerState::Configuring;
        let srv_dir = self.server_dir(&profile.id)?;
        fs::create_dir_all(&srv_dir).map_err(|e| ManagedServerError::DiskExhaustion {
            reason: format!("failed to create server directory: {e}"),
        })?;

        let jar_dest = self
            .paths
            .resolve(&["servers", &profile.id, "server.jar"])?;
        if artifact.path != jar_dest {
            fs::copy(&artifact.path, &jar_dest).map_err(|e| {
                ManagedServerError::DiskExhaustion {
                    reason: format!("failed to stage verified server.jar: {e}"),
                }
            })?;
        }

        let generated = generate(&ServerConfigRequest {
            profile,
            max_players,
            rcon_secret: None,
            enable_query: false,
            query_secret: None,
        })?;

        let props_path = self
            .paths
            .resolve(&["servers", &profile.id, "server.properties"])?;
        fs::write(&props_path, &generated.contents).map_err(|e| {
            ManagedServerError::DiskExhaustion {
                reason: format!("failed to write server.properties: {e}"),
            }
        })?;

        // SRV-015: never write eula=true on the host's behalf. Create eula.txt with eula=false
        // only when absent so the host can deliberately accept it.
        let eula_path = self.paths.resolve(&["servers", &profile.id, "eula.txt"])?;
        if !eula_path.exists() {
            fs::write(
                &eula_path,
                "# By changing the setting below to TRUE you are indicating your agreement to our EULA (https://aka.ms/MinecraftEULA).\neula=false\n",
            )
            .map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to create initial eula.txt: {e}"),
            })?;
        }

        let world_dir = self.paths.resolve(&["servers", &profile.id, "world"])?;
        fs::create_dir_all(&world_dir).map_err(|e| ManagedServerError::DiskExhaustion {
            reason: format!("failed to create world directory: {e}"),
        })?;

        self.local_server_port = Some(generated.server_port);
        self.status.version = Some(version.to_owned());
        self.status.crash_count = 0;
        self.record_log_line(&format!(
            "Provisioned {} {} on local port {} (RCON={}, query={})",
            profile.id,
            version,
            generated.server_port,
            generated.rcon_enabled,
            generated.query_enabled
        ));

        Ok((record, generated))
    }

    /// Check whether the host has deliberately set `eula=true` in `eula.txt` (`SRV-015`).
    pub fn is_eula_accepted(&self, profile_id: &str) -> Result<bool, ManagedServerError> {
        let eula_path = self.paths.resolve(&["servers", profile_id, "eula.txt"])?;
        if !eula_path.is_file() {
            return Ok(false);
        }
        let contents =
            fs::read_to_string(&eula_path).map_err(|e| ManagedServerError::ProcessIo {
                reason: format!("failed to read eula.txt: {e}"),
            })?;
        let accepted = contents.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.eq_ignore_ascii_case("eula=true")
        });
        Ok(accepted)
    }

    /// Deliberately set or revoke EULA acceptance in `servers/<profile_id>/eula.txt` (`SRV-015`).
    pub fn set_eula_accepted(
        &mut self,
        profile_id: &str,
        accepted: bool,
    ) -> Result<(), ManagedServerError> {
        let eula_path = self.paths.resolve(&["servers", profile_id, "eula.txt"])?;
        let s = if accepted {
            "# By changing the setting below to TRUE you are indicating your agreement to our EULA (https://aka.ms/MinecraftEULA).\neula=true\n"
        } else {
            "# By changing the setting below to TRUE you are indicating your agreement to our EULA (https://aka.ms/MinecraftEULA).\neula=false\n"
        };
        fs::write(&eula_path, s).map_err(|e| ManagedServerError::DiskExhaustion {
            reason: format!("failed to write eula.txt: {e}"),
        })?;
        Ok(())
    }

    /// Start the managed server process (`SRV-007`, `SRV-015`, `SRV-018`, `contracts.md` §13
    /// `start_managed_server`).
    ///
    /// Holds a writable `stdin` pipe for the lifetime of the child process (`SRV-007` requirement
    /// (b)) and redirects stdout/stderr to `servers/<profile_id>/server.log`.
    pub fn start_server(
        &mut self,
        profile_id: &str,
        launch_spec: ServerLaunchSpec,
    ) -> Result<ManagedServerState, ManagedServerError> {
        self.status.crash_count = 0;
        self.spawn_server_process(profile_id, launch_spec)
    }

    fn spawn_server_process(
        &mut self,
        profile_id: &str,
        launch_spec: ServerLaunchSpec,
    ) -> Result<ManagedServerState, ManagedServerError> {
        if launch_spec.selected_java_major < launch_spec.required_java_major {
            self.status.state = ManagedServerState::Failed;
            return Err(ManagedServerError::JavaRuntimeRejected {
                required_major: launch_spec.required_java_major,
                found_major: Some(launch_spec.selected_java_major),
                reason: "selected Java runtime major version is below required major version"
                    .to_owned(),
            });
        }

        let eula_path = self.paths.resolve(&["servers", profile_id, "eula.txt"])?;
        if !self.is_eula_accepted(profile_id)? {
            self.status.state = ManagedServerState::Failed;
            let err = ManagedServerError::LicenseTermsNotAccepted {
                eula_path: eula_path.display().to_string(),
            };
            self.record_log_line(&err.to_string());
            return Err(err);
        }

        let srv_dir = self.server_dir(profile_id)?;
        let log_path = self.paths.resolve(&["servers", profile_id, "server.log"])?;
        let log_file = File::create(&log_path).map_err(|e| ManagedServerError::ProcessIo {
            reason: format!("failed to create server.log: {e}"),
        })?;
        let stderr_file = log_file
            .try_clone()
            .map_err(|e| ManagedServerError::ProcessIo {
                reason: format!("failed to clone server.log handle: {e}"),
            })?;

        let mut child = Command::new(&launch_spec.program)
            .args(&launch_spec.args)
            .current_dir(&srv_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| ManagedServerError::ProcessIo {
                reason: format!("failed to spawn managed server process: {e}"),
            })?;

        let stdin = child.stdin.take();
        self.last_launch_spec = Some(launch_spec.clone());
        self.running = Some(RunningServerProcess {
            child,
            stdin,
            log_path,
            log_offset: 0,
            reached_ready: false,
            launch_spec,
        });
        self.confirmed_player_ids.clear();
        self.confirmed_presence.confirmed_players = 0;
        self.status.profile_id = profile_id.to_owned();
        self.status.state = ManagedServerState::Starting;
        Ok(self.status.state)
    }

    /// Poll the running server log and child process state (`SRV-007`, `SRV-011`).
    ///
    /// * Transitions `Starting -> Running` when [`MINECRAFT_JAVA_READY_MARKER`] (`Done (`) appears
    ///   in the server log.
    /// * Parses MSMP `minecraft:notification/players/joined` and `/left` notifications into
    ///   [`ConfirmedPresenceSnapshot`] (`SRV-011`).
    /// * If the process exits *before* the readiness line (`SRV-007` requirement (a)), transitions
    ///   directly to [`ManagedServerState::Failed`] and returns
    ///   [`ManagedServerError::RefusedBeforeReady`] without consuming crash-restart retries.
    /// * If the process exits unexpectedly *after* reaching `Running`, transitions to
    ///   [`ManagedServerState::Crashed`] and restarts automatically up to `max_crash_restarts`,
    ///   or transitions to [`ManagedServerState::Failed`] with
    ///   [`ManagedServerError::RestartLimitReached`].
    pub fn poll_process(&mut self) -> Result<ManagedServerState, ManagedServerError> {
        self.consume_new_log_lines();

        let Some(mut proc_state) = self.running.take() else {
            return Ok(self.status.state);
        };

        if proc_state.reached_ready && self.status.state == ManagedServerState::Starting {
            self.status.state = ManagedServerState::Running;
        }

        match proc_state.child.try_wait() {
            Ok(None) => {
                self.running = Some(proc_state);
                Ok(self.status.state)
            }
            Ok(Some(exit_status)) => {
                // Drain any final bytes written right before exit.
                self.running = Some(proc_state);
                self.consume_new_log_lines();
                let Some(proc_state) = self.running.take() else {
                    return Ok(self.status.state);
                };

                self.confirmed_player_ids.clear();
                self.confirmed_presence.confirmed_players = 0;

                if !proc_state.reached_ready {
                    // SRV-007 (a): a refused start exits 0 (e.g. EULA refusal or port bind failure).
                    // Never treat exit before readiness as a clean exit, and never retry blindly.
                    self.status.state = ManagedServerState::Failed;
                    return Err(ManagedServerError::RefusedBeforeReady {
                        exit_code: exit_status.code(),
                        log_excerpt: self.status.last_log_excerpt.clone(),
                    });
                }

                // Process was Running and exited unexpectedly.
                self.status.crash_count = self.status.crash_count.saturating_add(1);
                if self.status.crash_count > self.max_crash_restarts {
                    self.status.state = ManagedServerState::Failed;
                    return Err(ManagedServerError::RestartLimitReached {
                        attempts: self.status.crash_count,
                        log_excerpt: self.status.last_log_excerpt.clone(),
                    });
                }

                self.status.state = ManagedServerState::Crashed;
                let profile_id = self.status.profile_id.clone();
                let spec = proc_state.launch_spec;
                self.spawn_server_process(&profile_id, spec)
            }
            Err(e) => {
                self.status.state = ManagedServerState::Failed;
                Err(ManagedServerError::ProcessIo {
                    reason: format!("failed to poll managed server process: {e}"),
                })
            }
        }
    }

    /// Gracefully stop the managed server by writing `stop\n` to its held `stdin` pipe (`SRV-007`
    /// requirement (b)) and terminating the Windows process tree if it does not exit within
    /// `timeout` (`SRV-007` requirement (c)).
    pub fn stop_server(&mut self, timeout: Duration) -> Result<(), ManagedServerError> {
        let Some(mut proc_state) = self.running.take() else {
            return Ok(());
        };

        let pid = proc_state.child.id();
        if let Some(mut stdin) = proc_state.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
            let _ = stdin.flush();
            drop(stdin);
        }

        let deadline = Instant::now() + timeout;
        let mut exited = false;
        while Instant::now() < deadline {
            match proc_state.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }

        if !exited {
            terminate_process_tree(pid, &mut proc_state.child);
        } else {
            // Even on clean exit, ensure any auxiliary child JVM in the process tree is cleaned up
            // (`SRV-007` requirement (c)).
            kill_windows_process_tree_by_pid(pid);
        }

        // Read final shutdown/save log lines (`Saving worlds`, `Saving chunks`).
        self.running = Some(proc_state);
        self.consume_new_log_lines();
        let _ = self.running.take();

        self.confirmed_player_ids.clear();
        self.confirmed_presence.confirmed_players = 0;
        self.status.state = ManagedServerState::Configuring;
        Ok(())
    }

    /// Create a world backup under `servers/<profile_id>/backups/<backup_id>` (`SRV-008`,
    /// `contracts.md` §13 `backup_world`).
    pub fn backup_world(
        &mut self,
        profile_id: &str,
        backup_id: &str,
    ) -> Result<PathBuf, ManagedServerError> {
        let world_dir = self.paths.resolve(&["servers", profile_id, "world"])?;
        let backup_dir = self
            .paths
            .resolve(&["servers", profile_id, "backups", backup_id])?;

        if backup_dir.exists() {
            fs::remove_dir_all(&backup_dir).map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to clear existing backup directory: {e}"),
            })?;
        }
        copy_dir_bounded(&world_dir, &backup_dir)?;
        self.status.last_backup_id = Some(backup_id.to_owned());
        self.record_log_line(&format!("Created world backup {backup_id}"));
        Ok(backup_dir)
    }

    /// Restore a world backup from `servers/<profile_id>/backups/<backup_id>` (`SRV-008`,
    /// `contracts.md` §13 `restore_world`).
    pub fn restore_world(
        &mut self,
        profile_id: &str,
        backup_id: &str,
    ) -> Result<PathBuf, ManagedServerError> {
        if self.running.is_some() {
            self.stop_server(Duration::from_secs(5))?;
        }
        let backup_dir = self
            .paths
            .resolve(&["servers", profile_id, "backups", backup_id])?;
        if !backup_dir.is_dir() {
            return Err(ManagedServerError::ConfigurationInvalid {
                reason: format!("backup {backup_id:?} does not exist"),
            });
        }
        let world_dir = self.paths.resolve(&["servers", profile_id, "world"])?;
        if world_dir.exists() {
            fs::remove_dir_all(&world_dir).map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to clear world directory before restore: {e}"),
            })?;
        }
        copy_dir_bounded(&backup_dir, &world_dir)?;
        self.record_log_line(&format!("Restored world from backup {backup_id}"));
        Ok(world_dir)
    }

    /// Update the managed server artifact while preserving the previous `server.jar.bak` and
    /// `world/` data (`SRV-008`, `SRV-009`, `contracts.md` §13 `update_managed_server`).
    ///
    /// * Refuses if `tunnel_is_active && !confirm_disconnect` (`SRV-009`).
    /// * Verifies `new_artifact` **before** touching the existing `server.jar` (`SRV-002`, `SRV-009`);
    ///   if verification fails, the existing version and world remain untouched.
    pub fn update_server_artifact(
        &mut self,
        profile_id: &str,
        new_artifact: &ArtifactSource,
        new_version: &str,
        tunnel_is_active: bool,
        confirm_disconnect: bool,
    ) -> Result<(), ManagedServerError> {
        if tunnel_is_active && !confirm_disconnect {
            return Err(ManagedServerError::UpdateRequiresActiveTunnelConfirmation);
        }

        let prev_state = self.status.state;
        self.status.state = ManagedServerState::Updating;

        // Verify the candidate artifact BEFORE stopping or replacing anything (`SRV-002`, `SRV-009`).
        if let Err(err) = verify(new_artifact) {
            self.status.state = prev_state;
            let mapped = ManagedServerError::from(err);
            self.record_log_line(&mapped.to_string());
            return Err(mapped);
        }

        if self.running.is_some() {
            self.stop_server(Duration::from_secs(5))?;
        }

        let jar_path = self.paths.resolve(&["servers", profile_id, "server.jar"])?;
        let bak_path = self
            .paths
            .resolve(&["servers", profile_id, "server.jar.prev"])?;

        if jar_path.is_file() {
            fs::copy(&jar_path, &bak_path).map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to preserve previous server.jar: {e}"),
            })?;
        }

        if let Err(e) = fs::copy(&new_artifact.path, &jar_path) {
            // Roll back from backup if copy failed mid-way.
            if bak_path.is_file() {
                let _ = fs::copy(&bak_path, &jar_path);
            }
            self.status.state = ManagedServerState::Failed;
            return Err(ManagedServerError::DiskExhaustion {
                reason: format!("failed to install updated server.jar: {e}"),
            });
        }

        self.status.version = Some(new_version.to_owned());
        self.status.state = ManagedServerState::Configuring;
        self.record_log_line(&format!("Updated {profile_id} to version {new_version}"));
        Ok(())
    }

    /// Roll back `server.jar` to the preserved `server.jar.prev` while keeping `world/` intact
    /// (`SRV-008`, `SRV-009`).
    pub fn rollback_server_artifact(
        &mut self,
        profile_id: &str,
        previous_version: &str,
    ) -> Result<(), ManagedServerError> {
        if self.running.is_some() {
            self.stop_server(Duration::from_secs(5))?;
        }
        let jar_path = self.paths.resolve(&["servers", profile_id, "server.jar"])?;
        let bak_path = self
            .paths
            .resolve(&["servers", profile_id, "server.jar.prev"])?;
        if !bak_path.is_file() {
            return Err(ManagedServerError::ConfigurationInvalid {
                reason: "no previous server artifact backup found to roll back to".to_owned(),
            });
        }
        fs::copy(&bak_path, &jar_path).map_err(|e| ManagedServerError::DiskExhaustion {
            reason: format!("failed to restore previous server.jar: {e}"),
        })?;
        self.status.version = Some(previous_version.to_owned());
        self.status.state = ManagedServerState::Configuring;
        self.record_log_line(&format!(
            "Rolled back {profile_id} to version {previous_version}"
        ));
        Ok(())
    }

    /// Enable or disable MSMP confirmed-presence tracking (`SRV-011`, `research.md` RQ-13.4).
    pub fn set_confirmed_presence_enabled(&mut self, enabled: bool) {
        self.confirmed_presence.enabled = enabled;
        if !enabled {
            self.confirmed_player_ids.clear();
            self.confirmed_presence.confirmed_players = 0;
        }
    }

    /// Ingest a raw log or MSMP JSON-RPC line (`SRV-007`, `SRV-011`).
    pub fn ingest_log_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.contains(MINECRAFT_JAVA_READY_MARKER) {
            if let Some(proc_state) = self.running.as_mut() {
                proc_state.reached_ready = true;
            }
            if self.status.state == ManagedServerState::Starting {
                self.status.state = ManagedServerState::Running;
            }
        }
        if let Some(notification) = parse_msmp_player_notification(trimmed) {
            self.confirmed_presence.enabled = true;
            match notification {
                MsmpPlayerNotification::Joined { player_id } => {
                    if self.confirmed_player_ids.len() < MAX_CONFIRMED_PLAYERS {
                        self.confirmed_player_ids.insert(player_id);
                    }
                }
                MsmpPlayerNotification::Left { player_id } => {
                    self.confirmed_player_ids.remove(&player_id);
                }
            }
            self.confirmed_presence.confirmed_players =
                u32::try_from(self.confirmed_player_ids.len()).unwrap_or(u32::MAX);
        }
        self.record_log_line(trimmed);
    }

    /// Drain newly appended log lines from the running child process's `server.log`.
    pub fn consume_new_log_lines(&mut self) -> Vec<String> {
        let (log_path, offset) = match self.running.as_ref() {
            Some(p) => (p.log_path.clone(), p.log_offset),
            None => return Vec::new(),
        };

        let Ok(mut file) = File::open(&log_path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return Vec::new();
        }

        let mut buf = vec![0u8; MAX_LOG_EXCERPT_BYTES];
        let Ok(n) = file.read(&mut buf) else {
            return Vec::new();
        };
        if n == 0 {
            return Vec::new();
        }

        if let Some(p) = self.running.as_mut() {
            p.log_offset = offset.saturating_add(u64::try_from(n).unwrap_or(0));
        }

        let chunk = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
        let mut lines = Vec::new();
        for line in chunk.lines() {
            let clean = line.trim().to_owned();
            if !clean.is_empty() {
                self.ingest_log_line(&clean);
                lines.push(clean);
            }
        }
        lines
    }

    fn record_log_line(&mut self, line: &str) {
        let bounded: String = line.chars().take(MAX_LOG_EXCERPT_BYTES).collect();
        self.status.last_log_excerpt = bounded.clone();
        if self.recent_logs.len() >= MAX_RETAINED_LOG_LINES {
            let _ = self.recent_logs.pop_front();
        }
        self.recent_logs.push_back(bounded);
    }
}

impl Drop for ManagedServerSupervisor {
    fn drop(&mut self) {
        // SRV-007: stopping the app leaves no server process running.
        let _ = self.stop_server(Duration::from_millis(500));
    }
}

fn terminate_process_tree(pid: u32, child: &mut Child) {
    kill_windows_process_tree_by_pid(pid);
    let _ = child.kill();
    let _ = child.wait();
}

fn kill_windows_process_tree_by_pid(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

const MAX_BACKUP_FILES: usize = 512;

fn copy_dir_bounded(src: &Path, dst: &Path) -> Result<(), ManagedServerError> {
    fs::create_dir_all(dst).map_err(|e| ManagedServerError::DiskExhaustion {
        reason: format!("failed to create backup destination directory: {e}"),
    })?;
    let mut copied_files = 0usize;
    let entries = fs::read_dir(src).map_err(|e| ManagedServerError::DiskExhaustion {
        reason: format!("failed to read source directory for backup: {e}"),
    })?;
    for entry_res in entries {
        if copied_files >= MAX_BACKUP_FILES {
            return Err(ManagedServerError::DiskExhaustion {
                reason: "world directory exceeds bounded file count for backup".to_owned(),
            });
        }
        let entry = entry_res.map_err(|e| ManagedServerError::DiskExhaustion {
            reason: format!("failed to read directory entry: {e}"),
        })?;
        let file_type = entry
            .file_type()
            .map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to inspect file type: {e}"),
            })?;
        if file_type.is_file() {
            let dest_file = dst.join(entry.file_name());
            fs::copy(entry.path(), dest_file).map_err(|e| ManagedServerError::DiskExhaustion {
                reason: format!("failed to copy world file: {e}"),
            })?;
            copied_files = copied_files.saturating_add(1);
        }
    }
    Ok(())
}
