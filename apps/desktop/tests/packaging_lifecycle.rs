//! Integration tests for the per-user installer (`T-100`), WebView2 runtime detection (`T-100`),
//! signed update pipeline with atomic rollback (`T-101`), and clean uninstaller (`T-102`).
//!
//! Requirements exercised:
//! - `INST-001`, `SEC-012`: Per-user installation under `%LOCALAPPDATA%\Programs\GTT`,
//!   `requestedExecutionLevel="asInvoker"`, and rejection of privileged system directories.
//! - `INST-005`: Evergreen WebView2 runtime detection and missing-runtime remediation.
//! - `INST-002`, `INST-003`, `SEC-014`: Cryptographic signature & SHA-256 verification before
//!   applying updates, rejection of tampered/unsigned/downgrade packages, and automatic rollback
//!   on interrupted staging or failed post-promotion health check.
//! - `INST-004`, `SRV-010`: Uninstall stops active loopback listeners and tracked processes,
//!   removes `%LOCALAPPDATA%\Programs\GTT`, and preserves `%LOCALAPPDATA%\GTT` (managed servers,
//!   world backups, logs) unless `remove_user_data` is explicitly enabled.

use std::env;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gtt_desktop::packaging::{
    is_loopback_port_listening, sha256_hex, validate_unprivileged_path, verify_manifest_as_invoker,
    verify_slot_integrity, ActiveRuntimeState, PackagingError, PerUserInstaller, PerUserPaths,
    SignedUpdateManifest, TrustedUpdateKeyRing, UninstallOptions, Uninstaller, UpdateKeyStatus,
    UpdateSimulationFault, UpdaterEngine, WebView2Probe, WebView2Source, WebView2Status,
    APPLICATION_MANIFEST_XML, WEBVIEW2_BOOTSTRAPPER_URL, WEBVIEW2_EVERGREEN_CLIENT_GUID,
};

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

struct ScopedTempDir {
    path: PathBuf,
}

impl ScopedTempDir {
    fn new(label: &str) -> Result<Self, String> {
        let seq = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = env::temp_dir().join(format!("gtt-packaging-test-{label}-{pid}-{seq}"));
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }
}

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn t100_per_user_installer_enforces_as_invoker_and_rejects_privileged_paths() -> Result<(), String>
{
    // 1. Verify shipped manifest declares `asInvoker` and rejects `requireAdministrator` (SEC-012).
    verify_manifest_as_invoker(APPLICATION_MANIFEST_XML).map_err(|e| e.to_string())?;

    let bad_admin_manifest =
        r#"<requestedExecutionLevel level="requireAdministrator" uiAccess="false" />"#;
    if verify_manifest_as_invoker(bad_admin_manifest).is_ok() {
        return Err("expected requireAdministrator manifest to be rejected".to_owned());
    }

    // 2. Verify privileged system directories are rejected (INST-001, SEC-012).
    for forbidden in [
        "C:\\Program Files\\GTT",
        "C:\\Program Files (x86)\\GTT",
        "C:\\Windows\\System32\\GTT",
        "C:\\ProgramData\\GTT",
    ] {
        let path = PathBuf::from(forbidden);
        match validate_unprivileged_path(&path) {
            Err(PackagingError::PrivilegeEscalationRejected(_)) => {}
            other => {
                return Err(format!(
                    "expected PrivilegeEscalationRejected for {forbidden}, got {other:?}"
                ))
            }
        }
    }

    // 3. Install into an isolated standard-user `%LOCALAPPDATA%` directory.
    let tmp = ScopedTempDir::new("t100-install")?;
    let paths = PerUserPaths::from_local_app_data(&tmp.path).map_err(|e| e.to_string())?;
    let fake_v1_binary = b"MZ-GTT-DESKTOP-V0.1.0-STANDARD-USER-BINARY";
    let probe = WebView2Probe {
        hkcu_version: Some("131.0.2903.70".to_owned()),
        hklm_version: None,
        edge_fallback_version: None,
    };

    let receipt = PerUserInstaller::install(&paths, "0.1.0", fake_v1_binary, &probe)
        .map_err(|e| e.to_string())?;

    if receipt.execution_level != "asInvoker" {
        return Err(format!(
            "unexpected execution_level: {}",
            receipt.execution_level
        ));
    }
    if receipt.current_slot.version != "0.1.0" {
        return Err(format!(
            "unexpected installed version: {}",
            receipt.current_slot.version
        ));
    }
    if receipt.current_slot.binary_sha256 != sha256_hex(fake_v1_binary) {
        return Err("installed binary SHA-256 mismatch".to_owned());
    }
    if !paths.current_executable_path().exists() || !paths.current_manifest_path().exists() {
        return Err("expected executable and manifest in slots/current".to_owned());
    }
    verify_slot_integrity(&paths, &receipt.current_slot).map_err(|e| e.to_string())?;

    Ok(())
}

