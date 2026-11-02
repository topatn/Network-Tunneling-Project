# WebAuthn Admin Client Enforcement Spike (T-049 / ADR-019)

**Status:** Completed and Verified.  
**Authoritative Sources:** `contracts.md` §9, `requirements.md` (SEC-018, SEC-015), `research.md` ADR-019.

---

## 1. Objective and Problem Statement

`SEC-018` and ADR-019 require that the sole administrator identity (the project owner) authenticate using **WebAuthn (passkeys) with no TOTP fallback**, with short-lived sessions and step-up re-authentication before destructive operations (`suspend_user`, `unsuspend_user`, `terminate_tunnel`, `drain_relay`).

`T-049` called for an empirical spike to confirm:
1. Whether WebAuthn can actually be enforced on standard desktop and browser admin clients without degraded fallbacks.
2. How platform authenticators (Windows Hello, hardware FIDO2 keys) behave under strict `userVerification: "required"` constraints.
3. How cloned authenticator detection operates via monotonic sign counters.
4. How offline recovery codes function as a verified, single-use disaster recovery exception to `SEC-002` without leaking secrets to logs or audit trails (`INV-5`).

---

## 2. Client-Side Feasibility Analysis

### Browser and Platform Support
W3C WebAuthentication Level 3 (WebAuthn) and FIDO2 CTAP2 are universally supported across all target administrative client environments:
- **Windows 10/11:** Native Windows Hello platform authenticator (PIN, fingerprint, facial recognition) via WebAuthn API in Microsoft Edge, Google Chrome, and Mozilla Firefox. Hardware USB/NFC security keys (e.g. YubiKey 5 Series) supported natively via Windows WebAuthn APIs.
- **macOS / iOS:** Touch ID / Face ID via Apple Passkeys and iCloud Keychain.
- **Linux:** Hardware security keys via `libfido2` and `pcscd` in Chromium and Firefox.
- **Desktop WebView2 (Edge Chromium):** Standard `navigator.credentials.create({publicKey: ...})` and `navigator.credentials.get({publicKey: ...})` are supported out of the box with zero native C++ glue required.

### Enforcement Mechanism
The control plane enforces WebAuthn by requiring a valid `WebAuthnAssertion` on `/admin/v1/auth/login` and `/admin/v1/auth/step-up`. The server verifies:
1. **Challenge Freshness:** 256-bit cryptographically random challenge generated via OS CSPRNG, expiring after 300 seconds (`challenge_ttl_secs`).
2. **ClientDataJSON Validation:**
   - `type` must strictly equal `"webauthn.get"`.
   - `challenge` must match the issued challenge in constant time.
   - `origin` must strictly match the configured administrative origin (`https://admin.tunnel.internal`).
3. **Authenticator Data Flags:**
   - Bit 0 (`0x01`): User Present (UP) — must be set.
   - Bit 2 (`0x04`): User Verified (UV) — must be set (`userVerification: "required"`). Assertions without biometric or PIN verification return `403 user_verification_required`.
4. **Monotonic Counter Check (`SEC-018`):**
   - The 32-bit sign counter inside `authenticatorData` (bytes 33..37) must be strictly greater than the stored `passkey_counter`.
   - Any counter rollback or duplicate counter returns `403 authenticator_cloned`, instantly mitigating cloned authenticator threats.
5. **Credential ID and Signature Verification:**
   - `credential_id` must match `admins.passkey_credential_id`.
   - The signature over `authenticator_data || sha256(client_data_json)` must verify against `admins.passkey_public_key`.

---

## 3. Step-Up Re-Authentication Flow

Destructive operations (`suspend_user`, `unsuspend_user`, `terminate_tunnel`, `drain_relay`) can have catastrophic impact across all tenants. To prevent accidental operations or session hijacking:
1. Admin sessions are short-lived (15-minute TTL).
2. When calling a destructive endpoint, the server checks `is_session_stepped_up(&session, now)`.
3. If `step_up_until` is absent or expired, the request is rejected immediately with HTTP `403 forbidden`:
   ```json
   {
     "error": {
       "code": "step_up_required",
       "message": "Destructive operation requires recent step-up re-authentication."
     }
   }
   ```
4. To step up, the administrator requests a fresh challenge from `POST /admin/v1/auth/challenge` and submits `POST /admin/v1/auth/step-up` with a fresh biometric/PIN passkey assertion.
5. Upon successful verification, `step_up_until` is set to `now + 300 seconds` (5-minute window), allowing destructive calls until expiry.

---

## 4. Offline Recovery Path (ADR-019 / INV-5)

Because there is a single administrator and no secondary operator team, loss of the passkey would permanently lock out the service. The recovery architecture provides an offline recovery mechanism:
1. **Secret Generation:** An offline recovery code (`gtt_rc_` + 32 base64url characters, 256 bits entropy) is generated during initial admin bootstrapping.
2. **Storage at Rest:** Only `hash_recovery_code(code)` (`SHA-256`) is stored in `admins.recovery_code_hash`.
3. **Redemption:** The administrator submits the code to `POST /admin/v1/auth/recovery`.
4. **Constant-Time Verification:** The code is hashed and verified using constant-time comparison against `admins.recovery_code_hash`.
5. **Single-Use Enforcement:** `admins.recovery_used_at` is updated atomically. Any subsequent attempt to use the code returns HTTP `400 validation_failed` (`recovery_exhausted`).
6. **Leak Prevention (`INV-5`):** The redemption writes an `admin.recovery_used` audit event with `metadata: {}`. The recovery code string is never written to logs, database, or audit trails.
7. **Emergency Session:** Redemption grants a 15-minute emergency session to enroll a new passkey credential.

---

## 5. Security Boundary Verification Summary

| Threat / Test Case | Mitigation / Control | Status |
|---|---|---|
| User access token presented to `/admin/v1/*` | Strict rejection returning HTTP `403 forbidden` (`contracts.md` §9.1) | Verified |
| Replay of previous WebAuthn assertion | Bounded challenge TTL (300s) + monotonic counter check (`counter > stored_counter`) | Verified |
| Cloned authenticator device | Counter check fails with `authenticator_cloned` (HTTP 403) | Verified |
| Missing biometric / PIN verification | Authenticator flag UV (`0x04`) checked; fails with `user_verification_required` | Verified |
| Destructive action on unauthenticated session | Returns HTTP `401 unauthenticated` | Verified |
| Destructive action without recent passkey verification | Returns HTTP `403 step_up_required` | Verified |
| Recovery code reuse attempt | Single-use consumption flag (`recovery_used_at`) returns `recovery_exhausted` | Verified |
| Recovery code in audit logs | Redacted / zero metadata written (`INV-5`) | Verified |
