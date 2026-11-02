//! Guest source-IP admission enforcement, state-poll delta application, and revocation-latency
//! measurement (`T-065`, `FR-008`, `FR-009`, `PERF-007`, `SEC-009`, `PRIV-002`).
//!
//! ## Guarantees
//! 1. **Default-deny guest admission (`FR-008`, `FR-009`):** A guest source IP is permitted to
//!    open a TCP stream or send a UDP datagram to a tunnel's public relay port if and only if an
//!    active, non-expired admission exists for `(tunnel_id, canonical_ip)`.
//! 2. **Cross-tunnel isolation (`SEC-009`):** Admissions are keyed by `(tunnel_id, canonical_ip)`.
//!    An admission on tunnel A never admits traffic on tunnel B.
//! 3. **IPv4-mapped IPv6 normalization (`data-model.md` §4):** `::ffff:203.0.113.5` and
//!    `203.0.113.5` normalize to the same [`IpAddr::V4`] key so dual-stack sockets cannot bypass
//!    or fragment the admission table.
//! 4. **Revocation latency measurement (`PERF-007`):** When a state-poll delta marks an admission
//!    inactive (`active == false`), [`RelayAdmissionEnforcer::apply_state_sync`] computes the
//!    elapsed time between `revoked_at_epoch_ms` (recorded at the control plane) and
//!    `enforced_at_epoch_ms` (when the relay removes the admission and closes/blocks traffic),
//!    producing a [`RevocationLatencySample`] and aggregated [`RevocationTimingStats`].

use std::collections::HashMap;
use std::net::IpAddr;

/// Default maximum concurrent guest admissions tracked per relay instance (`AGENTS.md` §4).
pub const DEFAULT_MAX_RELAY_ADMISSIONS: usize = 16_384;

/// Upper ceiling on `max_admissions`.
pub const MAX_RELAY_ADMISSIONS_CEILING: usize = 100_000;

/// One guest admission delta received from `GET /internal/v1/relays/{relay_id}/state`
/// (`contracts.md` §7, `T-048`, `T-065`).
///
/// Never logged (`PRIV-002`, `INV-5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAdmissionSyncDelta {
    /// Admission UUID.
    pub admission_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Guest source IP address.
    pub source_ip: IpAddr,
    /// True if active, false if revoked or expired.
    pub active: bool,
    /// Expiration timestamp in Unix epoch milliseconds.
    pub expires_at_epoch_ms: u64,
    /// Control-plane revocation timestamp in Unix epoch milliseconds, when `active == false`.
    pub revoked_at_epoch_ms: Option<u64>,
}

/// Active admission stored in the relay's in-memory enforcement table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAdmissionEntry {
    /// Admission UUID.
    pub admission_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Canonical guest IP (IPv4-mapped IPv6 converted to `IpAddr::V4`).
    pub source_ip: IpAddr,
    /// Expiration timestamp in Unix epoch milliseconds.
    pub expires_at_epoch_ms: u64,
}

/// One measured revocation enforcement sample (`PERF-007`, `FR-009`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationLatencySample {
    /// Admission UUID revoked.
    pub admission_id: String,
    /// Tunnel UUID.
    pub tunnel_id: String,
    /// Timestamp when the control plane recorded `revoked_at` (Unix epoch ms).
    pub revoked_at_epoch_ms: u64,
    /// Timestamp when the relay enforced the revocation (Unix epoch ms).
    pub enforced_at_epoch_ms: u64,
    /// Measured revocation latency (`enforced_at_epoch_ms - revoked_at_epoch_ms`) in milliseconds.
    pub latency_ms: u64,
}

/// Summary statistics over measured [`RevocationLatencySample`]s (`PERF-007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevocationTimingStats {
    /// Number of revocation samples recorded.
    pub sample_count: usize,
    /// Minimum revocation latency in milliseconds.
    pub min_ms: u64,
    /// Median (p50) revocation latency in milliseconds.
    pub p50_ms: u64,
    /// 95th-percentile (p95) revocation latency in milliseconds.
    pub p95_ms: u64,
    /// Maximum revocation latency in milliseconds.
    pub max_ms: u64,
}

