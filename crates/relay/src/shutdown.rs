//! Graceful shutdown, crash-recovery listener cleanup, and port release (`T-067`, `NET-008`,
//! `contracts.md` §§10, 12).
//!
//! ## Guarantees (`NET-008`)
//!
//! * Every active allocation owns its bound public [`TcpListener`] and [`UdpSocket`] handles inside
//!   [`RelayBoundAllocation`].
//! * On graceful stop ([`RelayAllocationSupervisor::stop_allocation`]) or crash/orphan recovery
//!   ([`RelayAllocationSupervisor::recover_orphaned_allocations`]):
//!   1. All bound public TCP listeners and UDP sockets for the allocation are dropped immediately
//!      so the OS releases the ports (`NET-008`).
//!   2. All active TCP streams and UDP flow-table entries in [`RelayTunnelSession`] are closed and
//!      cleared.
//!   3. All queued outbound frames in [`BoundedSessionQueue`] are cleared and their byte reservation
//!      is returned to [`GlobalRelayMemoryBudget`].
//!   4. A length-prefixed `goodbye` control frame (`ControlMessage::Goodbye`) is encoded when
//!      stopping gracefully (`contracts.md` §10).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};

use gtt_protocol::{
    encode_control_frame, ControlMessage, GoodbyeMessage, HelloAckMapping, Protocol,
};

use crate::limits::{BoundedSessionQueue, RelayCapacityConfig, RelayCapacityError};
use crate::session::{RelaySessionConnectivity, RelayTunnelSession};

/// Summary returned when an allocation is stopped or recovered (`T-067`, `NET-008`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Allocation UUID that was cleaned up.
    pub allocation_id: String,
    /// Public socket addresses whose listeners were closed and released.
    pub released_public_addrs: Vec<(Protocol, SocketAddr)>,
    /// Number of active TCP flows closed during shutdown.
    pub closed_tcp_flows: u16,
    /// Number of active UDP flows cleared during shutdown.
    pub cleared_udp_flows: u16,
    /// Number of queued outbound bytes returned to the global relay memory budget.
    pub released_queue_bytes: usize,
    /// Length-prefixed `goodbye` control frame to send before closing the QUIC connection (if graceful).
    pub goodbye_frame: Option<Vec<u8>>,
}

/// Bound OS listeners (`TcpListener` / `UdpSocket`) and session state for one relay allocation
/// (`T-067`, `NET-008`).
#[derive(Debug)]
pub struct RelayBoundAllocation {
    /// Underlying tunnel session state machine and flow tables.
    pub session: RelayTunnelSession,
    /// Bounded outbound send queue tied to the relay's global memory budget.
    pub send_queue: BoundedSessionQueue,
    tcp_listeners: Vec<(u16, SocketAddr, TcpListener)>,
    udp_sockets: Vec<(u16, SocketAddr, UdpSocket)>,
}

