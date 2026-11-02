//! Relay load test, concurrent capacity limits, and memory saturation (`T-096`, `PERF-005`, `OPS-004`, `NET-006`).
//!
//! Asserts:
//! 1. **Allocation Capacity Scaling & Ceiling (`PERF-005`):** Scale concurrent sessions up to
//!    `max_allocations`; verify strict refusal on capacity exhaustion.
//! 2. **Per-Session Flow Saturation (`NET-006`):** Scale concurrent TCP and UDP flows up to limits;
//!    verify deterministic flow rejection and drop counting.
//! 3. **Global Memory Flood & Backpressure (`NET-006`, `OPS-004`):** Exhaust global relay memory budget
//!    under heavy packet burst; verify strict bounding, drop reasons, and zero memory leaks.
//! 4. **Token-Bucket Bandwidth Saturation & Refill (`OPS-004`):** Enforce byte rate limiting and
//!    assert burst consumption and refill recovery.
//! 5. **High-Density State Sweep Performance (`PERF-005`):** Measure sweep and lookup latency across
//!    250 active allocations and thousands of flows.

use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Instant;

use gtt_protocol::{HelloAckMapping, Protocol, SessionLimits, DEFAULT_MAX_DATAGRAM_BYTES};
use gtt_relay::{
    BackpressureDropReason, BoundedSessionQueue, GlobalRelayMemoryBudget,
    RelayAllocationSupervisor, RelayBoundAllocation, RelayCapacityConfig, RelayCapacityError,
    RelayTcpError, RelayTcpSession, RelayTunnelSession, RelayUdpSession, TokenBucketLimiter,
    DEFAULT_GLOBAL_MAX_QUEUED_BYTES, DEFAULT_MAX_QUEUE_BYTES, DEFAULT_MAX_QUEUE_FRAMES,
    DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
};

type TestResult = Result<(), Box<dyn Error>>;

