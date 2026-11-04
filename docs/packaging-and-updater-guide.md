# Per-User Installer, WebView2 Runtime, Signed Updater & Uninstaller Guide (`T-100`, `T-101`, `T-102`)

**Status:** Active (`M8`: `T-100`, `T-101`, `T-102`)  
**Requirements Covered:** `INST-001`, `INST-002`, `INST-003`, `INST-004`, `INST-005`, `SEC-012`, `SEC-014`, `SRV-010`  
**Implementation:** [`apps/desktop/src/packaging.rs`](../apps/desktop/src/packaging.rs), [`apps/desktop/src/main.rs`](../apps/desktop/src/main.rs), [`apps/desktop/gtt-desktop.exe.manifest`](../apps/desktop/gtt-desktop.exe.manifest)  
**Verification Suites:** [`apps/desktop/tests/packaging_lifecycle.rs`](../apps/desktop/tests/packaging_lifecycle.rs), [`scripts/verify_packaging_and_updater.py`](../scripts/verify_packaging_and_updater.py)

---

## 1. Per-User Installation & Zero-Elevation Policy (`T-100`, `INST-001`, `SEC-012`)

Game Tunnel Tooling (GTT) installs and operates strictly within a standard Windows user account without requesting UAC elevation (`SEC-012`):

- **Application Manifest (`apps/desktop/gtt-desktop.exe.manifest`):**
  Declares `<requestedExecutionLevel level="asInvoker" uiAccess="false" />` and Per-Monitor V2 DPI awareness (`PerMonitorV2, PerMonitor`). Both the installer (`PerUserInstaller::install`) and the runtime self-check (`gtt-desktop.exe --self-check`) verify that the manifest never requests `requireAdministrator` or `highestAvailable`.
- **Directory Layout (`PerUserPaths`):**
  - **Program Root (`%LOCALAPPDATA%\Programs\GTT`):**
    - `slots/current/gtt-desktop.exe` — active verified executable
    - `slots/current/gtt-desktop.exe.manifest` — active Windows application manifest
    - `slots/current/slot_metadata.json` — version, SHA-256 digest, size, and signing key ID
    - `slots/previous/` — previous working version retained for atomic rollback (`INST-003`)
    - `slots/staging/` — ephemeral staging directory used during update verification (`INST-002`)
    - `install_receipt.json` — canonical installation and rollback state receipt
    - `runtime_state.json` — tracked active PIDs and loopback listener ports (`INST-004`)
  - **User Data Root (`%LOCALAPPDATA%\GTT`):**
    - `config/` — host configuration and non-secret preferences
    - `logs/` — structured diagnostic logs (`PRIV-002`)
    - `servers/` — managed game server directories and world backups (`SRV-003`, `SRV-010`)
- **System Directory Guard:**
  `validate_unprivileged_path` rejects any installation or uninstall target inside `C:\Program Files`, `C:\Program Files (x86)`, `C:\Windows`, or `C:\ProgramData` with `PackagingError::PrivilegeEscalationRejected`.

---

## 2. Evergreen WebView2 Runtime Detection & Remediation (`T-100`, `INST-005`)

`WebView2Probe` (`gtt-desktop.exe --verify-webview2`) inspects the host for the Microsoft Edge Evergreen WebView2 runtime (`{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`):

1. **Per-User Registry:** `HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` (`pv` value != `"0.0.0.0"`).
2. **Per-Machine Registry:** `HKLM\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` and `HKLM\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`.
3. **Edge Application Fallback:** `msedge.exe` app-mode availability on standard Windows 10/11 installations.
4. **Missing Runtime Handling (`WebView2Status::Missing`):**
   If no WebView2 runtime is present, `WebView2Probe::evaluate` returns the official Microsoft user-mode Evergreen bootstrapper URL (`https://go.microsoft.com/fwlink/p/?LinkId=2124703`) and clear, plain-language remediation instructions that do not require administrator credentials (`INST-005`).

---

## 3. Signed Updates, Tamper Rejection & Atomic Rollback (`T-101`, `INST-002`, `INST-003`, `SEC-014`)

`UpdaterEngine` enforces cryptographic authenticity and atomic slot promotion:

1. **Pinned Release Key Ring (`TrustedUpdateKeyRing`):**
   Only manifests signed by a `UpdateKeyStatus::Trusted` key ID are accepted; unknown or `Revoked` key IDs fail immediately with `PackagingError::UntrustedUpdateKeyId` (`SEC-014`).
2. **Domain-Separated Signature Verification:**
   Before reading or staging the binary payload, `UpdaterEngine::verify_update_package` recomputes the RFC 2104 HMAC-SHA256 tag over:
   `gtt-update-v1|{version}|{artifact_filename}|{artifact_size_bytes}|{sha256_hex}|{min_webview2_version}`
   and compares it in constant time (`constant_time_eq_32`).
3. **Monotonic Version & SHA-256 Digest Check:**
   - Rejects any candidate version `<= current_version` with `PackagingError::DowngradeOrReplayRejected` (`SEC-014`).
   - Verifies exact byte length and SHA-256 digest of the update binary before staging (`PackagingError::ArtifactHashMismatch`).
4. **Atomic Promotion & Automatic Rollback (`INST-003`):**
   - Verified artifacts are written to `slots/staging/`.
   - If an update is interrupted during staging (`UpdateSimulationFault::InterruptDuringStaging`), `slots/staging/` is cleaned up and `slots/current/` remains untouched at the previous version.
   - During promotion, `slots/current` moves to `slots/previous` and `slots/staging` moves to `slots/current`.
   - Immediately after promotion, `verify_slot_integrity` validates the promoted executable and manifest. If the post-promotion health check fails (`UpdateSimulationFault::FailPostPromotionHealthCheck`), `UpdaterEngine::rollback_to_previous` automatically restores `slots/previous` back into `slots/current` and records `last_rollback_reason` in `install_receipt.json`, leaving the old version intact and launchable.

---

## 4. Clean Uninstallation & User Data Policy (`T-102`, `INST-004`, `SRV-010`)

`Uninstaller::uninstall` (`gtt-desktop.exe --uninstall-per-user <dir> [--remove-user-data]`) guarantees clean removal:

1. **Process & Listener Shutdown (`INST-004`):**
   Reads `runtime_state.json`, invokes shutdown hooks, terminates any remaining tracked child PIDs, and probes every tracked loopback port (`127.0.0.1:{port}`) to confirm no listener remains bound.
2. **Program Files Removal:**
   Deletes `%LOCALAPPDATA%\Programs\GTT` completely (`install_root_removed: true`).
3. **Optional User Data Removal (`INST-004`, `SRV-010`):**
   - By default (`remove_user_data: false`), `%LOCALAPPDATA%\GTT` (including `servers/` world saves and backups) is **preserved** so uninstalling or reinstalling GTT never destroys a host's game world (`SRV-010`).
   - When `--remove-user-data` (`remove_user_data: true`) is passed, `%LOCALAPPDATA%\GTT` is also deleted (`user_data_removed: true`).
