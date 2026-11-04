//! Per-user Windows installer, Evergreen WebView2 runtime detection, signed update staging
//! with atomic rollback, and clean uninstaller (`T-100`, `T-101`, `T-102`).
//!
//! ## Requirements Covered
//!
//! - `INST-001` & `SEC-012` (`T-100`): Installs per-user under `%LOCALAPPDATA%\Programs\GTT`
//!   and stores user data under `%LOCALAPPDATA%\GTT`. Requires `requestedExecutionLevel="asInvoker"`
//!   and rejects administrator-only target directories (`Program Files`, `Windows`, `System32`).
//! - `INST-005` (`T-100`): Detects the Microsoft Edge Evergreen WebView2 runtime
//!   (`{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`) in per-user or per-machine registry state and
//!   returns clear user-mode remediation instructions if missing.
//! - `INST-002`, `INST-003`, & `SEC-014` (`T-101`): Verifies cryptographic release signatures
//!   (HMAC-SHA256 over canonical release metadata + SHA-256 artifact digest) before staging or
//!   applying any update, rejects downgrades and tampered artifacts, and atomically rolls back
//!   to `slots/previous` if an update is interrupted or fails its post-promotion health check.
//! - `INST-004` & `SRV-010` (`T-102`): Stops all active GTT processes and loopback listeners,
//!   removes `%LOCALAPPDATA%\Programs\GTT`, and preserves `%LOCALAPPDATA%\GTT` (managed servers,
//!   world backups, logs, config) by default unless `remove_user_data` is explicitly enabled.

use std::fmt;
use std::fs;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Canonical Windows application manifest embedded in and shipped alongside `gtt-desktop.exe`
/// (`T-100`, `INST-001`, `SEC-012`).
pub const APPLICATION_MANIFEST_XML: &str = include_str!("../gtt-desktop.exe.manifest");

/// Official Microsoft Edge Evergreen WebView2 Runtime client GUID (`INST-005`).
pub const WEBVIEW2_EVERGREEN_CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// Official Microsoft Evergreen WebView2 user-mode bootstrapper URL (`INST-005`).
pub const WEBVIEW2_BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Maximum accepted update artifact size (256 MiB) to bound disk/memory usage (`AGENTS.md` §4).
pub const MAX_UPDATE_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of tracked active PIDs or listener ports in `runtime_state.json`.
pub const MAX_RUNTIME_ENTRIES: usize = 64;

/// Semantic version (`major.minor.patch`) with strict parsing and total ordering (`INST-002`, `SEC-014`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SemanticVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
}

