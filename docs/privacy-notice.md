# Privacy Notice & Encryption Disclosure (T-110)

**Status:** APPROVED & AUTHORITATIVE.
**Authoritative Requirements:** `PRIV-005` (Privacy notice lists relay visibility, stored metadata, no guest accounts, retention periods), `SEC-011` (Encryption limits and relay inspection capabilities disclosed).
**Linked References:** `architecture.md` §18, `requirements.md` (PRIV-001 to PRIV-006, SEC-011), `data-model.md` §4, `research.md` ADR-004, ADR-007.
**Date:** 2026-11-02.

---

## 1. Executive Summary & Plain Language Overview

This application allows game hosts to share their locally running game servers with friends across the internet without router port forwarding or public IP exposure.

To provide this service reliably and safely:
1. **The Relay Sees Game Traffic (Default Model):** The connection between your game server and the relay is protected by encrypted QUIC/TLS. However, guest players connect to the relay using standard, unencrypted game protocols (Minecraft, Terraria, Valve Source). **This service does NOT provide end-to-end encryption between guests and your host.** The relay terminates connections and forwards packets, meaning game payloads are technically visible to the relay operator in the default guest model (`SEC-011`, `PRIV-005`).
2. **Your Local Network is Private:** The relay and control plane never receive, store, or query your local LAN IP address or the local port your server runs on (`INV-1`).
3. **No Guest Accounts:** Guests do not create accounts, provide emails, or install software. They connect using an out-of-band invitation link (`PRIV-005`).
4. **Data Minimization:** Guest IP addresses are masked (`/24` for IPv4, `/48` for IPv6) before being visible to the host or recorded in audit trails (`PRIV-002`). Zero diagnostic telemetry or usage trackers exist (`PRIV-001`).

---

## 2. What Data is Handled & Where

| Data Category | What is Transmitted / Stored | Visibility / Storage Location | Retention Period |
|---|---|---|---|
| **Account Data** | Identity Provider Subject ID, Display Name | PostgreSQL database (`users` table). **Zero email addresses stored (`PRIV-006`).** | Active account lifetime + 30 days after deletion |
| **Local Game Target** | Local IP (e.g., `127.0.0.1`), Local Port (e.g., `25565`) | **Never transmitted.** Kept strictly host-local in engine memory on the host's PC (`INV-1`). | Memory only during active process |
| **Tunnel Metadata** | Tunnel Name, Game Profile ID, Public Ports | PostgreSQL (`tunnels`, `allocation_ports`). Visible to tunnel owner. | 30 days post-stop (`PRIV-003`) |
| **Game Traffic** | Raw TCP streams and UDP datagrams | Forwarded in-memory by relay. **Never recorded or saved to disk.** | Ephemeral in-flight memory only; 0 seconds retention |
| **Guest Admissions** | Guest Source IP (canonicalized for firewalling) | Relay RAM; masked (`a.b.c.0/24`) in control plane (`guest_admissions`). | 24 hours post-expiration or revocation |
| **Invitation Secrets** | Cryptographic random tokens (`gtt_inv_...`) | Domain-separated SHA-256 hash stored in DB (`token_hash`). Plaintext returned once to host. | 30 days post-stop |
| **Usage Metrics** | Aggregated byte and flow counts per tunnel | PostgreSQL (`usage_records`). No packet contents. | 90 days rolling window |
| **Audit Logs** | Security and administrative actions (logins, tunnel starts/stops, revocations) | PostgreSQL (`audit_events`). Secrets redacted; guest IPs masked. | 90 days rolling window |

---

## 3. Encryption Limits & Security Disclosure (`SEC-011`)

### 3.1 Network Transport Architecture
```
[ Guest Player ] ── (Game Protocol: Plaintext TCP/UDP) ──▶ [ Public Relay Node ]
                                                                 │
                                                       (Encrypted QUIC / TLS 1.3)
                                                                 │
                                                                 ▼
                                                       [ Host Desktop App ]
                                                                 │
                                                        (Localhost Loopback)
                                                                 │
                                                                 ▼
                                                       [ Local Game Server ]
```

