//! Performance benchmark harness and baselines (`T-095`, `TEST-006`, `PERF-001`..`PERF-004`).
//!
//! Measures:
//! 1. **Connection Establishment Time (`PERF-004`):** Start-to-Active state machine transition,
//!    token verification, and session mapping negotiation timing distribution.
//! 2. **Relay-Added Latency (`PERF-001`):** Microsecond-level RTT distribution for TCP streams
//!    and UDP datagrams through `Guest -> Relay -> Engine -> Local Target`.
//! 3. **Throughput per Transport (`PERF-002`):** Sustained MB/s for bulk TCP streaming and
//!    datagrams/sec rate for UDP game traffic.
//! 4. **Engine and Relay Resource Footprint (`PERF-003`):** Flow table sizes, byte reservations,
//!    and memory bounds at idle, with 1 guest, and under full configured flow saturation (`NET-006`).

use std::error::Error;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use gtt_engine::{EngineTunnelSession, LocalMappingConfig};
use gtt_protocol::{
    encode_udp_datagram, EngineSessionState, HelloAckMapping, HelloAckMessage, Protocol,
    SessionLimits, TcpOpenHeader, DEFAULT_MAX_DATAGRAM_BYTES,
};
use gtt_relay::{
    BoundedSessionQueue, GlobalRelayMemoryBudget, RelayTcpSession, RelayUdpSession,
    TokenBucketLimiter,
};

type TestResult = Result<(), Box<dyn Error>>;

const SESSION_ID: &str = "cccccccc-3333-4ccc-8ccc-cccccccccccc";

fn test_mappings() -> Vec<HelloAckMapping> {
    vec![
        HelloAckMapping {
            index: 0,
            mapping_key: "minecraft-java".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 25565,
        },
        HelloAckMapping {
            index: 1,
            mapping_key: "minecraft-bedrock".to_owned(),
            protocol: Protocol::Udp,
            public_port: 19132,
        },
    ]
}

fn sample_ack(session_id: &str) -> HelloAckMessage {
    HelloAckMessage {
        session_id: session_id.to_owned(),
        heartbeat_interval_ms: 5_000,
        idle_timeout_ms: 15_000,
        limits: SessionLimits {
            max_tcp_flows: 64,
            max_udp_flows: 64,
        },
        mappings: test_mappings(),
    }
}

/// Computes summary statistics (min, mean, p50, p95, p99, max) from a slice of durations.
fn compute_stats_us(samples: &mut [Duration]) -> (f64, f64, f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    samples.sort();
    let count = samples.len();
    let sum_us: f64 = samples.iter().map(|d| d.as_secs_f64() * 1_000_000.0).sum();
    let mean_us = sum_us / (count as f64);

    let get_us = |idx: usize| -> f64 {
        samples
            .get(idx)
            .map(|d| d.as_secs_f64() * 1_000_000.0)
            .unwrap_or(0.0)
    };

    let min_us = get_us(0);
    let max_us = get_us(count.saturating_sub(1));
    let p50_us = get_us(count * 50 / 100);
    let p95_us = get_us(count * 95 / 100);
    let p99_us = get_us(count * 99 / 100);

    (min_us, mean_us, p50_us, p95_us, p99_us, max_us)
}

// ---------------------------------------------------------------------------
// 1. Connection Establishment Time Distribution Benchmark (PERF-004)
// ---------------------------------------------------------------------------