impl SemanticVersion {
    /// Parse a strict `major.minor.patch` version string (optional leading `v` is rejected to
    /// keep manifest strings canonical).
    pub fn parse(raw: &str) -> Result<Self, PackagingError> {
        let trimmed = raw.trim();
        let mut parts = trimmed.split('.');
        let major_str = parts
            .next()
            .ok_or(PackagingError::InvalidVersion("missing major version"))?;
        let minor_str = parts
            .next()
            .ok_or(PackagingError::InvalidVersion("missing minor version"))?;
        let patch_str = parts
            .next()
            .ok_or(PackagingError::InvalidVersion("missing patch version"))?;
        if parts.next().is_some() {
            return Err(PackagingError::InvalidVersion(
                "version must have exactly three numeric segments (major.minor.patch)",
            ));
        }
        let major = major_str
            .parse::<u32>()
            .map_err(|_| PackagingError::InvalidVersion("invalid major version integer"))?;
        let minor = minor_str
            .parse::<u32>()
            .map_err(|_| PackagingError::InvalidVersion("invalid minor version integer"))?;
        let patch = patch_str
            .parse::<u32>()
            .map_err(|_| PackagingError::InvalidVersion("invalid patch version integer"))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for SemanticVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Errors produced by the installer, WebView2 probe, signed updater, or uninstaller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackagingError {
    /// Target path is empty or targets a privileged system directory (`SEC-012`).
    PrivilegeEscalationRejected(&'static str),
    /// Application manifest violates `requestedExecutionLevel="asInvoker"` (`SEC-012`).
    ManifestPolicyViolation(&'static str),
    /// Invalid semantic version string.
    InvalidVersion(&'static str),
    /// Filesystem I/O failure with context.
    IoError {
        /// High-level operation that failed.
        operation: &'static str,
        /// Underlying OS error description.
        detail: String,
    },
    /// JSON serialization or deserialization failure.
    SerializationError(String),
    /// Update key ID is unknown or revoked (`SEC-014`).
    UntrustedUpdateKeyId(String),
    /// Update signature verification failed (`SEC-014`).
    SignatureVerificationFailed,
    /// Update artifact SHA-256 digest or byte length did not match the signed manifest (`SEC-014`).
    ArtifactHashMismatch {
        /// Expected lowercase hex SHA-256 from signed manifest.
        expected_sha256: String,
        /// Actual lowercase hex SHA-256 computed over the artifact bytes.
        actual_sha256: String,
    },
    /// Update artifact size exceeded [`MAX_UPDATE_ARTIFACT_BYTES`] or disagreed with manifest.
    ArtifactSizeMismatch {
        /// Expected byte count in manifest.
        expected_bytes: u64,
        /// Actual byte count on disk.
        actual_bytes: u64,
    },
    /// Target update version is less than or equal to the currently installed version (`SEC-014`).
    DowngradeOrReplayRejected {
        /// Currently installed version.
        current_version: String,
        /// Candidate update version.
        candidate_version: String,
    },
    /// Update was interrupted during staging or failed post-promotion health check and was
    /// automatically rolled back to the previous working version (`INST-003`).
    UpdateInterruptedRolledBack {
        /// Version restored and verified working after rollback.
        restored_version: String,
        /// Cause of the update failure that triggered rollback.
        reason: String,
    },
    /// Rollback was requested, but no previous version slot was available.
    NoPreviousVersionAvailable,
    /// Uninstall detected a listener or process that remained active after shutdown attempts (`INST-004`).
    RemainingActiveResource(String),
}

impl fmt::Display for PackagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivilegeEscalationRejected(msg) => {
                write!(f, "per-user policy rejected privileged path (SEC-012): {msg}")
            }
            Self::ManifestPolicyViolation(msg) => {
                write!(f, "application manifest policy violation (SEC-012): {msg}")
            }
            Self::InvalidVersion(msg) => write!(f, "invalid semantic version: {msg}"),
            Self::IoError { operation, detail } => {
                write!(f, "packaging I/O error during {operation}: {detail}")
            }
            Self::SerializationError(detail) => {
                write!(f, "packaging metadata serialization error: {detail}")
            }
            Self::UntrustedUpdateKeyId(kid) => {
                write!(f, "untrusted or revoked update signing key_id (SEC-014): {kid}")
            }
            Self::SignatureVerificationFailed => {
                f.write_str("update cryptographic signature verification failed (SEC-014)")
            }
            Self::ArtifactHashMismatch {
                expected_sha256,
                actual_sha256,
            } => write!(
                f,
                "update artifact SHA-256 mismatch (SEC-014): expected {expected_sha256}, got {actual_sha256}"
            ),
            Self::ArtifactSizeMismatch {
                expected_bytes,
                actual_bytes,
            } => write!(
                f,
                "update artifact size mismatch: expected {expected_bytes} bytes, got {actual_bytes} bytes"
            ),
            Self::DowngradeOrReplayRejected {
                current_version,
                candidate_version,
            } => write!(
                f,
                "rejected downgrade or replay update from {current_version} to {candidate_version} (SEC-014)"
            ),
            Self::UpdateInterruptedRolledBack {
                restored_version,
                reason,
            } => write!(
                f,
                "update failed ({reason}); automatically rolled back to working version {restored_version} (INST-003)"
            ),
            Self::NoPreviousVersionAvailable => {
                f.write_str("no previous version slot is available for rollback")
            }
            Self::RemainingActiveResource(detail) => {
                write!(f, "active process or listener remained after uninstall shutdown: {detail}")
            }
        }
    }
}

impl std::error::Error for PackagingError {}

/// Per-user installation and data directories (`T-100`, `INST-001`, `SEC-012`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerUserPaths {
    /// Per-user program installation directory (`%LOCALAPPDATA%\Programs\GTT`).
    pub install_root: PathBuf,
    /// Per-user application data directory (`%LOCALAPPDATA%\GTT`).
    pub data_root: PathBuf,
}

impl PerUserPaths {
    /// Construct and validate per-user directories rooted under a standard user's
    /// `%LOCALAPPDATA%` base directory (`INST-001`, `SEC-012`).
    pub fn from_local_app_data(local_app_data: &Path) -> Result<Self, PackagingError> {
        validate_unprivileged_path(local_app_data)?;
        let install_root = local_app_data.join("Programs").join("GTT");
        let data_root = local_app_data.join("GTT");
        validate_unprivileged_path(&install_root)?;
        validate_unprivileged_path(&data_root)?;
        Ok(Self {
            install_root,
            data_root,
        })
    }

    /// Construct and validate explicit `install_root` and `data_root` paths.
    pub fn new(install_root: PathBuf, data_root: PathBuf) -> Result<Self, PackagingError> {
        validate_unprivileged_path(&install_root)?;
        validate_unprivileged_path(&data_root)?;
        Ok(Self {
            install_root,
            data_root,
        })
    }

    /// Active version slot directory (`install_root/slots/current`).
    pub fn current_slot_dir(&self) -> PathBuf {
        self.install_root.join("slots").join("current")
    }

    /// Previous version slot directory (`install_root/slots/previous`) used for atomic rollback (`INST-003`).
    pub fn previous_slot_dir(&self) -> PathBuf {
        self.install_root.join("slots").join("previous")
    }

    /// Staging slot directory (`install_root/slots/staging`) used during update verification (`INST-002`).
    pub fn staging_slot_dir(&self) -> PathBuf {
        self.install_root.join("slots").join("staging")
    }

    /// Path to the active executable (`install_root/slots/current/gtt-desktop.exe`).
    pub fn current_executable_path(&self) -> PathBuf {
        self.current_slot_dir().join("gtt-desktop.exe")
    }

    /// Path to the active Windows application manifest (`install_root/slots/current/gtt-desktop.exe.manifest`).
    pub fn current_manifest_path(&self) -> PathBuf {
        self.current_slot_dir().join("gtt-desktop.exe.manifest")
    }

    /// Path to `install_receipt.json` (`install_root/install_receipt.json`).
    pub fn install_receipt_path(&self) -> PathBuf {
        self.install_root.join("install_receipt.json")
    }