impl RelayBoundAllocation {
    /// Bind real public TCP listeners and UDP sockets on `bind_ip` for each mapping in
    /// `session.mappings()` (`contracts.md` §§7, 10, `NET-008`).
    ///
    /// If `use_ephemeral_ports` is true (used in local network tests where privileged/arbitrary
    /// fixed ports may collide), port `0` is bound and the OS-assigned port is recorded in the
    /// returned [`HelloAckMapping`] list.
    pub fn bind_for_session(
        mut session: RelayTunnelSession,
        send_queue: BoundedSessionQueue,
        bind_ip: IpAddr,
        use_ephemeral_ports: bool,
    ) -> io::Result<(Self, Vec<HelloAckMapping>)> {
        let mut tcp_listeners = Vec::new();
        let mut udp_sockets = Vec::new();
        let mut bound_mappings = Vec::with_capacity(session.mappings().len());

        for mapping in session.mappings() {
            let target_port = if use_ephemeral_ports {
                0
            } else {
                mapping.public_port
            };
            let bind_addr = SocketAddr::new(bind_ip, target_port);
            match mapping.protocol {
                Protocol::Tcp => {
                    let listener = TcpListener::bind(bind_addr)?;
                    listener.set_nonblocking(true)?;
                    let local_addr = listener.local_addr()?;
                    tcp_listeners.push((mapping.index, local_addr, listener));
                    bound_mappings.push(HelloAckMapping {
                        index: mapping.index,
                        mapping_key: mapping.mapping_key.clone(),
                        protocol: Protocol::Tcp,
                        public_port: local_addr.port(),
                    });
                }
                Protocol::Udp => {
                    let socket = UdpSocket::bind(bind_addr)?;
                    socket.set_nonblocking(true)?;
                    let local_addr = socket.local_addr()?;
                    udp_sockets.push((mapping.index, local_addr, socket));
                    bound_mappings.push(HelloAckMapping {
                        index: mapping.index,
                        mapping_key: mapping.mapping_key.clone(),
                        protocol: Protocol::Udp,
                        public_port: local_addr.port(),
                    });
                }
            }
        }

        // Keep session mappings synchronized if ephemeral ports were assigned.
        if use_ephemeral_ports {
            let sid = match session.connectivity() {
                RelaySessionConnectivity::Active { session_id } => session_id.clone(),
                _ => "00000000-0000-4000-8000-000000000000".to_owned(),
            };
            session = RelayTunnelSession::new(
                session.allocation_id().to_owned(),
                session.tunnel_id().to_owned(),
                sid,
                bound_mappings.clone(),
                5_000,
                15_000,
                30_000,
                gtt_protocol::SessionLimits {
                    max_tcp_flows: 64,
                    max_udp_flows: 64,
                },
                0,
            );
        }

        Ok((
            Self {
                session,
                send_queue,
                tcp_listeners,
                udp_sockets,
            },
            bound_mappings,
        ))
    }

    /// Return the bound `(Protocol, SocketAddr)` list for this allocation.
    pub fn bound_public_addrs(&self) -> Vec<(Protocol, SocketAddr)> {
        let mut out = Vec::with_capacity(self.tcp_listeners.len() + self.udp_sockets.len());
        for &(_, addr, _) in &self.tcp_listeners {
            out.push((Protocol::Tcp, addr));
        }
        for &(_, addr, _) in &self.udp_sockets {
            out.push((Protocol::Udp, addr));
        }
        out
    }

    /// Consume and shut down this allocation, closing all bound TCP listeners, UDP sockets, active
    /// flows, and queued buffers (`NET-008`).
    pub fn shutdown(mut self, goodbye_reason: Option<&str>) -> ShutdownReport {
        let released_public_addrs = self.bound_public_addrs();
        let closed_tcp_flows = self.session.tcp.metrics().active_tcp_flows;
        let cleared_udp_flows = self.session.udp.metrics().active_udp_flows;
        let released_queue_bytes = self.send_queue.metrics().queued_bytes;

        // Explicitly drop all TCP listeners and UDP sockets so OS ports are released immediately.
        self.tcp_listeners.clear();
        self.udp_sockets.clear();
        self.send_queue.clear();
        self.session.release();

        let goodbye_frame = goodbye_reason.and_then(|reason| {
            encode_control_frame(&ControlMessage::Goodbye(GoodbyeMessage {
                reason: reason.to_owned(),
            }))
            .ok()
        });

        ShutdownReport {
            allocation_id: self.session.allocation_id().to_owned(),
            released_public_addrs,
            closed_tcp_flows,
            cleared_udp_flows,
            released_queue_bytes,
            goodbye_frame,
        }
    }
}

/// Relay-wide supervisor tracking bound allocations and performing graceful stop, grace-expiry
/// cleanup, crash recovery, and instance capacity limit enforcement (`T-067`, `T-096`, `NET-008`, `PERF-005`).
#[derive(Debug, Default)]
pub struct RelayAllocationSupervisor {
    allocations: HashMap<String, RelayBoundAllocation>,
    capacity_config: RelayCapacityConfig,
}

