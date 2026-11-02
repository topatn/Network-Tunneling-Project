//! Network and flood tests for relay session heartbeats, idle timeouts, resumption (`T-063`),
//! and per-session / global limits and backpressure (`T-064`).

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use gtt_protocol::{
    encode_udp_datagram, ControlMessage, HelloAckMapping, Protocol, SequenceMessage, SessionLimits,
};
use gtt_relay::{
    BackpressureDropReason, BoundedSessionQueue, GlobalRelayMemoryBudget, RelaySessionConnectivity,
    RelayTunnelSession, TokenBucketLimiter,
};

type TestResult = Result<(), Box<dyn Error>>;

const ALLOCATION_ID: &str = "44444444-4444-4444-8444-444444444444";
const TUNNEL_ID: &str = "33333333-3333-4333-8333-333333333333";
const SESSION_1: &str = "55555555-5555-4555-8555-555555555551";
const SESSION_2: &str = "55555555-5555-4555-8555-555555555552";

fn sample_mappings() -> Vec<HelloAckMapping> {
    vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "tcp-game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "udp-game".to_owned(),
            protocol: Protocol::Udp,
            public_port: 19132,
        },
    ]
}

#[test]
fn relay_session_detects_idle_timeout_closes_tcp_retains_udp_and_resumes_within_grace() -> TestResult
{
    let mut session = RelayTunnelSession::new(
        ALLOCATION_ID,
        TUNNEL_ID,
        SESSION_1,
        sample_mappings(),
        5_000,  // heartbeat_interval_ms
        15_000, // idle_timeout_ms
        30_000, // grace_period_ms
        SessionLimits {
            max_tcp_flows: 8,
            max_udp_flows: 8,
        },
        1_000,
    );

    // 1. Ping/pong liveness updates last_peer_activity_ms and records RTT (`NET-005`).
    let pong = session.handle_ping(SequenceMessage { seq: 1 }, 5_000);
    assert_eq!(pong, ControlMessage::Pong(SequenceMessage { seq: 1 }));
    let rtt = session.handle_pong(SequenceMessage { seq: 1 }, 5_000, 5_042);
    assert_eq!(rtt, 42);
    assert_eq!(session.last_rtt_ms(), Some(42));

    // 2. Open 2 TCP flows and 1 UDP flow (`guest_addr -> flow_id`) before simulated outage.
    let guest_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    let guest_udp_addr = SocketAddr::new(guest_ip, 54321);
    let _ = session.tcp.begin_tcp_open(0, guest_ip)?;
    let _ = session.tcp.complete_tcp_open(0x00)?;
    let _ = session.tcp.begin_tcp_open(0, guest_ip)?;
    let _ = session.tcp.complete_tcp_open(0x00)?;
    assert_eq!(session.tcp.metrics().active_tcp_flows, 2);

    let framed_udp = session
        .udp
        .handle_guest_datagram(1, guest_udp_addr, b"hello-before-drop", 6_000)
        .ok_or("expected UDP frame")?;
    let flow_id_before = session
        .udp
        .flow_id_for_source(1, guest_udp_addr)
        .ok_or("missing flow_id_before")?;
    assert!(!framed_udp.is_empty());
    assert_eq!(session.udp.metrics().active_udp_flows, 1);

    // Verify flow_summary reflects both TCP and UDP flows (`contracts.md` §10).
    let summary = session.build_flow_summary();
    assert_eq!(summary.tcp_flows, 2);
    assert_eq!(summary.udp_flows, 1);
    assert!(summary.distinct_sources >= 1);

    // 3. Simulate silent peer drop: at t = 20_042ms (`20_042 - 5_042 == 15_000 == idle_timeout_ms`),
    // check_liveness transitions Active -> DisconnectedInGrace, closes all 2 TCP flows, and retains
    // the UDP flow and public port mappings (`NET-004`, `NET-005`, `FR-014`).
    let state = session.check_liveness(20_042).clone();
    assert_eq!(
        state,
        RelaySessionConnectivity::DisconnectedInGrace {
            dropped_at_ms: 20_042,
            grace_deadline_ms: 50_042,
        }
    );
    assert_eq!(session.tcp.metrics().active_tcp_flows, 0);
    assert_eq!(session.tcp_closed_on_drop_total(), 2);
    assert_eq!(session.udp.metrics().active_udp_flows, 1);

    // 4. Engine reconnects at t = 28_000ms (well before grace_deadline_ms = 50_042ms).
    // Session returns to Active with the exact same public ports (`25565`, `19132`) and preserves
    // the UDP flow (`flow_id_before`) so engine replies still route to `guest_udp_addr`!
    let ack = session.resume(SESSION_2, 28_000)?;
    assert_eq!(ack.session_id, SESSION_2);
    assert_eq!(ack.mappings, sample_mappings());
    assert_eq!(session.resumption_count(), 1);
    assert_eq!(
        session.connectivity(),
        &RelaySessionConnectivity::Active {
            session_id: SESSION_2.to_owned()
        }
    );
    assert_eq!(
        session.udp.flow_id_for_source(1, guest_udp_addr),
        Some(flow_id_before)
    );

    let engine_reply = encode_udp_datagram(1, flow_id_before, b"reply-after-resume", 1200)?;
    let (map_idx, routed_addr, payload) = session
        .udp
        .handle_engine_datagram(&engine_reply, 28_100)
        .ok_or("expected UDP reply to route after resume")?;
    assert_eq!(map_idx, 1);
    assert_eq!(routed_addr, guest_udp_addr);
    assert_eq!(payload, b"reply-after-resume");

    // 5. Simulate an outage longer than the grace period: drop at t = 30_000ms (grace deadline =
    // 60_000ms), then attempt to resume at t = 60_000ms -> refused and Released (`NET-004`).
    session.on_transport_disconnected(30_000);
    assert!(session
        .resume("55555555-5555-4555-8555-555555555553", 60_000)
        .is_err());
    assert_eq!(session.connectivity(), &RelaySessionConnectivity::Released);

    Ok(())
}