    /// Path to `runtime_state.json` (`install_root/runtime_state.json`) tracking active PIDs and listeners.
    pub fn runtime_state_path(&self) -> PathBuf {
        self.install_root.join("runtime_state.json")
    }
}

/// Reject paths that point into system-wide or administrator-protected Windows locations (`SEC-012`).
pub fn validate_unprivileged_path(path: &Path) -> Result<(), PackagingError> {
    let raw = path.to_string_lossy();
    if raw.trim().is_empty() {
        return Err(PackagingError::PrivilegeEscalationRejected(
            "target path must not be empty",
        ));
    }
    let normalized = raw.replace('/', "\\").to_ascii_lowercase();
    let forbidden_prefixes = [
        "c:\\program files\\",
        "c:\\program files (x86)\\",
        "c:\\windows\\",
        "c:\\programdata\\",
    ];
    for prefix in forbidden_prefixes {
        if normalized.starts_with(prefix) || normalized == prefix.trim_end_matches('\\') {
            return Err(PackagingError::PrivilegeEscalationRejected(
                "system-wide Program Files, ProgramData, and Windows directories require administrator elevation and are forbidden by INST-001 / SEC-012",
            ));
        }
    }
    Ok(())
}

/// Verify that a Windows application manifest XML enforces `requestedExecutionLevel level="asInvoker"`
/// and does not request administrator elevation (`T-100`, `INST-001`, `SEC-012`).
pub fn verify_manifest_as_invoker(manifest_xml: &str) -> Result<(), PackagingError> {
    if manifest_xml.contains("requireAdministrator") || manifest_xml.contains("highestAvailable") {
        return Err(PackagingError::ManifestPolicyViolation(
            "manifest must not request requireAdministrator or highestAvailable",
        ));
    }
    if !manifest_xml.contains("requestedExecutionLevel")
        || !manifest_xml.contains("level=\"asInvoker\"")
        || !manifest_xml.contains("uiAccess=\"false\"")
    {
        return Err(PackagingError::ManifestPolicyViolation(
            "manifest must declare <requestedExecutionLevel level=\"asInvoker\" uiAccess=\"false\" />",
        ));
    }
    Ok(())
}

/// Source where the Evergreen WebView2 runtime was detected (`INST-005`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebView2Source {
    /// Per-user registry key (`HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`).
    PerUserRegistry,
    /// Per-machine registry key (`HKLM\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\...`).
    PerMachineRegistry,
    /// Microsoft Edge Chromium application fallback binary on disk.
    EdgeApplicationFallback,
}

/// Result of probing for the Microsoft Edge Evergreen WebView2 runtime (`T-100`, `INST-005`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WebView2Status {
    /// WebView2 runtime is present and ready.
    Available {
        /// Detected WebView2 / Edge version string.
        version: String,
        /// Location where the runtime was found.
        source: WebView2Source,
    },
    /// WebView2 runtime is missing on this machine; provides user-mode bootstrapper remediation.
    Missing {
        /// Evergreen WebView2 client GUID probed.
        client_guid: String,
        /// Official Microsoft user-mode bootstrapper URL (`https://go.microsoft.com/fwlink/p/?LinkId=2124703`).
        bootstrapper_url: String,
        /// Clear, non-jargon user instructions (`INST-005`).
        instructions: String,
    },
}

impl WebView2Status {
    /// Returns `true` if a usable WebView2 runtime was detected.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Configurable probe inputs for deterministic WebView2 detection across unit tests, staging
/// exercises, and live Windows hosts (`T-100`, `INST-005`).
#[derive(Debug, Clone, Default)]
pub struct WebView2Probe {
    /// Simulated or queried `pv` value under `HKCU\Software\Microsoft\EdgeUpdate\Clients\{GUID}`.
    pub hkcu_version: Option<String>,
    /// Simulated or queried `pv` value under `HKLM\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\{GUID}`.
    pub hklm_version: Option<String>,
    /// Simulated or detected Microsoft Edge executable version fallback.
    pub edge_fallback_version: Option<String>,
}

impl WebView2Probe {
    /// Query the live Windows host for Evergreen WebView2 registry entries and Edge fallback paths.
    pub fn from_host_system() -> Self {
        let hkcu_key = format!(
            "HKCU\\Software\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_EVERGREEN_CLIENT_GUID}"
        );
        let hklm_wow64_key = format!(
            "HKLM\\Software\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_EVERGREEN_CLIENT_GUID}"
        );
        let hklm_native_key = format!(
            "HKLM\\Software\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_EVERGREEN_CLIENT_GUID}"
        );

        let hkcu_version = query_windows_registry_pv(&hkcu_key);
        let hklm_version = query_windows_registry_pv(&hklm_wow64_key)
            .or_else(|| query_windows_registry_pv(&hklm_native_key));

        let edge_paths = [
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
            "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        ];
        let edge_fallback_version = edge_paths
            .iter()
            .find(|p| Path::new(p).exists())
            .map(|_| "edge-app-mode-available".to_owned());

        Self {
            hkcu_version,
            hklm_version,
            edge_fallback_version,
        }
    }

    /// Evaluate the probe and return a structured [`WebView2Status`] (`INST-005`).
    pub fn evaluate(&self) -> WebView2Status {
        if let Some(ver) = self
            .hkcu_version
            .as_deref()
            .and_then(non_empty_runtime_version)
        {
            return WebView2Status::Available {
                version: ver.to_owned(),
                source: WebView2Source::PerUserRegistry,
            };
        }
        if let Some(ver) = self
            .hklm_version
            .as_deref()
            .and_then(non_empty_runtime_version)
        {
            return WebView2Status::Available {
                version: ver.to_owned(),
                source: WebView2Source::PerMachineRegistry,
            };
        }
        if let Some(ver) = self
            .edge_fallback_version
            .as_deref()
            .and_then(non_empty_runtime_version)
        {
            return WebView2Status::Available {
                version: ver.to_owned(),
                source: WebView2Source::EdgeApplicationFallback,
            };
        }

        WebView2Status::Missing {
            client_guid: WEBVIEW2_EVERGREEN_CLIENT_GUID.to_owned(),
            bootstrapper_url: WEBVIEW2_BOOTSTRAPPER_URL.to_owned(),
            instructions: format!(
                "Microsoft Edge WebView2 Runtime is required to display the GTT desktop window. \
                 Download and run the standard-user Evergreen WebView2 Bootstrapper from \
                 {WEBVIEW2_BOOTSTRAPPER_URL} (no administrator password required), then launch GTT again."
            ),
        }
    }
}

fn non_empty_runtime_version(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "0.0.0.0" {
        None
    } else {
        Some(trimmed)
    }
}

fn query_windows_registry_pv(key_path: &str) -> Option<String> {
    let output = Command::new("reg")
        .args(["query", key_path, "/v", "pv"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pv") {
            let mut parts = trimmed.split_whitespace();
            let _name = parts.next()?;
            let _reg_type = parts.next()?;
            let value = parts.next()?;
            return non_empty_runtime_version(value).map(ToOwned::to_owned);
        }
    }
    None
}

/// Metadata written to `install_root/slots/{current,previous}/slot_metadata.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotMetadata {
    /// Installed semantic version in this slot (`e.g. "0.1.0"`).
    pub version: String,
    /// Lowercase hex SHA-256 of `gtt-desktop.exe` in this slot.
    pub binary_sha256: String,
    /// Byte size of `gtt-desktop.exe` in this slot.
    pub binary_size_bytes: u64,
    /// Signing key ID that authorized this version (`"initial-install"` for initial install).
    pub key_id: String,
}

/// Persistent installation receipt stored at `install_root/install_receipt.json` (`T-100`, `T-101`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Schema version (`1`).
    pub schema_version: u32,
    /// Execution privilege level verified during install (`"asInvoker"`).
    pub execution_level: String,
    /// Per-user installation root path.
    pub install_root: String,
    /// Per-user data root path.
    pub data_root: String,
    /// Currently active slot metadata.
    pub current_slot: SlotMetadata,
    /// Previous slot metadata available for rollback, if any (`INST-003`).
    pub previous_slot: Option<SlotMetadata>,
    /// Detected WebView2 status at install/update time (`INST-005`).
    pub webview2_status: WebView2Status,
    /// Reason recorded if the most recent update attempt triggered an automatic rollback (`INST-003`).
    pub last_rollback_reason: Option<String>,
}