### 3.2 Formal Security Disclosure
- **Host-to-Relay Channel:** Fully authenticated and encrypted using QUIC with TLS 1.3 (`ALPN: gtt/1`). The host desktop engine validates the relay's TLS certificate against the registered relay hostname before sending allocation tokens (`SEC-010`).
- **Guest-to-Relay Channel:** Guest players connect to public relay ports using standard commercial game clients (e.g., Minecraft, Terraria, Left 4 Dead 2). Because standard game clients do not support custom tunnel transport protocols, traffic between the guest and the relay uses the game's native transport. These protocols are unencrypted.
- **Relay Role:** The relay decrypts the host's QUIC tunnel traffic and multiplexes game streams between the host and admitted guests. **The relay is an application-level proxy, not an opaque VPN.**
- **No E2EE:** End-to-end encryption between guests and host is **not provided** in this release (`ADR-004`). If your game server protocol transmits sensitive credentials (such as an unencrypted in-game RCON password or plaintext chat passwords), those strings traverse the relay in plaintext.
- **Recommendations for Hosts:** Never enable game server RCON on forwarded ports. Generated game server configurations disable RCON and JMX management ports by default (`ADR-015`).

---

## 4. Retention Schedule & Scheduled Cleanup (`PRIV-003`, `PRIV-004`)

Data retained in PostgreSQL is subject to automated background sweeps executing bounded batch purges (`crates/control-plane/src/db/cleanup.rs`):

1. **Guest Admissions (`guest_admissions`):** Automatically deleted **24 hours** after expiration or revocation.
2. **Idempotency Keys (`idempotency_keys`):** Automatically deleted **24 hours** after creation.
3. **Authentication Sessions (`auth_sessions`):** Automatically deleted **7 days** after expiration or revocation.
4. **Terminal Tunnels (`tunnels` `stopped`/`terminated`/`expired`):** Automatically deleted **30 days** post-stop, cascading to mappings, allocations, ports, and invitations.
5. **Usage Records (`usage_records`):** Aggregated hourly records deleted **90 days** from interval start.
6. **Audit Events (`audit_events`):** Security audit events deleted **90 days** from occurrence.
7. **Soft-Deleted User Accounts:** When a user requests account deletion, the account is marked `deleted_at = now()`, immediately revoking sessions and terminating tunnels. All user records are permanently purged after **30 days**.

---

## 5. Standard In-App UI Copy & Disclosure Strings

These approved copy strings must be embedded verbatim across client application touchpoints (`DESIGN.md`, `user-flows.md`):

### 5.1 Onboarding / First Run Screen
> **How Your Game Traffic is Handled**
>
> We connect your friends to your game server through high-speed relay servers so you don't have to configure your home router or expose your home IP address.
>
> **What you should know:**
> - **Encrypted Tunnel:** The connection between your computer and our relay is fully encrypted (TLS 1.3).
> - **Game Traffic Visibility:** Because standard game clients (like Minecraft and Terraria) connect without tunnel encryption, game traffic is visible to the relay while in transit. This is not end-to-end encrypted.
> - **Privacy Guarantee:** We never see or store your local IP address or port, guest IP addresses are masked, and we never sell or log your gameplay.

### 5.2 Sharing / Invitation Dialog Callout
> **🔒 Privacy & Encryption Note**
>
> Guests connect to a public relay address using an invitation link. Traffic between guests and the relay uses standard game networking. Do not enter passwords or sensitive personal details in in-game chat while using public relays.

### 5.3 Settings & Privacy Screen
> **Data & Retention Policy**
> - **Guest Connections:** Kept for 24 hours after leaving, then permanently deleted.
> - **Tunnel History:** Automatically purged 30 days after stopping.
> - **Zero Tracking:** No tracking cookies, advertising IDs, or behavioral analytics are collected.
> - [Read our full Privacy Notice & Technical Architecture →](docs/privacy-notice.md)

---

## 6. Review & Verification

- [x] Matches actual behavior documented in `architecture.md` §18 and `data-model.md` §4.
- [x] Confirms relay game traffic visibility (`PRIV-005`, `SEC-011`).
- [x] Documents exact retention schedules matching `cleanup.rs` implementation (`PRIV-003`, `PRIV-004`).
- [x] Supplies verbatim approved UI copy strings for desktop app integration (`DESIGN.md`).
