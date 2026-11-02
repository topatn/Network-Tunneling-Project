//! Integration and revocation-latency measurement tests for `T-065`
//! (`RelayAdmissionEnforcer`, `FR-008`, `FR-009`, `PERF-007`, `SEC-009`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Instant;

use gtt_relay::{
    canonicalize_guest_ip, RelayAdmissionEnforcer, RelayAdmissionSyncDelta,
    DEFAULT_MAX_RELAY_ADMISSIONS,
};

#[test]
fn relay_admits_only_live_admissions_isolates_tunnels_and_normalizes_ipv4_mapped_ipv6() {
    let mut enforcer = RelayAdmissionEnforcer::new(DEFAULT_MAX_RELAY_ADMISSIONS);
    let tunnel_a = "11111111-1111-4111-8111-111111111111";
    let tunnel_b = "22222222-2222-4222-8222-222222222222";
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 25));
    let unadmitted_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 99));

    // 1. Before any state sync, all guest IPs are refused (`FR-008`).
    assert!(!enforcer.is_guest_admitted(tunnel_a, guest_ip, 1_000));
    assert_eq!(enforcer.metrics.refused_unadmitted_total, 1);

    // 2. Sync full snapshot admitting `198.51.100.25` to `tunnel_a` until `t = 60_000`.
    let initial_snapshot = vec![RelayAdmissionSyncDelta {
        admission_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
        tunnel_id: tunnel_a.to_owned(),
        source_ip: guest_ip,
        active: true,
        expires_at_epoch_ms: 60_000,
        revoked_at_epoch_ms: None,
    }];
    let samples = enforcer.apply_state_sync(true, &initial_snapshot, 1_000);
    assert!(samples.is_empty());
    assert_eq!(enforcer.len(), 1);

    // 3. Admitted IP succeeds on `tunnel_a`, including when presented as IPv4-mapped IPv6
    //    `::ffff:198.51.100.25`.
    assert!(enforcer.is_guest_admitted(tunnel_a, guest_ip, 2_000));
    let mapped_v6 = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc633, 0x6419));
    assert_eq!(canonicalize_guest_ip(mapped_v6), guest_ip);
    assert!(enforcer.is_guest_admitted(tunnel_a, mapped_v6, 2_500));
    assert_eq!(enforcer.metrics.admitted_total, 2);

    // 4. Unadmitted IP is refused on `tunnel_a`, and admitted IP for `tunnel_a` is refused on
    //    `tunnel_b` (`SEC-009` cross-tunnel isolation).
    assert!(!enforcer.is_guest_admitted(tunnel_a, unadmitted_ip, 2_500));
    assert!(!enforcer.is_guest_admitted(tunnel_b, guest_ip, 2_500));
    assert_eq!(enforcer.metrics.refused_unadmitted_total, 3);

    // 5. Once `now_ms >= expires_at_epoch_ms`, the admission is refused and evicted.
    assert!(!enforcer.is_guest_admitted(tunnel_a, guest_ip, 60_000));
    assert_eq!(enforcer.metrics.refused_expired_total, 1);
    assert!(enforcer.is_empty());
}

#[test]
fn state_poll_revocation_enforces_immediately_and_measures_revocation_latency(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut enforcer = RelayAdmissionEnforcer::new(DEFAULT_MAX_RELAY_ADMISSIONS);
    let tunnel_id = "11111111-1111-4111-8111-111111111111";

    // Admit 5 guests at t = 1_000 ms.
    let mut initial = Vec::new();
    for i in 1..=5u8 {
        initial.push(RelayAdmissionSyncDelta {
            admission_id: format!("00000000-0000-4000-8000-00000000000{i}"),
            tunnel_id: tunnel_id.to_owned(),
            source_ip: IpAddr::V4(Ipv4Addr::new(198, 51, 100, i)),
            active: true,
            expires_at_epoch_ms: 300_000,
            revoked_at_epoch_ms: None,
        });
    }
    let _ = enforcer.apply_state_sync(true, &initial, 1_000);
    assert_eq!(enforcer.len(), 5);

    // Simulate revocations at the control plane followed by incremental state polls (`PERF-007`):
    // Poll intervals produce revocation latencies of 120 ms, 240 ms, 350 ms, 480 ms, and 910 ms.
    let simulated_latencies_ms = [120u64, 240, 350, 480, 910];
    let mut total_apply_ns = 0u128;

    for (idx, &lat_ms) in simulated_latencies_ms.iter().enumerate() {
        let guest_num = u8::try_from(idx + 1).unwrap_or(1);
        let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, guest_num));
        let revoked_at_ms = 10_000 + (u64::from(guest_num) * 1_000);
        let enforced_at_ms = revoked_at_ms + lat_ms;

        // Admitted right before state poll applies the revocation.
        assert!(enforcer.is_guest_admitted(tunnel_id, guest_ip, enforced_at_ms - 1));

        let delta = [RelayAdmissionSyncDelta {
            admission_id: format!("00000000-0000-4000-8000-00000000000{guest_num}"),
            tunnel_id: tunnel_id.to_owned(),
            source_ip: guest_ip,
            active: false,
            expires_at_epoch_ms: 300_000,
            revoked_at_epoch_ms: Some(revoked_at_ms),
        }];

        let start_wall = Instant::now();
        let samples = enforcer.apply_state_sync(false, &delta, enforced_at_ms);
        total_apply_ns += start_wall.elapsed().as_nanos();

        assert_eq!(samples.len(), 1);
        assert_eq!(samples.first().map(|s| s.latency_ms), Some(lat_ms));

        // Immediately refused after state sync applies the revocation (`FR-009`).
        assert!(!enforcer.is_guest_admitted(tunnel_id, guest_ip, enforced_at_ms));
    }

    assert!(enforcer.is_empty());
    assert_eq!(enforcer.metrics.revocations_enforced_total, 5);

    let stats = enforcer
        .revocation_timing_stats()
        .ok_or("expected revocation timing stats")?;
    assert_eq!(stats.sample_count, 5);
    assert_eq!(stats.min_ms, 120);
    assert_eq!(stats.p50_ms, 350);
    assert_eq!(stats.p95_ms, 910);
    assert_eq!(stats.max_ms, 910);
    // In-memory table revocation enforcement completes in under 10 ms total across all 5 polls.
    assert!(total_apply_ns < 10_000_000);
    Ok(())
}