/// Per-user installer engine (`T-100`, `INST-001`, `INST-005`, `SEC-012`).
pub struct PerUserInstaller;

impl PerUserInstaller {
    /// Perform a per-user installation of `binary_bytes` at `version` into `paths` without
    /// requiring administrator rights (`INST-001`, `SEC-012`).
    pub fn install(
        paths: &PerUserPaths,
        version: &str,
        binary_bytes: &[u8],
        webview2_probe: &WebView2Probe,
    ) -> Result<InstallReceipt, PackagingError> {
        let parsed_version = SemanticVersion::parse(version)?;
        validate_unprivileged_path(&paths.install_root)?;
        validate_unprivileged_path(&paths.data_root)?;
        verify_manifest_as_invoker(APPLICATION_MANIFEST_XML)?;

        if binary_bytes.is_empty() || (binary_bytes.len() as u64) > MAX_UPDATE_ARTIFACT_BYTES {
            return Err(PackagingError::ArtifactSizeMismatch {
                expected_bytes: 1,
                actual_bytes: binary_bytes.len() as u64,
            });
        }

        let current_dir = paths.current_slot_dir();
        create_dir_all_checked(&current_dir, "create current slot directory")?;
        create_dir_all_checked(&paths.data_root.join("config"), "create data config dir")?;
        create_dir_all_checked(&paths.data_root.join("logs"), "create data logs dir")?;
        create_dir_all_checked(&paths.data_root.join("servers"), "create data servers dir")?;

        let exe_path = paths.current_executable_path();
        write_file_checked(&exe_path, binary_bytes, "write gtt-desktop.exe")?;

        let manifest_path = paths.current_manifest_path();
        write_file_checked(
            &manifest_path,
            APPLICATION_MANIFEST_XML.as_bytes(),
            "write gtt-desktop.exe.manifest",
        )?;

        let binary_sha256 = sha256_hex(binary_bytes);
        let slot_meta = SlotMetadata {
            version: parsed_version.to_string(),
            binary_sha256,
            binary_size_bytes: binary_bytes.len() as u64,
            key_id: "initial-install".to_owned(),
        };
        write_json_checked(
            &current_dir.join("slot_metadata.json"),
            &slot_meta,
            "write current slot_metadata.json",
        )?;

        let webview2_status = webview2_probe.evaluate();
        let receipt = InstallReceipt {
            schema_version: 1,
            execution_level: "asInvoker".to_owned(),
            install_root: paths.install_root.to_string_lossy().into_owned(),
            data_root: paths.data_root.to_string_lossy().into_owned(),
            current_slot: slot_meta,
            previous_slot: None,
            webview2_status,
            last_rollback_reason: None,
        };
        write_json_checked(
            &paths.install_receipt_path(),
            &receipt,
            "write install_receipt.json",
        )?;

        let initial_runtime = ActiveRuntimeState::default();
        write_json_checked(
            &paths.runtime_state_path(),
            &initial_runtime,
            "write runtime_state.json",
        )?;

        Ok(receipt)
    }

    /// Read and validate the existing [`InstallReceipt`] from `paths`.
    pub fn read_receipt(paths: &PerUserPaths) -> Result<InstallReceipt, PackagingError> {
        read_json_checked(&paths.install_receipt_path(), "read install_receipt.json")
    }
}

/// Lifecycle state of a release signing key in [`TrustedUpdateKeyRing`] (`SEC-014`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKeyStatus {
    /// Key is trusted for verifying signed updates.
    Trusted,
    /// Key has been revoked and must be rejected immediately (`SEC-014`).
    Revoked,
}

/// A pinned update verification key entry (`T-101`, `SEC-014`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateVerificationKey {
    /// Key identifier (`e.g. "gtt-release-2026-q4"`).
    pub key_id: String,
    /// 32-byte key material used for HMAC-SHA256 release signature verification.
    pub material: [u8; 32],
    /// Key trust status.
    pub status: UpdateKeyStatus,
}

