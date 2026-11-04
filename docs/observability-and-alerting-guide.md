# Observability, Alerting, and Egress Cost Guide

**Document version:** 1.0.0  
**Status:** Approved  
**Last updated:** 2026-11-04  
**Requirements fulfilled:** `OPS-002`, `OPS-006`, `OPS-004`, `REL-002`, `PRIV-002`, `SEC-013`  
**Authoritative companions:** `architecture.md` §§15, 17; `research.md` §5 (RQ-8); `docs/ddos-and-abuse-mitigation-plan.md`

---

## 1. Executive Summary & Purpose

In Game Tunnel Tooling (GTT), user game traffic never crosses the control plane; it flows directly between the host machine's game server engine and connecting guests through distributed public relay nodes (`architecture.md` §1).

Consequently, service availability, performance, and operational cost depend on two critical observability pillars:
1. **Relay Health & Saturation Monitoring (`OPS-002`):** Real-time visibility into relay node reachability, heartbeat age, port-pool allocation saturation, global queue memory pressure (16 MiB ceiling), and token-bucket bandwidth drop rates.
2. **Egress Bandwidth Cost Monitoring (`OPS-006`):** Continuous tracking of outbound network transfer (the dominant variable operating expense identified in `research.md` RQ-8), daily cost calculation against cloud provider tariffs, and automated threshold alerts.

---

## 2. Metrics Catalogue

Metrics are exposed in the standard Prometheus text-based format from internal scrape endpoints on the control plane (`:9090/metrics`) and individual relay nodes (`:9091/metrics`).

### 2.1 Relay Subsystem Metrics (`crates/relay/src/observability.rs`)

| Metric Name | Type | Labels | Description / Operational Significance |
|---|---|---|---|
| `gtt_relay_uptime_seconds` | Gauge | `relay_id`, `region` | Seconds elapsed since relay process start. |
| `gtt_relay_sessions_active` | Gauge | `relay_id`, `region` | Currently active QUIC tunnel sessions with host engines. |
| `gtt_relay_tcp_flows_active` | Gauge | `relay_id`, `region` | Number of currently concurrent TCP stream forwarding flows. |
| `gtt_relay_tcp_connections_total` | Counter | `relay_id`, `region` | Total guest TCP connections accepted on reserved public ports. |
| `gtt_relay_tcp_refused_total` | Counter | `relay_id`, `region` | Guest TCP connections refused (no engine listener or unmapped port). |
| `gtt_relay_tcp_limit_reached_total` | Counter | `relay_id`, `region` | Connections refused due to `max_tcp_flows` ceiling (`0x02`). |
| `gtt_relay_udp_flows_active` | Gauge | `relay_id`, `region` | Active NAT flow entries in the relay's UDP translation table. |
| `gtt_relay_udp_datagrams_forwarded_total` | Counter | `relay_id`, `region`, `direction` (`ingress` \| `egress`) | UDP packets successfully bridged between guest and host engine. |
| `gtt_relay_udp_flows_expired_total` | Counter | `relay_id`, `region` | UDP flow entries expired and evicted following inactivity (`55s`). |
| `gtt_relay_ingress_bytes_total` | Counter | `relay_id`, `region`, `protocol` | Total inbound payload bytes received by the relay. |
| `gtt_relay_egress_bytes_total` | Counter | `relay_id`, `region`, `protocol` | **Dominant cost metric:** Outbound bytes forwarded from the relay. |
| `gtt_relay_queued_frames` | Gauge | `relay_id`, `region` | Frames currently buffered across all active session queues. |
| `gtt_relay_queued_bytes` | Gauge | `relay_id`, `region` | Bytes currently buffered across active session queues. |
| `gtt_relay_global_queued_bytes` | Gauge | `relay_id`, `region` | Aggregate bytes in queue memory against the global budget. |
| `gtt_relay_global_memory_budget_bytes` | Gauge | `relay_id`, `region` | Configured memory ceiling (`16,777,216` bytes / 16 MiB default). |
| `gtt_relay_global_memory_utilization_ratio` | Gauge | `relay_id`, `region` | Ratio (`global_queued / budget`). Alert triggers at $\ge 0.85$. |
| `gtt_relay_frames_dropped_total` | Counter | `relay_id`, `region`, `reason` | Drops by category: `bandwidth_exceeded`, `global_memory_cap`, `queue_bytes_full`, `queue_frames_full`, `oversize`, `unknown_mapping`, `malformed`, `flow_limit`. |
| `gtt_relay_admissions_checks_total` | Counter | `relay_id`, `region`, `result` (`admitted` \| `refused_unadmitted` \| `refused_expired`) | Guest IP admission check outcomes (`FR-009`, `SEC-007`). |