/// Metrics counters for relay guest admission enforcement (`T-065`, `FR-009`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayAdmissionMetrics {
    /// Total admission checks that succeeded.
    pub admitted_total: u64,
    /// Total admission checks refused because no admission existed for `(tunnel_id, source_ip)`.
    pub refused_unadmitted_total: u64,
    /// Total admission checks refused because the admission had expired (`now_ms >= expires_at`).
    pub refused_expired_total: u64,
    /// Total admissions refused at sync time due to `max_admissions` capacity ceiling.
    pub sync_dropped_capacity_total: u64,
    /// Total revocations enforced via state sync.
    pub revocations_enforced_total: u64,
}

/// Normalize an [`IpAddr`] so IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) map to
/// `IpAddr::V4(a.b.c.d)` (`data-model.md` §4).
pub fn canonicalize_guest_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
    }
}

/// Bounded in-memory guest source-IP admission enforcer for a relay node (`T-065`, `FR-009`,
/// `PERF-007`).
#[derive(Debug, Clone)]
pub struct RelayAdmissionEnforcer {
    max_admissions: usize,
    /// Keyed by `(tunnel_id, canonical_source_ip)`.
    entries: HashMap<(String, IpAddr), RelayAdmissionEntry>,
    /// Bounded history of revocation latency measurements (`PERF-007`).
    revocation_samples: Vec<RevocationLatencySample>,
    /// Enforcement counters.
    pub metrics: RelayAdmissionMetrics,
}

/// Maximum number of [`RevocationLatencySample`]s retained in memory (`AGENTS.md` §4).
pub const MAX_REVOCATION_SAMPLES: usize = 1_024;

impl RelayAdmissionEnforcer {
    /// Create a new [`RelayAdmissionEnforcer`] with a bounded capacity (`1..=100_000`).
    pub fn new(max_admissions: usize) -> Self {
        let cap = max_admissions.clamp(1, MAX_RELAY_ADMISSIONS_CEILING);
        Self {
            max_admissions: cap,
            entries: HashMap::new(),
            revocation_samples: Vec::new(),
            metrics: RelayAdmissionMetrics::default(),
        }
    }

