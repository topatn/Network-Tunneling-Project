//! Port-scan and resource-cleanup tests for relay graceful shutdown and crash recovery
//! (`T-067`, `NET-008`, `contracts.md` §§10, 12).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

use gtt_protocol::{
    decode_control_frame, ControlMessage, HelloAckMapping, Protocol, SessionLimits,
};
use gtt_relay::{
    BoundedSessionQueue, GlobalRelayMemoryBudget, RelayAllocationSupervisor, RelayBoundAllocation,
    RelayTunnelSession, TokenBucketLimiter,
};

fn build_test_session(
    allocation_id: &str,
    tunnel_id: &str,
    session_id: &str,
) -> RelayTunnelSession {
    let mappings = vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "query".to_owned(),
            protocol: Protocol::Udp,
            public_port: 25566,
        },
    ];
    RelayTunnelSession::new(
        allocation_id,
        tunnel_id,
        session_id,
        mappings,
        5_000,
        15_000,
        30_000,
        SessionLimits {
            max_tcp_flows: 8,
            max_udp_flows: 8,
        },
        0,
    )
}

/// Port-scan helper: verifies whether a TCP port is accepting connections (`NET-008`).
fn is_tcp_listener_open(addr: SocketAddr) -> bool {
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// Port-scan helper: verifies whether a UDP port is currently held by a listener (`NET-008`).
///
/// If binding the exact address succeeds immediately, no listener remains holding that UDP port.
fn is_udp_port_held(addr: SocketAddr) -> bool {
    UdpSocket::bind(addr).is_err()
}

/// Helper to confirm a TCP port is completely released and re-bindable after stop (`NET-008`).
fn is_tcp_port_rebindable(addr: SocketAddr) -> bool {
    TcpListener::bind(addr).is_ok()
}

#[test]
fn port_scan_after_graceful_stop_confirms_no_listeners_or_queued_bytes_remain(
) -> Result<(), Box<dyn std::error::Error>> {
    let budget = GlobalRelayMemoryBudget::new(65_536);
    let mut queue = BoundedSessionQueue::new(
        32,
        16_384,
        budget.clone(),
        TokenBucketLimiter::new(65_536, 65_536, 0),
    );
    queue
        .try_enqueue(&[0xAA; 1024], 0)
        .map_err(|e| format!("enqueue failed: {e:?}"))?;
    assert_eq!(budget.current_bytes(), 1024);

    let session = build_test_session(
        "52222222-2222-4222-8222-222222222222",
        "11111111-1111-4111-8111-111111111111",
        "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44",
    );

    let (mut bound, bound_mappings) = RelayBoundAllocation::bind_for_session(
        session,
        queue,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true,
    )?;
    assert_eq!(bound_mappings.len(), 2);

    // Open one active TCP flow and one active UDP flow before stopping.
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 77));
    let _ = bound.session.tcp.begin_tcp_open(0, guest_ip)?;
    let _ = bound
        .session
        .udp
        .handle_guest_datagram(1, SocketAddr::new(guest_ip, 51000), b"ping-payload", 100)
        .ok_or("expected UDP datagram encapsulation")?;
    assert_eq!(bound.session.tcp.metrics().active_tcp_flows, 1);
    assert_eq!(bound.session.udp.metrics().active_udp_flows, 1);

    let addrs = bound.bound_public_addrs();
    assert_eq!(addrs.len(), 2);
    let tcp_addr = addrs
        .iter()
        .find(|(proto, _)| *proto == Protocol::Tcp)
        .map(|(_, a)| *a)
        .ok_or("missing TCP address")?;
    let udp_addr = addrs
        .iter()
        .find(|(proto, _)| *proto == Protocol::Udp)
        .map(|(_, a)| *a)
        .ok_or("missing UDP address")?;

    // While active, the TCP listener accepts connections and the UDP port is held.
    assert!(is_tcp_listener_open(tcp_addr));
    assert!(is_udp_port_held(udp_addr));

    let mut supervisor = RelayAllocationSupervisor::new();
    supervisor.insert(bound)?;
    assert_eq!(supervisor.active_allocation_count(), 1);

    // Graceful stop (`NET-008`).
    let report = supervisor
        .stop_allocation("52222222-2222-4222-8222-222222222222", "user_stopped")
        .ok_or("expected allocation shutdown report")?;

    assert_eq!(supervisor.active_allocation_count(), 0);
    assert_eq!(report.closed_tcp_flows, 1);
    assert_eq!(report.cleared_udp_flows, 1);
    assert_eq!(report.released_queue_bytes, 1024);
    assert_eq!(budget.current_bytes(), 0);

    // Verify encoded goodbye control frame (`contracts.md` §10).
    let goodbye_bytes = report.goodbye_frame.ok_or("expected goodbye frame")?;
    let (msg, consumed) = decode_control_frame(&goodbye_bytes)?;
    assert_eq!(consumed, goodbye_bytes.len());
    match msg {
        ControlMessage::Goodbye(g) => assert_eq!(g.reason, "user_stopped"),
        other => return Err(format!("expected Goodbye frame, got {other:?}").into()),
    }

    // Port scan after stop: neither TCP nor UDP listener remains open (`NET-008`).
    assert!(!is_tcp_listener_open(tcp_addr));
    assert!(is_tcp_port_rebindable(tcp_addr));
    assert!(!is_udp_port_held(udp_addr));

    Ok(())
}