### 2.2 Control Plane Metrics (`crates/control-plane/src/observability.rs`)

| Metric Name | Type | Labels | Description / Operational Significance |
|---|---|---|---|
| `gtt_control_plane_info` | Gauge | `environment`, `version` | Constant 1 gauge with build and environment metadata. |
| `gtt_control_plane_uptime_seconds` | Gauge | `environment` | Control plane process uptime. |
| `gtt_control_plane_tunnels_active` | Gauge | `environment` | Count of tunnels in non-terminal states (`ready`, `started`). |
| `gtt_control_plane_allocation_requests_total` | Counter | `environment` | Total tunnel start and token allocation requests received. |
| `gtt_control_plane_allocation_failures_total` | Counter | `environment`, `reason` | Failed allocations (`no_healthy_relay`, `port_exhaustion`, `quota_exceeded`, `db_error`). |
| `gtt_control_plane_rate_limit_rejections_total` | Counter | `environment`, `class` (`strict` \| `normal` \| `lenient`) | Total HTTP 429 rejections emitted by sliding-window rate limiters. |
| `gtt_control_plane_relay_heartbeat_timestamp_seconds` | Gauge | `environment`, `relay_id`, `region` | Unix timestamp of last received state/usage report from relay. |
| `gtt_control_plane_relay_status` | Gauge | `environment`, `relay_id`, `region`, `status` | State indicator (`healthy`, `unhealthy`, `draining`, `retired`). |
| `gtt_control_plane_relay_ports_allocated` | Gauge | `environment`, `relay_id`, `region` | Number of reserved public ports currently assigned on relay. |
| `gtt_control_plane_relay_ports_capacity` | Gauge | `environment`, `relay_id`, `region` | Total configured public port range capacity on relay. |
| `gtt_control_plane_relay_port_pool_utilization_ratio` | Gauge | `environment`, `relay_id`, `region` | Ratio of allocated ports to total capacity. Alert triggers at $\ge 0.90$. |
| `gtt_control_plane_relay_egress_bytes_recorded_total` | Counter | `environment`, `relay_id`, `region` | Egress bytes recorded in database from periodic relay reports. |

---

## 3. Prometheus Alerting Rules (`OPS-002`, `OPS-006`)

All alerting rules are declared in [`infra/monitoring/prometheus/alert_rules.yml`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/infra/monitoring/prometheus/alert_rules.yml).

### Alert Rule Specifications

```yaml
groups:
  - name: gtt_relay_health_and_saturation_alerts
    rules:
      - alert: RelayUnhealthy
        expr: >
          (gtt_control_plane_relay_status{status="healthy"} != 1)
          or
          ((time() - gtt_control_plane_relay_heartbeat_timestamp_seconds) > 30)
        for: 1m
        labels:
          severity: critical
          tier: infrastructure

      - alert: AllocationFailureSpike
        expr: >
          sum by (environment) (rate(gtt_control_plane_allocation_failures_total[5m])) > 0.05
        for: 2m
        labels:
          severity: warning
          tier: control-plane

      - alert: RelayPortPoolSaturation
        expr: >
          (gtt_control_plane_relay_ports_allocated / gtt_control_plane_relay_ports_capacity) >= 0.90
        for: 2m
        labels:
          severity: warning
          tier: relay

      - alert: RelayBandwidthSaturation
        expr: >
          sum by (relay_id, region) (rate(gtt_relay_frames_dropped_total{reason="bandwidth_exceeded"}[5m])) > 5
        for: 2m
        labels:
          severity: warning
          tier: relay

      - alert: RelayMemorySaturation
        expr: >
          (gtt_relay_global_memory_utilization_ratio >= 0.85)
          or
          (sum by (relay_id, region) (rate(gtt_relay_frames_dropped_total{reason="global_memory_cap"}[5m])) > 0)
        for: 1m
        labels:
          severity: critical
          tier: relay

  - name: gtt_egress_cost_alerts
    rules:
      - alert: RelayDailyEgressCostThresholdExceeded
        expr: >
          ((sum by (relay_id) (increase(gtt_relay_egress_bytes_total[24h])) / 1073741824) * 0.01) > 10.0
        for: 15m
        labels:
          severity: warning
          tier: billing

      - alert: RelayProjectedMonthlyEgressBudgetExceeded
        expr: >
          (((sum by (relay_id) (increase(gtt_relay_egress_bytes_total[24h])) / 1073741824) * 0.01) * 30) > 250.0
        for: 1h
        labels:
          severity: critical
          tier: billing
```

---

## 4. Egress Cost Monitoring Model (`OPS-006`)

### 4.1 Cost Drivers & Provider Tariffs (`research.md` RQ-8)

Because relays blindly forward application frames without decoding game states, **outbound network egress** is the dominant variable operating expense.