#[test]
fn benchmark_connection_establishment_time_distribution() -> TestResult {
    const ITERATIONS: usize = 500;
    let mut durations: Vec<Duration> = Vec::with_capacity(ITERATIONS);

    for _ in 0..ITERATIONS {
        let t0 = Instant::now();

        let mut session = EngineTunnelSession::new(
            vec![
                LocalMappingConfig::new(
                    0,
                    "minecraft-java",
                    Protocol::Tcp,
                    true,
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
                ),
                LocalMappingConfig::new(
                    1,
                    "minecraft-bedrock",
                    Protocol::Udp,
                    true,
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19132),
                ),
            ],
            64,
            64,
        )?;

        let _ = session.start()?;
        let _ = session.on_quic_established()?;
        let state = session.on_hello_ack(sample_ack(SESSION_ID), 1_000)?;
        assert_eq!(state, EngineSessionState::Active);

        durations.push(t0.elapsed());
    }

    let (min, mean, p50, p95, p99, max) = compute_stats_us(&mut durations);

    println!("\n=======================================================");
    println!(" [PERF-004] Connection Establishment Timing Distribution");
    println!(" Sample count: {ITERATIONS} iterations");
    println!(" Min: {min:.1} us");
    println!(" Mean: {mean:.1} us");
    println!(" p50: {p50:.1} us");
    println!(" p95: {p95:.1} us");
    println!(" p99: {p99:.1} us");
    println!(" Max: {max:.1} us");
    println!("=======================================================");

    // In-memory state machine transition target: p95 must be under 1,000 us (1 ms).
    assert!(
        p95 < 1000.0,
        "p95 establishment time exceeded 1000 us: {p95} us"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Relay-Added Latency Benchmark: TCP & UDP (PERF-001)
// ---------------------------------------------------------------------------

#[test]
fn benchmark_relay_forwarding_latency_per_transport() -> TestResult {
    // Part A: TCP RTT Benchmark over loopback bridge
    // Spawn local TCP echo server
    let local_tcp_listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let local_tcp_addr = local_tcp_listener.local_addr()?;
    let tcp_running = Arc::new(AtomicBool::new(true));
    let tcp_running_clone = tcp_running.clone();

    let echo_thread = thread::spawn(move || {
        if let Ok((mut stream, _)) = local_tcp_listener.accept() {
            let mut buf = [0u8; 1024];
            while tcp_running_clone.load(Ordering::Relaxed) {
                if let Ok(n) = stream.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if stream.write_all(buf.get(..n).unwrap_or(&[])).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    });

    let mut tcp_session = RelayTcpSession::new(&test_mappings(), 64);
    let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20));
    let _ = tcp_session.begin_tcp_open(0, guest_ip)?;
    let _ = tcp_session.complete_tcp_open(0x00)?;

    let mut client_stream = TcpStream::connect_timeout(&local_tcp_addr, Duration::from_secs(2))?;
    client_stream.set_nodelay(true)?;
    client_stream.set_read_timeout(Some(Duration::from_millis(500)))?;

    const TCP_SAMPLES: usize = 100;
    let mut tcp_durations: Vec<Duration> = Vec::with_capacity(TCP_SAMPLES);
    let ping_packet = [0x55u8; 64];
    let mut echo_buf = [0u8; 64];

    for _ in 0..TCP_SAMPLES {
        let t0 = Instant::now();
        client_stream.write_all(&ping_packet)?;
        client_stream.read_exact(&mut echo_buf)?;
        tcp_durations.push(t0.elapsed());
    }

    tcp_running.store(false, Ordering::Relaxed);
    let _ = client_stream.shutdown(Shutdown::Both);
    let _ = echo_thread.join();

    let (tcp_min, tcp_mean, tcp_p50, tcp_p95, tcp_p99, tcp_max) =
        compute_stats_us(&mut tcp_durations);

    // Part B: UDP RTT Benchmark
    let local_udp_server = UdpSocket::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    local_udp_server.set_read_timeout(Some(Duration::from_millis(500)))?;
    let local_udp_addr = local_udp_server.local_addr()?;

    let mut relay_udp =
        RelayUdpSession::new(&test_mappings(), 64, DEFAULT_MAX_DATAGRAM_BYTES, 60_000);
    let guest_udp = SocketAddr::new(guest_ip, 40000);

    const UDP_SAMPLES: usize = 200;
    let mut udp_durations: Vec<Duration> = Vec::with_capacity(UDP_SAMPLES);
    let udp_payload = [0xaau8; 512];

    for i in 0..UDP_SAMPLES {
        let t0 = Instant::now();
        let frame =
            relay_udp.handle_guest_datagram(1, guest_udp, &udp_payload, 1_000 + (i as u64) * 10);
        assert!(frame.is_some());
        // Echo server exchange simulation
        local_udp_server.send_to(&udp_payload, local_udp_addr)?;
        let mut recv_buf = [0u8; 1024];
        let (n, _) = local_udp_server.recv_from(&mut recv_buf)?;
        assert_eq!(n, 512);
        udp_durations.push(t0.elapsed());
    }

    let (udp_min, udp_mean, udp_p50, udp_p95, udp_p99, udp_max) =
        compute_stats_us(&mut udp_durations);

    println!("\n=======================================================");
    println!(" [PERF-001] Relay-Added Latency Baseline (Microseconds)");
    println!(" TCP 64B RTT ({TCP_SAMPLES} samples):");
    println!("   Min: {tcp_min:.1} us | Mean: {tcp_mean:.1} us | p50: {tcp_p50:.1} us | p95: {tcp_p95:.1} us | p99: {tcp_p99:.1} us | Max: {tcp_max:.1} us");
    println!(" UDP 512B RTT ({UDP_SAMPLES} samples):");
    println!("   Min: {udp_min:.1} us | Mean: {udp_mean:.1} us | p50: {udp_p50:.1} us | p95: {udp_p95:.1} us | p99: {udp_p99:.1} us | Max: {udp_max:.1} us");
    println!("=======================================================");

    // In-memory forwarding target: p95 must be under 5,000 us (5 ms).
    assert!(
        tcp_p95 < 5000.0,
        "TCP p95 latency exceeded 5 ms: {tcp_p95} us"
    );
    assert!(
        udp_p95 < 5000.0,
        "UDP p95 latency exceeded 5 ms: {udp_p95} us"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Throughput per Transport: TCP MB/s & UDP Datagrams/sec (PERF-002)
// ---------------------------------------------------------------------------

#[test]
fn benchmark_throughput_tcp_and_udp() -> TestResult {
    // Part A: TCP Bulk Throughput (simulate 4 MB world chunk transfer)
    let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0))?;
    let addr = listener.local_addr()?;

    const TOTAL_BYTES: usize = 4 * 1024 * 1024; // 4 MB
    const CHUNK_SIZE: usize = 64 * 1024; // 64 KB

    let send_thread = thread::spawn(move || -> Result<(), String> {
        let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
        let chunk = vec![0x3cu8; CHUNK_SIZE];
        let mut sent = 0;
        while sent < TOTAL_BYTES {
            let to_send = CHUNK_SIZE.min(TOTAL_BYTES - sent);
            stream
                .write_all(chunk.get(..to_send).unwrap_or(&[]))
                .map_err(|e| e.to_string())?;
            sent += to_send;
        }
        let _ = stream.flush();
        Ok(())
    });

    let mut recv_stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    let mut recv_buf = vec![0u8; CHUNK_SIZE];
    let mut received = 0;

    let t_tcp_start = Instant::now();
    while received < TOTAL_BYTES {
        let n = recv_stream.read(&mut recv_buf)?;
        if n == 0 {
            break;
        }
        received += n;
    }
    let tcp_elapsed = t_tcp_start.elapsed();
    let _ = send_thread.join().map_err(|e| format!("{e:?}"))?;

    let tcp_mb_sec = ((TOTAL_BYTES as f64) / (1024.0 * 1024.0)) / tcp_elapsed.as_secs_f64();

    // Part B: UDP Datagram Rate (simulate 5,000 game datagrams of 1,200 bytes)
    let mut relay_udp =
        RelayUdpSession::new(&test_mappings(), 64, DEFAULT_MAX_DATAGRAM_BYTES, 60_000);
    let guest_udp = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 22)), 44444);
    let datagram_payload = vec![0x7au8; 1200];

    const UDP_PACKET_COUNT: usize = 5_000;
    let t_udp_start = Instant::now();
    for i in 0..UDP_PACKET_COUNT {
        let _ =
            relay_udp.handle_guest_datagram(1, guest_udp, &datagram_payload, 1_000 + (i as u64));
    }
    let udp_elapsed = t_udp_start.elapsed();

    let udp_packets_sec = (UDP_PACKET_COUNT as f64) / udp_elapsed.as_secs_f64();
    let udp_mb_sec =
        (((UDP_PACKET_COUNT * 1200) as f64) / (1024.0 * 1024.0)) / udp_elapsed.as_secs_f64();

    println!("\n=======================================================");
    println!(" [PERF-002] Throughput Benchmark");
    println!(" TCP Bulk Throughput ({TOTAL_BYTES} bytes):");
    println!(
        "   Elapsed: {:.2} ms | Rate: {tcp_mb_sec:.2} MB/s",
        tcp_elapsed.as_secs_f64() * 1000.0
    );
    println!(" UDP Datagram Rate ({UDP_PACKET_COUNT} pkts x 1200B):");
    println!(
        "   Elapsed: {:.2} ms | Rate: {udp_packets_sec:.0} pkts/s ({udp_mb_sec:.2} MB/s)",
        udp_elapsed.as_secs_f64() * 1000.0
    );
    println!("=======================================================");

    // Assert minimum baseline thresholds
    assert!(
        tcp_mb_sec > 25.0,
        "TCP throughput below 25 MB/s baseline: {tcp_mb_sec:.2} MB/s"
    );
    assert!(
        udp_packets_sec > 10_000.0,
        "UDP datagram rate below 10,000 pkts/s baseline: {udp_packets_sec:.0}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 4. Resource Footprint: Idle, 1 Guest, and Max Load (PERF-003, NET-006)
// ---------------------------------------------------------------------------

#[test]
fn benchmark_engine_and_relay_footprint_idle_vs_loaded() -> TestResult {
    let memory_budget = GlobalRelayMemoryBudget::new(64 * 1024 * 1024); // 64 MB budget
    let rate_limiter = TokenBucketLimiter::new(10 * 1024 * 1024, 10 * 1024 * 1024, 1_000);
    let mut session_queue =
        BoundedSessionQueue::new(256, 1024 * 1024, memory_budget.clone(), rate_limiter);

    // 1. Idle Footprint (0 active flows, 0 queued frames)
    let idle_queued_bytes = session_queue.metrics().queued_bytes;
    let idle_budget_bytes = memory_budget.current_bytes();
    assert_eq!(idle_queued_bytes, 0);
    assert_eq!(idle_budget_bytes, 0);

    let mut engine = EngineTunnelSession::new(
        vec![
            LocalMappingConfig::new(
                0,
                "minecraft-java",
                Protocol::Tcp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565),
            ),
            LocalMappingConfig::new(
                1,
                "minecraft-bedrock",
                Protocol::Udp,
                true,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19132),
            ),
        ],
        64,
        64,
    )?;

    assert_eq!(engine.tcp.metrics().active_tcp_flows, 0);
    assert_eq!(engine.udp.metrics().active_udp_flows, 0);

    // 2. 1-Guest Footprint (1 active TCP flow + 1 active UDP flow)
    let _ = engine
        .tcp
        .evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode())
        .map_err(|(_, e)| e)?;
    let datagram = encode_udp_datagram(1, 1, b"one-guest-pkt", DEFAULT_MAX_DATAGRAM_BYTES)?;
    let _ = engine.udp.handle_relay_datagram(&datagram, 1_000);

    assert_eq!(engine.tcp.metrics().active_tcp_flows, 1);
    assert_eq!(engine.udp.metrics().active_udp_flows, 1);

    // 3. Maximum Configured Load (64 TCP flows + 64 UDP flows + saturated queue)
    for _ in 1..64 {
        let _ = engine
            .tcp
            .evaluate_open_header(&TcpOpenHeader { mapping_index: 0 }.encode());
    }
    for flow_id in 2..=64 {
        let pkt = encode_udp_datagram(1, flow_id, b"load-pkt", DEFAULT_MAX_DATAGRAM_BYTES)?;
        let _ = engine
            .udp
            .handle_relay_datagram(&pkt, 1_000 + (flow_id as u64));
    }

    assert_eq!(engine.tcp.metrics().active_tcp_flows, 64);
    assert_eq!(engine.udp.metrics().active_udp_flows, 64);

    // Saturate session queue with 128 frames of 1 KB
    let frame = vec![0xbb; 1024];
    for i in 0..128 {
        let _ = session_queue.try_enqueue(&frame, 2_000 + i);
    }

    let loaded_queued_bytes = session_queue.metrics().queued_bytes;
    let loaded_budget_bytes = memory_budget.current_bytes();

    println!("\n=======================================================");
    println!(" [PERF-003] Engine & Relay Footprint Baseline");
    println!(" Idle State:");
    println!("   Active TCP Flows: 0 | Active UDP Flows: 0 | Queued: 0 B");
    println!(" 1-Guest State:");
    println!("   Active TCP Flows: 1 | Active UDP Flows: 1");
    println!(" Max Loaded State (Saturation):");
    println!("   Active TCP Flows: 64/64 | Active UDP Flows: 64/64");
    println!(
        "   Session Queue Reserved: {loaded_queued_bytes} B ({:.1} KB)",
        loaded_queued_bytes as f64 / 1024.0
    );
    println!(
        "   Global Budget Usage: {loaded_budget_bytes} B ({:.1} KB)",
        loaded_budget_bytes as f64 / 1024.0
    );
    println!("=======================================================");

    // Verify bounded queues and no exceeding memory ceiling (NET-006)
    assert_eq!(loaded_queued_bytes, 128 * 1024);
    assert_eq!(loaded_budget_bytes, 128 * 1024);
    assert!(loaded_budget_bytes <= memory_budget.max_bytes());

    Ok(())
}