#[test]
fn t100_webview2_detection_handles_present_and_missing_runtime() -> Result<(), String> {
    // 1. Per-user HKCU Evergreen WebView2 present.
    let present_hkcu = WebView2Probe {
        hkcu_version: Some("131.0.2903.70".to_owned()),
        hklm_version: None,
        edge_fallback_version: None,
    };
    match present_hkcu.evaluate() {
        WebView2Status::Available { version, source } => {
            if version != "131.0.2903.70" || source != WebView2Source::PerUserRegistry {
                return Err(format!("unexpected HKCU status: {version} {source:?}"));
            }
        }
        other => return Err(format!("expected Available, got {other:?}")),
    }

    // 2. Per-machine HKLM Evergreen WebView2 present when HKCU is `"0.0.0.0"`.
    let present_hklm = WebView2Probe {
        hkcu_version: Some("0.0.0.0".to_owned()),
        hklm_version: Some("130.0.2849.80".to_owned()),
        edge_fallback_version: None,
    };
    match present_hklm.evaluate() {
        WebView2Status::Available { version, source } => {
            if version != "130.0.2849.80" || source != WebView2Source::PerMachineRegistry {
                return Err(format!("unexpected HKLM status: {version} {source:?}"));
            }
        }
        other => return Err(format!("expected Available, got {other:?}")),
    }

    // 3. Missing WebView2 runtime returns user-mode Evergreen bootstrapper URL and clear instructions (INST-005).
    let missing_probe = WebView2Probe {
        hkcu_version: None,
        hklm_version: Some("0.0.0.0".to_owned()),
        edge_fallback_version: None,
    };
    match missing_probe.evaluate() {
        WebView2Status::Missing {
            client_guid,
            bootstrapper_url,
            instructions,
        } => {
            if client_guid != WEBVIEW2_EVERGREEN_CLIENT_GUID {
                return Err(format!("unexpected client_guid: {client_guid}"));
            }
            if bootstrapper_url != WEBVIEW2_BOOTSTRAPPER_URL {
                return Err(format!("unexpected bootstrapper_url: {bootstrapper_url}"));
            }
            if !instructions.contains("no administrator password required") {
                return Err(format!(
                    "instructions missing standard-user note: {instructions}"
                ));
            }
        }
        other => return Err(format!("expected Missing, got {other:?}")),
    }

    Ok(())
}