fn make_test_mappings() -> Vec<HelloAckMapping> {
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

fn create_mock_bound_allocation(
    aid: &str,
    tid: &str,
    global_budget: GlobalRelayMemoryBudget,
) -> Result<RelayBoundAllocation, Box<dyn Error>> {
    let session = RelayTunnelSession::new(
        aid.to_owned(),
        tid.to_owned(),
        format!("sid-{aid}"),
        make_test_mappings(),
        5_000,
        15_000,
        30_000,
        SessionLimits {
            max_tcp_flows: 64,
            max_udp_flows: 64,
        },
        1_000,
    );

    let queue = BoundedSessionQueue::new(
        DEFAULT_MAX_QUEUE_FRAMES,
        DEFAULT_MAX_QUEUE_BYTES,
        global_budget,
        TokenBucketLimiter::new(
            DEFAULT_MAX_QUEUE_BYTES as u64,
            DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
            1_000,
        ),
    );

    let (bound, _) = RelayBoundAllocation::bind_for_session(
        session,
        queue,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true, // use ephemeral ports
    )?;

    Ok(bound)
}

// ---------------------------------------------------------------------------
// 1. Allocation Capacity Scaling & Ceiling (PERF-005, OPS-004)
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_allocation_scaling_and_capacity_ceiling() -> TestResult {
    const TEST_CAPACITY: usize = 50;
    let config = RelayCapacityConfig::new(
        TEST_CAPACITY,
        DEFAULT_GLOBAL_MAX_QUEUED_BYTES,
        DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
        DEFAULT_MAX_QUEUE_BYTES as u64,
    );

    let mut supervisor = RelayAllocationSupervisor::with_capacity(config);
    let budget = GlobalRelayMemoryBudget::new(DEFAULT_GLOBAL_MAX_QUEUED_BYTES);

    let t0 = Instant::now();

    // Scale up to TEST_CAPACITY allocations
    for i in 0..TEST_CAPACITY {
        let aid = format!("11111111-0000-4000-8000-{:012x}", i);
        let tid = format!("22222222-0000-4000-8000-{:012x}", i);
        let bound = create_mock_bound_allocation(&aid, &tid, budget.clone())?;
        supervisor.insert(bound)?;
    }

    let elapsed = t0.elapsed();
    assert_eq!(supervisor.active_allocation_count(), TEST_CAPACITY);
    println!(
        "\n [PERF-005] Successfully scaled {TEST_CAPACITY} allocations in {:.2} ms",
        elapsed.as_secs_f64() * 1000.0
    );

    // Attempt to register allocation TEST_CAPACITY + 1: must be rejected with MaxAllocationsReached
    let extra_aid = "11111111-0000-4000-8000-ffffffffffff";
    let extra_tid = "22222222-0000-4000-8000-ffffffffffff";
    let extra_bound = create_mock_bound_allocation(extra_aid, extra_tid, budget.clone())?;

    let err = match supervisor.insert(extra_bound) {
        Err(e) => e,
        Ok(()) => return Err("exceeding max_allocations must fail".into()),
    };

    assert_eq!(
        err,
        RelayCapacityError::MaxAllocationsReached {
            current: TEST_CAPACITY,
            limit: TEST_CAPACITY,
        }
    );

    // Gracefully stop 10 allocations: active count drops to TEST_CAPACITY - 10
    for i in 0..10 {
        let aid = format!("11111111-0000-4000-8000-{:012x}", i);
        let report = supervisor
            .stop_allocation(&aid, "load_test_stop")
            .ok_or("expected shutdown report")?;
        assert_eq!(report.allocation_id, aid);
    }
    assert_eq!(supervisor.active_allocation_count(), TEST_CAPACITY - 10);

    // Inserting a new allocation now succeeds
    let replacement_bound = create_mock_bound_allocation(extra_aid, extra_tid, budget)?;
    supervisor.insert(replacement_bound)?;
    assert_eq!(supervisor.active_allocation_count(), TEST_CAPACITY - 9);

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Per-Session Flow Saturation & Drop Counting (NET-006)
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_flow_saturation_and_limit_enforcement() -> TestResult {
    let mappings = make_test_mappings();
    let mut tcp_session = RelayTcpSession::new(&mappings, 64);
    let mut udp_session = RelayUdpSession::new(&mappings, 64, DEFAULT_MAX_DATAGRAM_BYTES, 60_000);

    // 1. TCP Flow Saturation: register 64 flows
    for flow_id in 1..=64 {
        let guest_ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, flow_id as u8));
        let result = tcp_session.begin_tcp_open(0, guest_ip);
        assert!(result.is_ok(), "flow {flow_id} must be accepted");
    }
    assert_eq!(tcp_session.metrics().active_tcp_flows, 64);

    // 65th TCP connection: must be rejected with FlowLimitReached
    let guest_ip_overflow = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 200));
    let overflow_err = match tcp_session.begin_tcp_open(0, guest_ip_overflow) {
        Err(e) => e,
        Ok(_) => return Err("65th TCP flow must be rejected".into()),
    };
    assert!(matches!(
        overflow_err,
        RelayTcpError::FlowLimitReached { max_tcp_flows: 64 }
    ));
    assert_eq!(tcp_session.metrics().active_tcp_flows, 64);

    // 2. UDP Flow Saturation: register 64 distinct guest socket addresses
    let payload = vec![0x42; 64];
    for flow_id in 1..=64 {
        let guest_udp =
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20)), 40000 + flow_id);
        let frame = udp_session.handle_guest_datagram(1, guest_udp, &payload, 1_000);
        assert!(frame.is_some(), "UDP flow {flow_id} must be admitted");
    }
    assert_eq!(udp_session.metrics().active_udp_flows, 64);

    // 65th UDP distinct guest endpoint: must be refused/dropped without allocating flow
    let guest_udp_overflow = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 99)), 49999);
    let dropped_frame = udp_session.handle_guest_datagram(1, guest_udp_overflow, &payload, 1_000);
    assert!(dropped_frame.is_none(), "overflow UDP flow must be dropped");
    assert_eq!(udp_session.metrics().active_udp_flows, 64);
    assert_eq!(udp_session.metrics().dropped_flow_limit, 1);

    println!(
        " [NET-006] Flow saturation verified: 64/64 TCP and 64/64 UDP held; overflow strictly rejected."
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Global Relay Memory Saturation & Backpressure (NET-006, OPS-004)
// ---------------------------------------------------------------------------

#[test]
fn test_global_relay_memory_saturation_and_backpressure_drop() -> TestResult {
    // Constrain global memory budget to 64 KiB for rapid saturation testing
    const TEST_GLOBAL_MEMORY: usize = 64 * 1024;
    let budget = GlobalRelayMemoryBudget::new(TEST_GLOBAL_MEMORY);

    // Create 4 concurrent session queues sharing this 64 KiB global budget
    const SESSIONS: usize = 4;
    let mut queues: Vec<BoundedSessionQueue> = Vec::with_capacity(SESSIONS);

    for _ in 0..SESSIONS {
        let q = BoundedSessionQueue::new(
            100,
            32 * 1024, // 32 KiB session max (would allow 4 * 32 = 128 KiB without global cap)
            budget.clone(),
            TokenBucketLimiter::new(1_000_000, 1_000_000, 1_000), // generous bandwidth limiter
        );
        queues.push(q);
    }

    let frame = vec![0x55u8; 1024]; // 1 KiB frame

    // Enqueue 16 frames per queue = 64 KiB total -> exactly saturates global budget
    for q in queues.iter_mut() {
        for _ in 0..16 {
            q.try_enqueue(&frame, 1_000)?;
        }
    }

    assert_eq!(budget.current_bytes(), TEST_GLOBAL_MEMORY);
    println!(" [NET-006] Global budget saturated to capacity: {TEST_GLOBAL_MEMORY} B");

    // Any further frame enqueued on ANY of the 4 sessions must be dropped with GlobalMemoryCapReached
    let q0 = queues.get_mut(0).ok_or("missing queue 0")?;
    let err = match q0.try_enqueue(&frame, 1_000) {
        Err(e) => e,
        Ok(()) => return Err("enqueuing past global budget must fail".into()),
    };
    assert_eq!(err, BackpressureDropReason::GlobalMemoryCapReached);
    assert_eq!(q0.metrics().dropped_global_memory_cap, 1);
    assert_eq!(q0.metrics().frames_dropped, 1);

    // Pop 4 frames from queue 0 (frees 4 KiB in global budget)
    for _ in 0..4 {
        assert!(q0.pop_front().is_some());
    }
    assert_eq!(budget.current_bytes(), TEST_GLOBAL_MEMORY - 4096);

    // Enqueueing on queue 1 now succeeds for up to 4 KiB
    let q1 = queues.get_mut(1).ok_or("missing queue 1")?;
    for _ in 0..4 {
        q1.try_enqueue(&frame, 1_000)?;
    }
    assert_eq!(budget.current_bytes(), TEST_GLOBAL_MEMORY);

    // Clearing all queues frees all memory
    for q in queues.iter_mut() {
        q.clear();
    }
    assert_eq!(budget.current_bytes(), 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// 4. Token-Bucket Bandwidth Saturation & Refill Recovery (OPS-004)
// ---------------------------------------------------------------------------

#[test]
fn test_bandwidth_saturation_and_refill_recovery() -> TestResult {
    // 10 KiB burst capacity, 10 KiB/sec sustained refill
    const CAPACITY: u64 = 10 * 1024;
    const RATE: u64 = 10 * 1024;
    let budget = GlobalRelayMemoryBudget::new(DEFAULT_GLOBAL_MAX_QUEUED_BYTES);

    let limiter = TokenBucketLimiter::new(CAPACITY, RATE, 1_000);
    let mut queue = BoundedSessionQueue::new(100, DEFAULT_MAX_QUEUE_BYTES, budget, limiter);

    let chunk = vec![0xaa; 2048]; // 2 KiB chunk

    // Send 5 chunks (10 KiB total) at t = 1,000 ms -> exhausts token bucket
    for _ in 0..5 {
        queue.try_enqueue(&chunk, 1_000)?;
    }
    assert_eq!(queue.len(), 5);

    // 6th chunk at t = 1,000 ms: must be rejected with BandwidthLimitExceeded
    let err = match queue.try_enqueue(&chunk, 1_000) {
        Err(e) => e,
        Ok(()) => return Err("bandwidth bucket must be exhausted".into()),
    };
    assert_eq!(err, BackpressureDropReason::BandwidthLimitExceeded);
    assert_eq!(queue.metrics().dropped_bandwidth_exceeded, 1);

    // Advance simulated time by 500 ms (t = 1,500 ms) -> refills 5 KiB tokens
    // We can now send two 2 KiB chunks (4 KiB total)
    queue.try_enqueue(&chunk, 1_500)?;
    queue.try_enqueue(&chunk, 1_500)?;
    assert_eq!(queue.len(), 7);

    // Third chunk at t = 1,500 ms: exceeds refilled tokens and drops
    let err2 = match queue.try_enqueue(&chunk, 1_500) {
        Err(e) => e,
        Ok(()) => return Err("remaining tokens must be exhausted".into()),
    };
    assert_eq!(err2, BackpressureDropReason::BandwidthLimitExceeded);

    // Advance to t = 2,000 ms (another 500 ms = 5 KiB refilled)
    queue.try_enqueue(&chunk, 2_000)?;
    assert_eq!(queue.len(), 8);

    println!(" [OPS-004] Bandwidth token bucket burst exhaustion and continuous refill verified.");
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. High-Density State Sweep & Lookup Performance (PERF-005)
// ---------------------------------------------------------------------------

#[test]
fn test_high_density_state_sweep_performance() -> TestResult {
    const BENCHMARK_ALLOCATIONS: usize = 250;
    let config = RelayCapacityConfig::new(
        BENCHMARK_ALLOCATIONS,
        DEFAULT_GLOBAL_MAX_QUEUED_BYTES,
        DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
        DEFAULT_MAX_QUEUE_BYTES as u64,
    );

    let mut supervisor = RelayAllocationSupervisor::with_capacity(config);
    let budget = GlobalRelayMemoryBudget::new(DEFAULT_GLOBAL_MAX_QUEUED_BYTES);

    let mut allocation_ids = Vec::with_capacity(BENCHMARK_ALLOCATIONS);

    // Provision 250 allocations
    for i in 0..BENCHMARK_ALLOCATIONS {
        let aid = format!("aaaaaaaa-0000-4000-8000-{:012x}", i);
        let tid = format!("bbbbbbbb-0000-4000-8000-{:012x}", i);
        let bound = create_mock_bound_allocation(&aid, &tid, budget.clone())?;
        supervisor.insert(bound)?;
        allocation_ids.push(aid);
    }
    assert_eq!(supervisor.active_allocation_count(), BENCHMARK_ALLOCATIONS);

    // Measure lookup performance across all 250 allocations
    let t_lookup = Instant::now();
    for aid in &allocation_ids {
        assert!(supervisor.get(aid).is_some());
    }
    let lookup_elapsed = t_lookup.elapsed();
    let avg_lookup_us =
        (lookup_elapsed.as_secs_f64() * 1_000_000.0) / (BENCHMARK_ALLOCATIONS as f64);

    println!(
        "\n=======================================================\n\
         [PERF-005] High-Density State Benchmark (250 Allocations)\n\
         Lookup Time: {:.2} us avg per allocation (total: {:.2} ms)\n\
         =======================================================",
        avg_lookup_us,
        lookup_elapsed.as_secs_f64() * 1000.0
    );

    // Assert lookup time is under 10 us per allocation
    assert!(
        avg_lookup_us < 10.0,
        "average lookup time exceeded 10 us: {avg_lookup_us} us"
    );

    // Simulate grace period expiry sweep
    let now_ms = 40_000;
    // Keep 200 authoritative active, drop 50
    let authoritative_active: Vec<String> = allocation_ids
        .get(..200)
        .ok_or("expected at least 200 allocations")?
        .to_vec();

    let t_sweep = Instant::now();
    let reports = supervisor.recover_and_sweep(&authoritative_active, now_ms);
    let sweep_elapsed = t_sweep.elapsed();

    assert_eq!(reports.len(), 50);
    assert_eq!(supervisor.active_allocation_count(), 200);

    println!(
        " Sweep Time (50 evicted): {:.2} ms (total)\n\
         Remaining Active Allocations: {}\n\
         =======================================================",
        sweep_elapsed.as_secs_f64() * 1000.0,
        supervisor.active_allocation_count()
    );

    assert!(
        sweep_elapsed.as_secs_f64() < 0.05,
        "sweep took more than 50 ms: {:.2} ms",
        sweep_elapsed.as_secs_f64() * 1000.0
    );

    Ok(())
}