/// Pinned key ring used by the updater to authenticate signed update manifests (`T-101`, `SEC-014`).
#[derive(Debug, Clone, Default)]
pub struct TrustedUpdateKeyRing {
    keys: Vec<UpdateVerificationKey>,
}

impl TrustedUpdateKeyRing {
    /// Create an empty key ring.
    pub fn new() -> Self {
        Self { keys: Vec::new() }
    }

    /// Register a verification key in the key ring.
    pub fn add_key(
        &mut self,
        key_id: impl Into<String>,
        material: [u8; 32],
        status: UpdateKeyStatus,
    ) {
        self.keys.push(UpdateVerificationKey {
            key_id: key_id.into(),
            material,
            status,
        });
    }

    /// Look up a trusted (non-revoked) key by `key_id`.
    pub fn find_trusted_key(&self, key_id: &str) -> Option<&UpdateVerificationKey> {
        self.keys
            .iter()
            .find(|k| k.key_id == key_id && k.status == UpdateKeyStatus::Trusted)
    }
}

/// Signed update manifest describing a candidate release artifact (`T-101`, `INST-002`, `SEC-014`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedUpdateManifest {
    /// Target semantic version (`major.minor.patch`).
    pub version: String,
    /// Relative filename of the update binary in the update bundle (`e.g. "gtt-desktop.exe"`).
    pub artifact_filename: String,
    /// Exact byte length of the update binary.
    pub artifact_size_bytes: u64,
    /// Lowercase hex SHA-256 digest of the update binary.
    pub sha256_hex: String,
    /// Signing key identifier (`kid`) from [`TrustedUpdateKeyRing`].
    pub key_id: String,
    /// Lowercase hex HMAC-SHA256 signature over the canonical manifest signing string.
    pub signature_hex: String,
    /// Minimum required Evergreen WebView2 version string.
    pub min_webview2_version: String,
}

impl SignedUpdateManifest {
    /// Canonical domain-separated signing input (`SEC-014`).
    pub fn canonical_signing_input(&self) -> String {
        format!(
            "gtt-update-v1|{}|{}|{}|{}|{}",
            self.version.trim(),
            self.artifact_filename.trim(),
            self.artifact_size_bytes,
            self.sha256_hex.trim().to_ascii_lowercase(),
            self.min_webview2_version.trim()
        )
    }

    /// Create and sign an update manifest for `artifact_bytes` using `(key_id, key_material)`.
    pub fn sign_artifact(
        version: &str,
        artifact_filename: &str,
        artifact_bytes: &[u8],
        min_webview2_version: &str,
        key_id: &str,
        key_material: &[u8; 32],
    ) -> Result<Self, PackagingError> {
        let parsed = SemanticVersion::parse(version)?;
        let sha256 = sha256_hex(artifact_bytes);
        let mut manifest = Self {
            version: parsed.to_string(),
            artifact_filename: artifact_filename.to_owned(),
            artifact_size_bytes: artifact_bytes.len() as u64,
            sha256_hex: sha256,
            key_id: key_id.to_owned(),
            signature_hex: String::new(),
            min_webview2_version: min_webview2_version.to_owned(),
        };
        let signing_input = manifest.canonical_signing_input();
        let tag = hmac_sha256(key_material, signing_input.as_bytes());
        manifest.signature_hex = bytes_to_hex(&tag);
        Ok(manifest)
    }
}

/// Fault-injection and health-check policy for staging and post-promotion verification (`T-101`, `INST-003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateSimulationFault {
    /// Normal update execution: run real binary integrity/self-check verification.
    #[default]
    None,
    /// Simulate an interrupted write / crash mid-staging before slot promotion (`INST-003`).
    InterruptDuringStaging,
    /// Simulate the newly promoted binary failing its startup health check (`INST-003`).
    FailPostPromotionHealthCheck,
}

/// Signed updater and atomic rollback engine (`T-101`, `INST-002`, `INST-003`, `SEC-014`).
pub struct UpdaterEngine;

impl UpdaterEngine {
    /// Cryptographically verify `manifest` and `artifact_bytes` against `key_ring` and
    /// `current_version` without modifying disk state (`SEC-014`).
    pub fn verify_update_package(
        manifest: &SignedUpdateManifest,
        artifact_bytes: &[u8],
        key_ring: &TrustedUpdateKeyRing,
        current_version: SemanticVersion,
    ) -> Result<SemanticVersion, PackagingError> {
        let candidate_version = SemanticVersion::parse(&manifest.version)?;

        // 1. Verify key_id is trusted and not revoked (SEC-014).
        let verification_key = key_ring
            .find_trusted_key(&manifest.key_id)
            .ok_or_else(|| PackagingError::UntrustedUpdateKeyId(manifest.key_id.clone()))?;

        // 2. Verify cryptographic signature in constant time BEFORE inspecting payload (SEC-014).
        let provided_sig = hex_to_32_bytes(&manifest.signature_hex)
            .ok_or(PackagingError::SignatureVerificationFailed)?;
        let expected_sig = hmac_sha256(
            &verification_key.material,
            manifest.canonical_signing_input().as_bytes(),
        );
        if !constant_time_eq_32(&provided_sig, &expected_sig) {
            return Err(PackagingError::SignatureVerificationFailed);
        }

        // 3. Reject downgrades or same-version replays (SEC-014).
        if candidate_version <= current_version {
            return Err(PackagingError::DowngradeOrReplayRejected {
                current_version: current_version.to_string(),
                candidate_version: candidate_version.to_string(),
            });
        }

        // 4. Verify artifact byte length matches manifest and bounds.
        let actual_size = artifact_bytes.len() as u64;
        if actual_size == 0
            || actual_size > MAX_UPDATE_ARTIFACT_BYTES
            || actual_size != manifest.artifact_size_bytes
        {
            return Err(PackagingError::ArtifactSizeMismatch {
                expected_bytes: manifest.artifact_size_bytes,
                actual_bytes: actual_size,
            });
        }

        // 5. Verify SHA-256 digest of artifact bytes matches signed manifest (SEC-014).
        let actual_sha256 = sha256_hex(artifact_bytes);
        let expected_sha256 = manifest.sha256_hex.trim().to_ascii_lowercase();
        let expected_digest_bytes =
            hex_to_32_bytes(&expected_sha256).ok_or(PackagingError::SignatureVerificationFailed)?;
        let actual_digest_bytes = sha256_digest(artifact_bytes);
        if !constant_time_eq_32(&expected_digest_bytes, &actual_digest_bytes) {
            return Err(PackagingError::ArtifactHashMismatch {
                expected_sha256,
                actual_sha256,
            });
        }

        Ok(candidate_version)
    }