#[test]
fn t101_signed_update_rejects_tampered_artifacts_and_rolls_back_interrupted_updates(
) -> Result<(), String> {
    let tmp = ScopedTempDir::new("t101-updater")?;
    let paths = PerUserPaths::from_local_app_data(&tmp.path).map_err(|e| e.to_string())?;
    let v1_bytes = b"MZ-GTT-DESKTOP-V0.1.0-WORKING-RELEASE";
    let v2_bytes = b"MZ-GTT-DESKTOP-V0.2.0-SIGNED-UPGRADE-RELEASE";
    let v3_bytes = b"MZ-GTT-DESKTOP-V0.3.0-SIGNED-UPGRADE-RELEASE";

    let probe = WebView2Probe {
        hkcu_version: Some("131.0.2903.70".to_owned()),
        hklm_version: None,
        edge_fallback_version: None,
    };
    PerUserInstaller::install(&paths, "0.1.0", v1_bytes, &probe).map_err(|e| e.to_string())?;

    let trusted_key_id = "gtt-release-2026-q4";
    let trusted_key_material = [0x42u8; 32];
    let revoked_key_id = "gtt-release-2025-revoked";
    let revoked_key_material = [0x99u8; 32];

    let mut key_ring = TrustedUpdateKeyRing::new();
    key_ring.add_key(
        trusted_key_id,
        trusted_key_material,
        UpdateKeyStatus::Trusted,
    );
    key_ring.add_key(
        revoked_key_id,
        revoked_key_material,
        UpdateKeyStatus::Revoked,
    );

    let valid_v2_manifest = SignedUpdateManifest::sign_artifact(
        "0.2.0",
        "gtt-desktop.exe",
        v2_bytes,
        "120.0.0.0",
        trusted_key_id,
        &trusted_key_material,
    )
    .map_err(|e| e.to_string())?;

    // Case A: Tampered artifact payload bytes MUST be rejected before touching `slots/current` (SEC-014).
    let mut tampered_v2_bytes = v2_bytes.to_vec();
    if let Some(first) = tampered_v2_bytes.first_mut() {
        *first ^= 0xFF;
    }
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &valid_v2_manifest,
        &tampered_v2_bytes,
        &key_ring,
        UpdateSimulationFault::None,
    ) {
        Err(PackagingError::ArtifactHashMismatch { .. }) => {}
        other => {
            return Err(format!(
                "expected ArtifactHashMismatch for tampered payload, got {other:?}"
            ))
        }
    }

    // Case B: Tampered signature / metadata MUST be rejected (SEC-014).
    let mut forged_manifest = valid_v2_manifest.clone();
    forged_manifest.version = "0.9.9".to_owned();
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &forged_manifest,
        v2_bytes,
        &key_ring,
        UpdateSimulationFault::None,
    ) {
        Err(PackagingError::SignatureVerificationFailed) => {}
        other => {
            return Err(format!(
                "expected SignatureVerificationFailed for forged version, got {other:?}"
            ))
        }
    }

    // Case C: Revoked signing key MUST be rejected (SEC-014).
    let revoked_manifest = SignedUpdateManifest::sign_artifact(
        "0.2.0",
        "gtt-desktop.exe",
        v2_bytes,
        "120.0.0.0",
        revoked_key_id,
        &revoked_key_material,
    )
    .map_err(|e| e.to_string())?;
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &revoked_manifest,
        v2_bytes,
        &key_ring,
        UpdateSimulationFault::None,
    ) {
        Err(PackagingError::UntrustedUpdateKeyId(_)) => {}
        other => {
            return Err(format!(
                "expected UntrustedUpdateKeyId for revoked key, got {other:?}"
            ))
        }
    }

    // Verify `0.1.0` is completely untouched after all three rejected tampering attempts.
    let receipt_after_rejections =
        PerUserInstaller::read_receipt(&paths).map_err(|e| e.to_string())?;
    if receipt_after_rejections.current_slot.version != "0.1.0" {
        return Err("expected 0.1.0 to remain active after rejected updates".to_owned());
    }
    verify_slot_integrity(&paths, &receipt_after_rejections.current_slot)
        .map_err(|e| e.to_string())?;

    // Case D: Interrupted update mid-staging leaves `0.1.0` working (INST-003).
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &valid_v2_manifest,
        v2_bytes,
        &key_ring,
        UpdateSimulationFault::InterruptDuringStaging,
    ) {
        Err(PackagingError::UpdateInterruptedRolledBack {
            restored_version, ..
        }) => {
            if restored_version != "0.1.0" {
                return Err(format!(
                    "expected restored_version 0.1.0, got {restored_version}"
                ));
            }
        }
        other => {
            return Err(format!(
                "expected UpdateInterruptedRolledBack on staging interruption, got {other:?}"
            ))
        }
    }
    let receipt_after_staging_interrupt =
        PerUserInstaller::read_receipt(&paths).map_err(|e| e.to_string())?;
    verify_slot_integrity(&paths, &receipt_after_staging_interrupt.current_slot)
        .map_err(|e| e.to_string())?;

    // Case E: Clean signed update from `0.1.0` -> `0.2.0` succeeds (`INST-002`).
    let updated_receipt = UpdaterEngine::stage_and_apply_update(
        &paths,
        &valid_v2_manifest,
        v2_bytes,
        &key_ring,
        UpdateSimulationFault::None,
    )
    .map_err(|e| e.to_string())?;
    if updated_receipt.current_slot.version != "0.2.0" {
        return Err(format!(
            "expected active version 0.2.0, got {}",
            updated_receipt.current_slot.version
        ));
    }
    if updated_receipt
        .previous_slot
        .as_ref()
        .map(|s| s.version.as_str())
        != Some("0.1.0")
    {
        return Err("expected previous_slot to retain 0.1.0".to_owned());
    }
    verify_slot_integrity(&paths, &updated_receipt.current_slot).map_err(|e| e.to_string())?;

    // Case F: Downgrade attack (`0.2.0` -> `0.1.0`) is rejected (`SEC-014`).
    let downgrade_manifest = SignedUpdateManifest::sign_artifact(
        "0.1.0",
        "gtt-desktop.exe",
        v1_bytes,
        "120.0.0.0",
        trusted_key_id,
        &trusted_key_material,
    )
    .map_err(|e| e.to_string())?;
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &downgrade_manifest,
        v1_bytes,
        &key_ring,
        UpdateSimulationFault::None,
    ) {
        Err(PackagingError::DowngradeOrReplayRejected { .. }) => {}
        other => return Err(format!("expected DowngradeOrReplayRejected, got {other:?}")),
    }

    // Case G: Update `0.2.0` -> `0.3.0` that fails its post-promotion startup health check
    // automatically rolls back to `0.2.0` and leaves `0.2.0` working (`INST-003`).
    let valid_v3_manifest = SignedUpdateManifest::sign_artifact(
        "0.3.0",
        "gtt-desktop.exe",
        v3_bytes,
        "120.0.0.0",
        trusted_key_id,
        &trusted_key_material,
    )
    .map_err(|e| e.to_string())?;
    match UpdaterEngine::stage_and_apply_update(
        &paths,
        &valid_v3_manifest,
        v3_bytes,
        &key_ring,
        UpdateSimulationFault::FailPostPromotionHealthCheck,
    ) {
        Err(PackagingError::UpdateInterruptedRolledBack {
            restored_version, ..
        }) => {
            if restored_version != "0.2.0" {
                return Err(format!(
                    "expected rollback to restore 0.2.0, got {restored_version}"
                ));
            }
        }
        other => {
            return Err(format!(
                "expected UpdateInterruptedRolledBack on post-promotion failure, got {other:?}"
            ))
        }
    }

    let receipt_after_rollback =
        PerUserInstaller::read_receipt(&paths).map_err(|e| e.to_string())?;
    if receipt_after_rollback.current_slot.version != "0.2.0" {
        return Err("expected current_slot.version to be 0.2.0 after rollback".to_owned());
    }
    verify_slot_integrity(&paths, &receipt_after_rollback.current_slot)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[test]