impl RelayAllocationSupervisor {
    /// Construct an empty [`RelayAllocationSupervisor`] with default capacity limits (`PERF-005`).
    pub fn new() -> Self {
        Self {
            allocations: HashMap::new(),
            capacity_config: RelayCapacityConfig::default(),
        }
    }

    /// Construct a [`RelayAllocationSupervisor`] with explicit capacity configuration (`PERF-005`, `OPS-004`).
    pub fn with_capacity(capacity_config: RelayCapacityConfig) -> Self {
        Self {
            allocations: HashMap::new(),
            capacity_config,
        }
    }

    /// Current capacity configuration for this supervisor.
    pub fn capacity(&self) -> &RelayCapacityConfig {
        &self.capacity_config
    }

    /// Number of currently bound allocations on the relay.
    pub fn active_allocation_count(&self) -> usize {
        self.allocations.len()
    }

    /// Attempt to register a bound allocation, enforcing the `max_allocations` instance capacity limit
    /// (`PERF-005`, `OPS-004`).
    pub fn try_insert(
        &mut self,
        allocation: RelayBoundAllocation,
    ) -> Result<(), RelayCapacityError> {
        let aid = allocation.session.allocation_id();
        if !self.allocations.contains_key(aid)
            && self.allocations.len() >= self.capacity_config.max_allocations
        {
            return Err(RelayCapacityError::MaxAllocationsReached {
                current: self.allocations.len(),
                limit: self.capacity_config.max_allocations,
            });
        }
        self.allocations
            .insert(allocation.session.allocation_id().to_owned(), allocation);
        Ok(())
    }

    /// Register a bound allocation, enforcing capacity limits (`PERF-005`, `OPS-004`).
    pub fn insert(&mut self, allocation: RelayBoundAllocation) -> Result<(), RelayCapacityError> {
        self.try_insert(allocation)
    }

    /// Borrow a bound allocation by `allocation_id`.
    pub fn get(&self, allocation_id: &str) -> Option<&RelayBoundAllocation> {
        self.allocations.get(allocation_id)
    }

    /// Mutably borrow a bound allocation by `allocation_id`.
    pub fn get_mut(&mut self, allocation_id: &str) -> Option<&mut RelayBoundAllocation> {
        self.allocations.get_mut(allocation_id)
    }

    /// Gracefully stop a single allocation by `allocation_id`, closing all sockets and emitting a
    /// `goodbye` frame (`NET-008`).
    pub fn stop_allocation(&mut self, allocation_id: &str, reason: &str) -> Option<ShutdownReport> {
        let alloc = self.allocations.remove(allocation_id)?;
        Some(alloc.shutdown(Some(reason)))
    }

    /// Reconcile bound allocations against the control-plane's authoritative active allocation IDs
    /// after a crash recovery or state poll (`REL-004`, `NET-008`), and evict any allocation whose
    /// grace period has expired at `now_ms`.
    pub fn recover_and_sweep(
        &mut self,
        authoritative_active_ids: &[String],
        now_ms: u64,
    ) -> Vec<ShutdownReport> {
        let mut to_remove = Vec::new();
        for (aid, bound) in &mut self.allocations {
            let state = bound.session.check_liveness(now_ms);
            if matches!(state, RelaySessionConnectivity::Released)
                || !authoritative_active_ids.iter().any(|id| id == aid)
            {
                to_remove.push(aid.clone());
            }
        }

        let mut reports = Vec::with_capacity(to_remove.len());
        for aid in to_remove {
            if let Some(bound) = self.allocations.remove(&aid) {
                reports.push(bound.shutdown(None));
            }
        }
        reports
    }

    /// Shut down the entire relay instance, closing all listeners and releasing all ports (`NET-008`).
    pub fn shutdown_all(&mut self, reason: &str) -> Vec<ShutdownReport> {
        let keys: Vec<String> = self.allocations.keys().cloned().collect();
        let mut reports = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(bound) = self.allocations.remove(&key) {
                reports.push(bound.shutdown(Some(reason)));
            }
        }
        reports
    }
}