    /// Stage, verify, promote, and health-check a signed update (`T-101`, `INST-002`, `INST-003`, `SEC-014`).
    ///
    /// - If cryptographic verification fails (`SEC-014`), no slot is modified and `slots/staging` is removed.
    /// - If staging is interrupted or the new binary fails its post-promotion health check (`INST-003`),
    ///   `slots/previous` is atomically restored into `slots/current` and `UpdateInterruptedRolledBack`
    ///   is returned with the old version verified working.
    pub fn stage_and_apply_update(
        paths: &PerUserPaths,
        manifest: &SignedUpdateManifest,
        artifact_bytes: &[u8],
        key_ring: &TrustedUpdateKeyRing,
        fault: UpdateSimulationFault,
    ) -> Result<InstallReceipt, PackagingError> {
        let mut receipt = PerUserInstaller::read_receipt(paths)?;
        let current_version = SemanticVersion::parse(&receipt.current_slot.version)?;

        // Clean any leftover staging directory before starting.
        let staging_dir = paths.staging_slot_dir();
        if staging_dir.exists() {
            let _ = fs::remove_dir_all(&staging_dir);
        }

        // Perform full cryptographic + version + hash verification before writing anything.
        let candidate_version =
            Self::verify_update_package(manifest, artifact_bytes, key_ring, current_version)?;

        create_dir_all_checked(&staging_dir, "create update staging directory")?;
        let staged_exe = staging_dir.join("gtt-desktop.exe");
        let staged_manifest = staging_dir.join("gtt-desktop.exe.manifest");

        if fault == UpdateSimulationFault::InterruptDuringStaging {
            // Write partial/truncated staging file to simulate power loss or process kill mid-staging,
            // then clean up staging and keep `slots/current` untouched.
            let partial_len = artifact_bytes.len() / 2;
            let partial_slice = artifact_bytes.get(..partial_len).unwrap_or(&[]);
            let _ = fs::write(&staged_exe, partial_slice);
            let _ = fs::remove_dir_all(&staging_dir);

            receipt.last_rollback_reason =
                Some("update interrupted during staging; kept current slot intact".to_owned());
            write_json_checked(
                &paths.install_receipt_path(),
                &receipt,
                "record staging interruption in install_receipt.json",
            )?;

            return Err(PackagingError::UpdateInterruptedRolledBack {
                restored_version: receipt.current_slot.version,
                reason: "interrupted during staging write".to_owned(),
            });
        }

        write_file_checked(&staged_exe, artifact_bytes, "write staged gtt-desktop.exe")?;
        write_file_checked(
            &staged_manifest,
            APPLICATION_MANIFEST_XML.as_bytes(),
            "write staged gtt-desktop.exe.manifest",
        )?;

        let new_slot_meta = SlotMetadata {
            version: candidate_version.to_string(),
            binary_sha256: manifest.sha256_hex.trim().to_ascii_lowercase(),
            binary_size_bytes: manifest.artifact_size_bytes,
            key_id: manifest.key_id.clone(),
        };
        write_json_checked(
            &staging_dir.join("slot_metadata.json"),
            &new_slot_meta,
            "write staged slot_metadata.json",
        )?;

        // Rotate `slots/current` -> `slots/previous`, then `slots/staging` -> `slots/current`.
        let current_dir = paths.current_slot_dir();
        let previous_dir = paths.previous_slot_dir();
        if previous_dir.exists() {
            remove_dir_all_checked(&previous_dir, "clear old previous slot")?;
        }
        rename_checked(&current_dir, &previous_dir, "move current slot to previous")?;
        if let Err(promote_err) = rename_checked(
            &staging_dir,
            &current_dir,
            "promote staging slot to current",
        ) {
            // Restore previous -> current immediately if promotion failed.
            let _ = fs::rename(&previous_dir, &current_dir);
            return Err(promote_err);
        }

        let previous_slot_meta = receipt.current_slot.clone();
        receipt.previous_slot = Some(previous_slot_meta.clone());
        receipt.current_slot = new_slot_meta;
        receipt.last_rollback_reason = None;
        write_json_checked(
            &paths.install_receipt_path(),
            &receipt,
            "update install_receipt.json after promotion",
        )?;

        // Post-promotion health verification (INST-003).
        let health_ok = match fault {
            UpdateSimulationFault::FailPostPromotionHealthCheck => false,
            UpdateSimulationFault::None | UpdateSimulationFault::InterruptDuringStaging => {
                verify_slot_integrity(paths, &receipt.current_slot).is_ok()
            }
        };

        if !health_ok {
            let reason = "post-promotion startup health check failed";
            let rolled_back = Self::rollback_to_previous(paths, reason)?;
            return Err(PackagingError::UpdateInterruptedRolledBack {
                restored_version: rolled_back.current_slot.version,
                reason: reason.to_owned(),
            });
        }

        Ok(receipt)
    }