| Provider / Tier | Included Monthly Egress | Overage Tariff (per GB) | Effective Cost for 1 TB Overage |
|---|---|---|---|
| **Hetzner Cloud (EU)** | 20 TB / instance | €1.00 / TB ($\approx \$0.0011/\text{GB}$) | $\approx \$1.10$ |
| **Hetzner Cloud (US)** | 1 TB / instance | €1.00 / TB ($\approx \$0.0011/\text{GB}$) | $\approx \$1.10$ |
| **Vultr / DigitalOcean** | 1–2 TB / instance | $\$0.0100/\text{GB}$ | $\$10.00$ |
| **AWS / GCP Internet Egress** | 100 GB tier | $\$0.0800\text{--}\$0.1200/\text{GB}$ | $\$80.00\text{--}\$120.00$ |

### 4.2 Formulae & Threshold Calculations

In Prometheus and Grafana dashboards, daily cost is evaluated as:
$$\text{Cost}_{\text{24h}} = \left( \frac{\Delta_{24\text{h}}(\text{egress\_bytes})}{1,073,741,824} \right) \times C_{\text{GB}}$$
where $C_{\text{GB}}$ is the configurable provider rate (default $\$0.01/\text{GB}$).

- **Daily Alert Ceiling:** $\$10.00/\text{day}$ ($\approx 1,000\text{ GB/day}$ at $\$0.01/\text{GB}$).
- **Monthly Projected Ceiling:** $\text{Cost}_{\text{24h}} \times 30 > \$250.00/\text{month}$.

---

## 5. Grafana Operational Dashboards

The monitoring stack includes two production-grade dashboards:

1. **[`relay_health_and_saturation.json`](file:///c:/Users/chris/Documents/Network%20Tunneling%20Project/infra/monitoring/dashboards/relay_health_and_saturation.json):**
   - Real-time status cards for relay heartbeat age and health status.
   - Port pool utilization gauges with color thresholds (Green $<75\%$, Yellow $75\text{--}89\%$, Red $\ge 90\%$).
   - Global queue memory gauge vs. 16 MiB ceiling (Alert at $\ge 85\%$).
   - Active TCP/UDP flows and network bandwidth throughput (Mbps).
   - Stacked bar chart showing drops by specific reason (`bandwidth_exceeded`, `global_memory_cap`, etc.).
   - Guest admission authorization check rate.

2. **[`relay_egress_and_cost.json`](file:///c:/Users/chris/Documents/Network%20Tunneling Project/infra/monitoring/dashboards/relay_egress_and_cost.json):**
   - Single-stat cards for 24-hour egress, 24-hour cost, 30-day volume, and projected month-end spend.
   - Interactive variable dropdown `$cost_per_gb` (Hetzner $\$0.001$, Vultr/DO $\$0.01$, AWS/GCP $\$0.09$).
   - Time-series tracking daily egress volume (GB/day per relay) and cost run-rate ($/day) against the $\$10.00/\text{day}$ threshold line.

---

## 6. Structured Logging & Redaction Protocol (`PRIV-002`, `SEC-013`)

All diagnostic and operational logs from control plane and relay components must be serialized as JSON lines and scrubbed of sensitive material prior to disk or network emission:

```json
{
  "timestamp": "2026-11-04T12:00:00Z",
  "level": "info",
  "target": "gtt_control_plane::routes::invitations",
  "event": "invitation_claimed",
  "fields": {
    "tunnel_id": "33333333-3333-4333-8333-333333333333",
    "access_token": "[REDACTED-SECRET]",
    "guest_ip": "[REDACTED-IP]",
    "secret": "[REDACTED-SECRET]"
  }
}
```

### Invariant Checks (`crates/control-plane/src/redaction.rs`):
- Bearer tokens and signed JWT fragments matching `^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$` are scrubbed.
- Fields matching `SENSITIVE_FIELD_KEYS` (`access_token`, `refresh_token`, `secret`, `password`, `code_verifier`, `token`) are replaced with `[REDACTED-SECRET]`.
- Raw IPv4 and IPv6 strings are replaced with `[REDACTED-IP]` or masked to `/24` and `/48` (`mask_ip_for_owner`).
- Any log message failing `scan_for_sensitive_leaks` is dropped and replaced with a sanitized safety notice.

---

## 7. Verification & Exercise Procedures

Run the automated observability verification suite locally or in CI:

```powershell
python scripts/verify_observability.py
```

This verifies:
1. Syntactic validity of Prometheus alerts and scrape targets.
2. Structural compliance of Grafana dashboards.
3. Accurate evaluation of all 7 alert conditions across healthy and breached synthetic time-series.
4. Total compliance of structured log redaction with zero token/IP/secret leaks.