fn t102_uninstall_stops_listeners_and_respects_user_data_preservation_policy() -> Result<(), String>
{
    let tmp = ScopedTempDir::new("t102-uninstall")?;
    let paths = PerUserPaths::from_local_app_data(&tmp.path).map_err(|e| e.to_string())?;
    let v1_bytes = b"MZ-GTT-DESKTOP-V0.1.0-UNINSTALL-TEST";
    let probe = WebView2Probe {
        hkcu_version: Some("131.0.2903.70".to_owned()),
        hklm_version: None,
        edge_fallback_version: None,
    };

    PerUserInstaller::install(&paths, "0.1.0", v1_bytes, &probe).map_err(|e| e.to_string())?;

    // Write a simulated managed server world file under `%LOCALAPPDATA%\GTT\servers\world-1\level.dat`
    // to verify `SRV-010` and `INST-004`.
    let world_file = paths
        .data_root
        .join("servers")
        .join("world-1")
        .join("level.dat");
    if let Some(parent) = world_file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&world_file, b"MANAGED-WORLD-SAVE-DATA").map_err(|e| e.to_string())?;

    // Bind a real loopback listener on `127.0.0.1:0` to simulate an active GTT desktop/engine listener.
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let mut active_listener = Some(listener);

    ActiveRuntimeState::record_activity(&paths, None, Some(port)).map_err(|e| e.to_string())?;

    // 1. Uninstall with `remove_user_data: false` (default: preserve worlds & backups per SRV-010).
    let report_preserve = Uninstaller::uninstall(
        &paths,
        UninstallOptions {
            remove_user_data: false,
        },
        |_state| {
            // Shutdown hook drops the active loopback listener so the port closes cleanly.
            active_listener = None;
        },
    )
    .map_err(|e| e.to_string())?;

    if !report_preserve.install_root_removed || paths.install_root.exists() {
        return Err("expected install_root to be removed during uninstall".to_owned());
    }
    if report_preserve.user_data_removed || !world_file.exists() {
        return Err(
            "expected user data and managed server world to be preserved when remove_user_data=false"
                .to_owned(),
        );
    }
    if is_loopback_port_listening(port) {
        return Err(format!(
            "expected loopback port {port} to be closed after uninstall"
        ));
    }

    // 2. Re-install and uninstall with `remove_user_data: true` (`--remove-user-data` per INST-004).
    PerUserInstaller::install(&paths, "0.1.0", v1_bytes, &probe).map_err(|e| e.to_string())?;
    let report_purge = Uninstaller::uninstall(
        &paths,
        UninstallOptions {
            remove_user_data: true,
        },
        |_state| {},
    )
    .map_err(|e| e.to_string())?;

    if !report_purge.install_root_removed || paths.install_root.exists() {
        return Err("expected install_root to be removed".to_owned());
    }
    if !report_purge.user_data_removed || paths.data_root.exists() {
        return Err("expected data_root to be removed when remove_user_data=true".to_owned());
    }

    Ok(())
}