    /// Roll back `slots/current` to `slots/previous` and update `install_receipt.json` (`T-101`, `INST-003`).
    pub fn rollback_to_previous(
        paths: &PerUserPaths,
        reason: &str,
    ) -> Result<InstallReceipt, PackagingError> {
        let mut receipt = PerUserInstaller::read_receipt(paths)?;
        let previous_meta = receipt
            .previous_slot
            .take()
            .ok_or(PackagingError::NoPreviousVersionAvailable)?;

        let current_dir = paths.current_slot_dir();
        let previous_dir = paths.previous_slot_dir();
        if !previous_dir.exists() {
            return Err(PackagingError::NoPreviousVersionAvailable);
        }

        if current_dir.exists() {
            remove_dir_all_checked(&current_dir, "remove failed current slot during rollback")?;
        }
        rename_checked(
            &previous_dir,
            &current_dir,
            "restore previous slot to current",
        )?;

        receipt.current_slot = previous_meta;
        receipt.last_rollback_reason = Some(reason.to_owned());
        write_json_checked(
            &paths.install_receipt_path(),
            &receipt,
            "write install_receipt.json after rollback",
        )?;

        verify_slot_integrity(paths, &receipt.current_slot)?;
        Ok(receipt)
    }
}

/// Verify that the active slot binary and manifest exist, match `asInvoker`, and match the
/// recorded SHA-256 and byte length in `slot_meta`.
pub fn verify_slot_integrity(
    paths: &PerUserPaths,
    slot_meta: &SlotMetadata,
) -> Result<(), PackagingError> {
    let exe_bytes =
        fs::read(paths.current_executable_path()).map_err(|e| PackagingError::IoError {
            operation: "read active slot executable",
            detail: e.to_string(),
        })?;
    if (exe_bytes.len() as u64) != slot_meta.binary_size_bytes {
        return Err(PackagingError::ArtifactSizeMismatch {
            expected_bytes: slot_meta.binary_size_bytes,
            actual_bytes: exe_bytes.len() as u64,
        });
    }
    let actual_sha256 = sha256_hex(&exe_bytes);
    if actual_sha256 != slot_meta.binary_sha256 {
        return Err(PackagingError::ArtifactHashMismatch {
            expected_sha256: slot_meta.binary_sha256.clone(),
            actual_sha256,
        });
    }
    let manifest_xml =
        fs::read_to_string(paths.current_manifest_path()).map_err(|e| PackagingError::IoError {
            operation: "read active slot manifest",
            detail: e.to_string(),
        })?;
    verify_manifest_as_invoker(&manifest_xml)
}

/// Runtime state persisted at `install_root/runtime_state.json` so the uninstaller can stop
/// all active GTT processes and verify no loopback listeners remain (`T-102`, `INST-004`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveRuntimeState {
    /// Active GTT desktop, engine, or managed-server child PIDs.
    pub active_pids: Vec<u32>,
    /// Active loopback TCP ports (`127.0.0.1:{port}`) bound by GTT.
    pub listener_ports: Vec<u16>,
}

impl ActiveRuntimeState {
    /// Register an active PID and/or loopback listener port in `runtime_state.json`.
    pub fn record_activity(
        paths: &PerUserPaths,
        pid: Option<u32>,
        listener_port: Option<u16>,
    ) -> Result<Self, PackagingError> {
        let mut state: Self = if paths.runtime_state_path().exists() {
            read_json_checked(&paths.runtime_state_path(), "read runtime_state.json")?
        } else {
            Self::default()
        };
        if let Some(p) = pid {
            if !state.active_pids.contains(&p) && state.active_pids.len() < MAX_RUNTIME_ENTRIES {
                state.active_pids.push(p);
            }
        }
        if let Some(port) = listener_port {
            if !state.listener_ports.contains(&port)
                && state.listener_ports.len() < MAX_RUNTIME_ENTRIES
            {
                state.listener_ports.push(port);
            }
        }
        write_json_checked(
            &paths.runtime_state_path(),
            &state,
            "write runtime_state.json",
        )?;
        Ok(state)
    }
}

/// User options controlling uninstallation (`T-102`, `INST-004`, `SRV-010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UninstallOptions {
    /// Whether to also delete `%LOCALAPPDATA%\GTT` (managed servers, world backups, logs, config).
    /// Defaults to `false` so worlds and backups are preserved unless the user explicitly opts in (`SRV-010`).
    pub remove_user_data: bool,
}

/// Summary report returned after uninstallation completes (`T-102`, `INST-004`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallReport {
    /// PIDs that were terminated during uninstall shutdown.
    pub terminated_pids: Vec<u32>,
    /// Loopback ports verified closed (`127.0.0.1:{port}`).
    pub verified_closed_ports: Vec<u16>,
    /// Whether `install_root` (`%LOCALAPPDATA%\Programs\GTT`) was completely removed.
    pub install_root_removed: bool,
    /// Whether `data_root` (`%LOCALAPPDATA%\GTT`) was removed (`true`) or preserved (`false`).
    pub user_data_removed: bool,
}

/// Uninstaller engine (`T-102`, `INST-004`, `SRV-010`).
pub struct Uninstaller;