#[test]
fn flood_test_enforces_queue_bounds_bandwidth_caps_and_keeps_relay_memory_bounded() -> TestResult {
    // Global relay memory budget: 64 KiB across all sessions (`NET-006`, `OPS-004`).
    let global_budget = GlobalRelayMemoryBudget::new(64 * 1024);

    // Session A: max 16 frames, max 40 KiB, large bandwidth bucket (10 MiB) so queue caps trigger first.
    let mut queue_a = BoundedSessionQueue::new(
        16,
        40 * 1024,
        global_budget.clone(),
        TokenBucketLimiter::new(10 * 1024 * 1024, 10 * 1024 * 1024, 1_000),
    );

    // Session B: max 64 frames, max 40 KiB, sharing the 64 KiB global budget with Session A.
    let mut queue_b = BoundedSessionQueue::new(
        64,
        40 * 1024,
        global_budget.clone(),
        TokenBucketLimiter::new(10 * 1024 * 1024, 10 * 1024 * 1024, 1_000),
    );

    // 1. Flood Session A with 1_000 1-KiB frames -> only 16 frames (16 KiB) fit due to `max_queue_frames = 16`.
    let one_kib = [0x41u8; 1024];
    for _ in 0..1_000 {
        let _ = queue_a.try_enqueue(&one_kib, 1_000);
    }
    assert_eq!(queue_a.len(), 16);
    assert_eq!(queue_a.metrics().queued_bytes, 16 * 1024);
    assert_eq!(queue_a.metrics().dropped_queue_frames_full, 984);

    // Drain Session A and flood it with 4-KiB frames -> max_queue_bytes (40 KiB = 10 frames) triggers before 16 frames!
    queue_a.clear();
    assert_eq!(global_budget.current_bytes(), 0);

    let four_kib = [0x42u8; 4096];
    for _ in 0..500 {
        let _ = queue_a.try_enqueue(&four_kib, 1_000);
    }
    assert_eq!(queue_a.len(), 10);
    assert_eq!(queue_a.metrics().queued_bytes, 40 * 1024);
    assert_eq!(queue_a.metrics().dropped_queue_bytes_full, 490);

    // 2. While Session A holds 40 KiB of the 64 KiB global budget, flood Session B with 500 4-KiB
    // frames -> only 6 frames (24 KiB) fit before the 64 KiB global memory cap is reached!
    for _ in 0..500 {
        let _ = queue_b.try_enqueue(&four_kib, 1_000);
    }
    assert_eq!(queue_b.len(), 6);
    assert_eq!(queue_b.metrics().queued_bytes, 24 * 1024);
    assert_eq!(queue_b.metrics().dropped_global_memory_cap, 494);
    assert_eq!(global_budget.current_bytes(), 64 * 1024);
    assert!(
        global_budget.peak_bytes() <= global_budget.max_bytes(),
        "relay global memory exceeded budget: {} > {}",
        global_budget.peak_bytes(),
        global_budget.max_bytes()
    );

    // 3. Verify token-bucket bandwidth rate limiting (`OPS-004`):
    // Create Session C with 4 KiB burst and 4 KiB/s refill rate.
    queue_a.clear();
    queue_b.clear();
    assert_eq!(global_budget.current_bytes(), 0);

    let mut queue_c = BoundedSessionQueue::new(
        64,
        40 * 1024,
        global_budget.clone(),
        TokenBucketLimiter::new(4096, 4096, 2_000),
    );
    // First 4 KiB frame at t = 2_000ms succeeds; second 4 KiB frame at t = 2_000ms is dropped with
    // BandwidthLimitExceeded, and its global memory reservation is immediately rolled back.
    assert_eq!(queue_c.try_enqueue(&four_kib, 2_000), Ok(()));
    assert_eq!(
        queue_c.try_enqueue(&four_kib, 2_000),
        Err(BackpressureDropReason::BandwidthLimitExceeded)
    );
    assert_eq!(queue_c.metrics().dropped_bandwidth_exceeded, 1);
    assert_eq!(global_budget.current_bytes(), 4096);

    // After 1_000ms (t = 3_000ms), 4 KiB has refilled so the next 4 KiB frame is accepted.
    assert_eq!(queue_c.try_enqueue(&four_kib, 3_000), Ok(()));
    assert_eq!(queue_c.metrics().queued_bytes, 8192);
    assert!(global_budget.peak_bytes() <= 64 * 1024);

    Ok(())
}