#[test]
fn crash_recovery_and_grace_expiry_sweep_releases_orphaned_ports(
) -> Result<(), Box<dyn std::error::Error>> {
    let budget = GlobalRelayMemoryBudget::new(65_536);

    // Allocation A: still active in control-plane state.
    let session_a = build_test_session(
        "52222222-2222-4222-8222-222222222221",
        "11111111-1111-4111-8111-111111111111",
        "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b41",
    );
    let queue_a = BoundedSessionQueue::new(
        16,
        8192,
        budget.clone(),
        TokenBucketLimiter::new(65_536, 65_536, 0),
    );
    let (bound_a, _) = RelayBoundAllocation::bind_for_session(
        session_a,
        queue_a,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true,
    )?;
    let addrs_a = bound_a.bound_public_addrs();

    // Allocation B: orphaned after control-plane crash/failover (missing from authoritative list).
    let session_b = build_test_session(
        "52222222-2222-4222-8222-222222222222",
        "11111111-1111-4111-8111-111111111112",
        "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b42",
    );
    let mut queue_b = BoundedSessionQueue::new(
        16,
        8192,
        budget.clone(),
        TokenBucketLimiter::new(65_536, 65_536, 0),
    );
    queue_b
        .try_enqueue(&[0xBB; 512], 0)
        .map_err(|e| format!("enqueue failed: {e:?}"))?;
    let (bound_b, _) = RelayBoundAllocation::bind_for_session(
        session_b,
        queue_b,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true,
    )?;
    let addrs_b = bound_b.bound_public_addrs();

    // Allocation C: present in authoritative list, but its grace period has expired.
    let session_c = build_test_session(
        "52222222-2222-4222-8222-222222222223",
        "11111111-1111-4111-8111-111111111113",
        "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b43",
    );
    let queue_c = BoundedSessionQueue::new(
        16,
        8192,
        budget.clone(),
        TokenBucketLimiter::new(65_536, 65_536, 0),
    );
    let (mut bound_c, _) = RelayBoundAllocation::bind_for_session(
        session_c,
        queue_c,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true,
    )?;
    let _ = bound_c.session.on_transport_disconnected(1_000);
    let addrs_c = bound_c.bound_public_addrs();

    let mut supervisor = RelayAllocationSupervisor::new();
    supervisor.insert(bound_a)?;
    supervisor.insert(bound_b)?;
    supervisor.insert(bound_c)?;
    assert_eq!(supervisor.active_allocation_count(), 3);

    // Run crash-recovery & grace sweep at t = 40_000 ms (past 30s grace window for C, while A has
    // fresh activity at t = 39_000 ms).
    if let Some(a) = supervisor.get_mut("52222222-2222-4222-8222-222222222221") {
        let _ = a
            .session
            .handle_ping(gtt_protocol::SequenceMessage { seq: 1 }, 39_000);
    }

    let authoritative_ids = vec![
        "52222222-2222-4222-8222-222222222221".to_owned(),
        "52222222-2222-4222-8222-222222222223".to_owned(),
    ];
    let reports = supervisor.recover_and_sweep(&authoritative_ids, 40_000);
    assert_eq!(reports.len(), 2);
    assert_eq!(supervisor.active_allocation_count(), 1);
    assert_eq!(budget.current_bytes(), 0);

    // Verify B and C have no open listeners (`NET-008`).
    for (proto, addr) in addrs_b.into_iter().chain(addrs_c) {
        match proto {
            Protocol::Tcp => {
                assert!(!is_tcp_listener_open(addr));
                assert!(is_tcp_port_rebindable(addr));
            }
            Protocol::Udp => {
                assert!(!is_udp_port_held(addr));
            }
        }
    }

    // Verify A is still open until full relay shutdown.
    for (proto, addr) in &addrs_a {
        match proto {
            Protocol::Tcp => assert!(is_tcp_listener_open(*addr)),
            Protocol::Udp => assert!(is_udp_port_held(*addr)),
        }
    }

    // Full relay shutdown releases A as well.
    let final_reports = supervisor.shutdown_all("relay_draining");
    assert_eq!(final_reports.len(), 1);
    assert_eq!(supervisor.active_allocation_count(), 0);

    for (proto, addr) in addrs_a {
        match proto {
            Protocol::Tcp => {
                assert!(!is_tcp_listener_open(addr));
                assert!(is_tcp_port_rebindable(addr));
            }
            Protocol::Udp => {
                assert!(!is_udp_port_held(addr));
            }
        }
    }

    Ok(())
}