impl Uninstaller {
    /// Stop all registered GTT processes and listeners, verify no tracked loopback listeners
    /// remain bound, remove `install_root`, and optionally remove `data_root` (`INST-004`, `SRV-010`).
    pub fn uninstall<F>(
        paths: &PerUserPaths,
        options: UninstallOptions,
        mut shutdown_hook: F,
    ) -> Result<UninstallReport, PackagingError>
    where
        F: FnMut(&ActiveRuntimeState),
    {
        validate_unprivileged_path(&paths.install_root)?;
        validate_unprivileged_path(&paths.data_root)?;

        let runtime_state: ActiveRuntimeState = if paths.runtime_state_path().exists() {
            read_json_checked(&paths.runtime_state_path(), "read runtime_state.json")?
        } else {
            ActiveRuntimeState::default()
        };

        // 1. Invoke caller shutdown hook (allows cleanly dropping in-process test listeners/children).
        shutdown_hook(&runtime_state);

        // 2. Terminate any OS processes recorded in `runtime_state.active_pids` (excluding self).
        let self_pid = std::process::id();
        let mut terminated_pids = Vec::new();
        for &pid in &runtime_state.active_pids {
            if pid != 0 && pid != self_pid {
                terminate_os_process(pid);
                terminated_pids.push(pid);
            }
        }

        // 3. Verify that every tracked loopback listener port (`127.0.0.1:{port}`) is closed.
        let mut verified_closed_ports = Vec::new();
        for &port in &runtime_state.listener_ports {
            if is_loopback_port_listening(port) {
                return Err(PackagingError::RemainingActiveResource(format!(
                    "loopback listener 127.0.0.1:{port} is still accepting connections"
                )));
            }
            verified_closed_ports.push(port);
        }

        // 4. Remove `install_root` (`%LOCALAPPDATA%\Programs\GTT`).
        if paths.install_root.exists() {
            remove_dir_all_checked(&paths.install_root, "remove per-user install_root")?;
        }
        let install_root_removed = !paths.install_root.exists();

        // 5. Remove `data_root` (`%LOCALAPPDATA%\GTT`) only when `options.remove_user_data` is true.
        let user_data_removed = if options.remove_user_data {
            if paths.data_root.exists() {
                remove_dir_all_checked(&paths.data_root, "remove per-user data_root")?;
            }
            !paths.data_root.exists()
        } else {
            false
        };

        Ok(UninstallReport {
            terminated_pids,
            verified_closed_ports,
            install_root_removed,
            user_data_removed,
        })
    }
}

/// Check whether `127.0.0.1:{port}` has an active listener accepting TCP connections.
pub fn is_loopback_port_listening(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for attempt in 0..3 {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok() {
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(40));
                continue;
            }
            return true;
        }
        return false;
    }
    false
}

fn terminate_os_process(pid: u32) {
    let pid_str = pid.to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", &pid_str, "/T", "/F"])
        .output();
}

/// Compute SHA-256 digest of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Compute lowercase hex SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    bytes_to_hex(&sha256_digest(bytes))
}

/// Compute RFC 2104 HMAC-SHA256 using `sha2::Sha256`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        for (dst, src) in key_block.iter_mut().zip(hashed.iter()) {
            *dst = *src;
        }
    } else {
        for (dst, src) in key_block.iter_mut().zip(key.iter()) {
            *dst = *src;
        }
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for (i, k) in key_block.iter().enumerate() {
        if let Some(ip) = ipad.get_mut(i) {
            *ip ^= *k;
        }
        if let Some(op) = opad.get_mut(i) {
            *op ^= *k;
        }
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let outer_hash = outer.finalize();

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer_hash);
    out
}

/// Constant-time comparison of two 32-byte arrays (`SEC-014`).
pub fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// Encode a byte slice as lowercase hexadecimal.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &b in bytes {
        let hi = usize::from(b >> 4);
        let lo = usize::from(b & 0x0F);
        if let Some(&ch) = HEX_LOWER.get(hi) {
            out.push(char::from(ch));
        }
        if let Some(&ch) = HEX_LOWER.get(lo) {
            out.push(char::from(ch));
        }
    }
    out
}

/// Decode a 64-character hex string into a 32-byte array.
pub fn hex_to_32_bytes(hex: &str) -> Option<[u8; 32]> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let (pairs, rem) = trimmed.as_bytes().as_chunks::<2>();
    if !rem.is_empty() {
        return None;
    }
    for (idx, &[hi_ch, lo_ch]) in pairs.iter().enumerate() {
        let hi = decode_hex_nibble(hi_ch)?;
        let lo = decode_hex_nibble(lo_ch)?;
        if let Some(slot) = out.get_mut(idx) {
            *slot = (hi << 4) | lo;
        }
    }
    Some(out)
}

fn decode_hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn create_dir_all_checked(path: &Path, operation: &'static str) -> Result<(), PackagingError> {
    fs::create_dir_all(path).map_err(|e| PackagingError::IoError {
        operation,
        detail: e.to_string(),
    })
}

fn write_file_checked(
    path: &Path,
    bytes: &[u8],
    operation: &'static str,
) -> Result<(), PackagingError> {
    fs::write(path, bytes).map_err(|e| PackagingError::IoError {
        operation,
        detail: e.to_string(),
    })
}

fn write_json_checked<T: Serialize>(
    path: &Path,
    value: &T,
    operation: &'static str,
) -> Result<(), PackagingError> {
    let serialized = serde_json::to_vec_pretty(value)
        .map_err(|e| PackagingError::SerializationError(e.to_string()))?;
    write_file_checked(path, &serialized, operation)
}

fn read_json_checked<T: for<'de> Deserialize<'de>>(
    path: &Path,
    operation: &'static str,
) -> Result<T, PackagingError> {
    let bytes = fs::read(path).map_err(|e| PackagingError::IoError {
        operation,
        detail: e.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| PackagingError::SerializationError(e.to_string()))
}

fn rename_checked(from: &Path, to: &Path, operation: &'static str) -> Result<(), PackagingError> {
    fs::rename(from, to).map_err(|e| PackagingError::IoError {
        operation,
        detail: e.to_string(),
    })
}

fn remove_dir_all_checked(path: &Path, operation: &'static str) -> Result<(), PackagingError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(PackagingError::IoError {
            operation,
            detail: e.to_string(),
        }),
    }
}