    /// Number of currently stored admission entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no admissions are currently stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Evict expired admissions as of `now_ms`.
    pub fn purge_expired(&mut self, now_ms: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, entry| entry.expires_at_epoch_ms > now_ms);
        before.saturating_sub(self.entries.len())
    }

    /// Apply a state-poll response from `GET /internal/v1/relays/{relay_id}/state` (`contracts.md`
    /// §7, `REL-004`, `FR-009`, `PERF-007`).
    ///
    /// * When `full_snapshot` is `true` (relay startup or restart recovery), replaces the table
    ///   with all active, non-expired entries in `deltas`.
    /// * When `full_snapshot` is `false` (incremental cursor poll), upserts active entries and
    ///   removes inactive/revoked entries, recording a [`RevocationLatencySample`] for each
    ///   revocation enforced.
    pub fn apply_state_sync(
        &mut self,
        full_snapshot: bool,
        deltas: &[RelayAdmissionSyncDelta],
        now_ms: u64,
    ) -> Vec<RevocationLatencySample> {
        let _ = self.purge_expired(now_ms);
        let mut newly_measured = Vec::new();

        if full_snapshot {
            self.entries.clear();
        }

        for delta in deltas {
            let ip = canonicalize_guest_ip(delta.source_ip);
            let key = (delta.tunnel_id.clone(), ip);

            if delta.active && delta.expires_at_epoch_ms > now_ms {
                if !self.entries.contains_key(&key) && self.entries.len() >= self.max_admissions {
                    self.metrics.sync_dropped_capacity_total =
                        self.metrics.sync_dropped_capacity_total.saturating_add(1);
                    continue;
                }
                self.entries.insert(
                    key,
                    RelayAdmissionEntry {
                        admission_id: delta.admission_id.clone(),
                        tunnel_id: delta.tunnel_id.clone(),
                        source_ip: ip,
                        expires_at_epoch_ms: delta.expires_at_epoch_ms,
                    },
                );
            } else {
                let removed = self.entries.remove(&key);
                if removed.is_some() || delta.revoked_at_epoch_ms.is_some() {
                    self.metrics.revocations_enforced_total =
                        self.metrics.revocations_enforced_total.saturating_add(1);
                    let revoked_at = delta.revoked_at_epoch_ms.unwrap_or(now_ms);
                    let sample = RevocationLatencySample {
                        admission_id: delta.admission_id.clone(),
                        tunnel_id: delta.tunnel_id.clone(),
                        revoked_at_epoch_ms: revoked_at,
                        enforced_at_epoch_ms: now_ms,
                        latency_ms: now_ms.saturating_sub(revoked_at),
                    };
                    if self.revocation_samples.len() >= MAX_REVOCATION_SAMPLES {
                        let _ = self.revocation_samples.remove(0);
                    }
                    self.revocation_samples.push(sample.clone());
                    newly_measured.push(sample);
                }
            }
        }

        newly_measured
    }

    /// Check whether `guest_ip` is currently admitted to `tunnel_id` as of `now_ms` (`FR-008`,
    /// `FR-009`, `SEC-009`).
    pub fn is_guest_admitted(&mut self, tunnel_id: &str, guest_ip: IpAddr, now_ms: u64) -> bool {
        let ip = canonicalize_guest_ip(guest_ip);
        let key = (tunnel_id.to_owned(), ip);

        let Some(entry) = self.entries.get(&key) else {
            self.metrics.refused_unadmitted_total =
                self.metrics.refused_unadmitted_total.saturating_add(1);
            return false;
        };

        if now_ms >= entry.expires_at_epoch_ms {
            let _ = self.entries.remove(&key);
            self.metrics.refused_expired_total =
                self.metrics.refused_expired_total.saturating_add(1);
            return false;
        }

        self.metrics.admitted_total = self.metrics.admitted_total.saturating_add(1);
        true
    }

    /// Remove all admissions belonging to `tunnel_id` (e.g. when a tunnel allocation is released).
    pub fn remove_all_for_tunnel(&mut self, tunnel_id: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(tid, _), _| tid != tunnel_id);
        before.saturating_sub(self.entries.len())
    }

    /// Return all recorded [`RevocationLatencySample`]s (`PERF-007`).
    pub fn revocation_samples(&self) -> &[RevocationLatencySample] {
        &self.revocation_samples
    }

    /// Compute min / p50 / p95 / max revocation latency statistics in milliseconds (`PERF-007`).
    pub fn revocation_timing_stats(&self) -> Option<RevocationTimingStats> {
        if self.revocation_samples.is_empty() {
            return None;
        }
        let mut latencies: Vec<u64> = self
            .revocation_samples
            .iter()
            .map(|s| s.latency_ms)
            .collect();
        latencies.sort_unstable();

        let count = latencies.len();
        let min_ms = latencies.first().copied().unwrap_or(0);
        let max_ms = latencies.last().copied().unwrap_or(0);
        let p50_idx = count / 2;
        let p95_idx = (count.saturating_mul(95).saturating_sub(1)) / 100;
        let p50_ms = latencies.get(p50_idx).copied().unwrap_or(max_ms);
        let p95_ms = latencies.get(p95_idx).copied().unwrap_or(max_ms);

        Some(RevocationTimingStats {
            sample_count: count,
            min_ms,
            p50_ms,
            p95_ms,
            max_ms,
        })
    }
}
