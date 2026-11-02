//! Per-session and global relay limits, backpressure queues, and bandwidth rate limiting
//! (`T-064`, `NET-006`, `OPS-004`, `AGENTS.md` §4).
//!
//! ## Enforced bounds (`NET-006`, `OPS-004`)
//!
//! 1. **Per-session queue frame bound (`max_queue_frames`):** refuses/drops frames when the session
//!    outbound queue reaches `max_queue_frames`.
//! 2. **Per-session queue byte bound (`max_queue_bytes`):** refuses/drops frames when the session's
//!    queued bytes would exceed `max_queue_bytes`.
//! 3. **Global relay memory cap (`global_max_queued_bytes`):** refuses/drops frames across all
//!    sessions when the relay's aggregate queued bytes would exceed `global_max_queued_bytes`,
//!    preventing memory exhaustion under flood (`NET-006`).
//! 4. **Per-session and global token-bucket bandwidth limit (`max_bandwidth_bytes_per_sec`):**
//!    throttles/drops excess traffic when the session or relay exceeds its configured byte rate
//!    (`OPS-004`).

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Default maximum frames queued per session (`256`, `AGENTS.md` §4).
pub const DEFAULT_MAX_QUEUE_FRAMES: usize = 256;

/// Default maximum bytes queued per session (`256 KiB`, `AGENTS.md` §4).
pub const DEFAULT_MAX_QUEUE_BYTES: usize = 262_144;

/// Default global relay queued-memory cap (`16 MiB`, `NET-006`, `OPS-004`).
pub const DEFAULT_GLOBAL_MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;

/// Default per-session bandwidth limit (`2 MiB/s`, *(proposal)*).
pub const DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;

use crate::{DEFAULT_MAX_ALLOCATIONS, MAX_ALLOCATIONS_CEILING};

/// Default maximum total concurrent flows per relay instance (`256 * 128 = 32,768`, `PERF-005`).
pub const DEFAULT_MAX_TOTAL_FLOWS: usize = DEFAULT_MAX_ALLOCATIONS * 128;

/// Error returned when an operation exceeds relay instance capacity (`PERF-005`, `OPS-004`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayCapacityError {
    /// Allocation limit reached on this relay instance.
    MaxAllocationsReached { current: usize, limit: usize },
}

impl std::fmt::Display for RelayCapacityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxAllocationsReached { current, limit } => {
                write!(
                    f,
                    "relay allocation capacity limit reached: {current} active allocations (limit {limit})"
                )
            }
        }
    }
}

impl std::error::Error for RelayCapacityError {}

/// Relay instance capacity configuration establishing limits on concurrent allocations,
/// global memory, and bandwidth (`T-096`, `PERF-005`, `OPS-004`, `NET-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCapacityConfig {
    /// Maximum concurrent active allocations permitted on this relay (`PERF-005`).
    pub max_allocations: usize,
    /// Maximum queued bytes permitted globally across all sessions on this relay (`NET-006`, `OPS-004`).
    pub global_max_queued_bytes: usize,
    /// Maximum sustained bandwidth per session in bytes/sec (`OPS-004`).
    pub session_bandwidth_bytes_per_sec: u64,
    /// Token bucket burst capacity in bytes per session.
    pub session_burst_bytes: u64,
}

impl RelayCapacityConfig {
    /// Construct a verified capacity configuration, clamping `max_allocations` to [`MAX_ALLOCATIONS_CEILING`].
    pub fn new(
        max_allocations: usize,
        global_max_queued_bytes: usize,
        session_bandwidth_bytes_per_sec: u64,
        session_burst_bytes: u64,
    ) -> Self {
        Self {
            max_allocations: max_allocations.clamp(1, MAX_ALLOCATIONS_CEILING),
            global_max_queued_bytes: global_max_queued_bytes.max(1024),
            session_bandwidth_bytes_per_sec: session_bandwidth_bytes_per_sec.max(1024),
            session_burst_bytes: session_burst_bytes.max(1024),
        }
    }
}

impl Default for RelayCapacityConfig {
    fn default() -> Self {
        Self {
            max_allocations: DEFAULT_MAX_ALLOCATIONS,
            global_max_queued_bytes: DEFAULT_GLOBAL_MAX_QUEUED_BYTES,
            session_bandwidth_bytes_per_sec: DEFAULT_SESSION_BANDWIDTH_BYTES_PER_SEC,
            session_burst_bytes: DEFAULT_MAX_QUEUE_BYTES as u64,
        }
    }
}

/// Reason a frame was dropped or refused by [`BoundedSessionQueue::try_enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackpressureDropReason {
    /// Per-session frame count limit (`max_queue_frames`) reached.
    SessionQueueFramesFull,
    /// Per-session byte limit (`max_queue_bytes`) reached.
    SessionQueueBytesFull,
    /// Relay-wide global memory cap (`global_max_queued_bytes`) reached (`NET-006`).
    GlobalMemoryCapReached,
    /// Per-session token-bucket bandwidth limit exceeded (`OPS-004`).
    BandwidthLimitExceeded,
}

impl std::fmt::Display for BackpressureDropReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionQueueFramesFull => write!(f, "session queue frame capacity full"),
            Self::SessionQueueBytesFull => write!(f, "session queue byte capacity full"),
            Self::GlobalMemoryCapReached => write!(f, "global relay memory cap reached"),
            Self::BandwidthLimitExceeded => write!(f, "bandwidth rate limit exceeded"),
        }
    }
}

impl std::error::Error for BackpressureDropReason {}

/// Metrics exported by [`BoundedSessionQueue`] (`NET-006`, `OPS-004`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayLimitMetrics {
    /// Current frames in this session's queue.
    pub queued_frames: usize,
    /// Current bytes in this session's queue.
    pub queued_bytes: usize,
    /// High-water mark of `queued_bytes` observed for this session.
    pub peak_session_queued_bytes: usize,
    /// Total frames enqueued and accepted.
    pub frames_enqueued: u64,
    /// Total bytes enqueued and accepted.
    pub bytes_enqueued: u64,
    /// Total frames dropped across all limit/backpressure reasons.
    pub frames_dropped: u64,
    /// Subset of `frames_dropped`: `max_queue_frames` reached.
    pub dropped_queue_frames_full: u64,
    /// Subset of `frames_dropped`: `max_queue_bytes` reached.
    pub dropped_queue_bytes_full: u64,
    /// Subset of `frames_dropped`: `global_max_queued_bytes` reached.
    pub dropped_global_memory_cap: u64,
    /// Subset of `frames_dropped`: token-bucket bandwidth limit exceeded.
    pub dropped_bandwidth_exceeded: u64,
}

/// Integer token-bucket rate limiter (`OPS-004`, `NET-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBucketLimiter {
    capacity_bytes: u64,
    refill_bytes_per_sec: u64,
    available_bytes: u64,
    last_refill_ms: u64,
}

impl TokenBucketLimiter {
    /// Construct a token bucket with burst `capacity_bytes` and `refill_bytes_per_sec` rate.
    pub fn new(capacity_bytes: u64, refill_bytes_per_sec: u64, now_ms: u64) -> Self {
        let cap = capacity_bytes.max(1);
        Self {
            capacity_bytes: cap,
            refill_bytes_per_sec: refill_bytes_per_sec.max(1),
            available_bytes: cap,
            last_refill_ms: now_ms,
        }
    }

    /// Available bytes in the bucket without refilling.
    pub fn available_bytes(&self) -> u64 {
        self.available_bytes
    }

    /// Refill tokens up to `now_ms` and attempt to consume `bytes`.
    pub fn try_consume(&mut self, bytes: usize, now_ms: u64) -> bool {
        self.refill(now_ms);
        let needed = u64::try_from(bytes).unwrap_or(u64::MAX);
        if self.available_bytes < needed {
            return false;
        }
        self.available_bytes = self.available_bytes.saturating_sub(needed);
        true
    }

    fn refill(&mut self, now_ms: u64) {
        if now_ms <= self.last_refill_ms {
            return;
        }
        let elapsed_ms = now_ms.saturating_sub(self.last_refill_ms);
        let added = elapsed_ms
            .saturating_mul(self.refill_bytes_per_sec)
            .checked_div(1_000)
            .unwrap_or(self.capacity_bytes);
        if added > 0 {
            self.available_bytes = self
                .available_bytes
                .saturating_add(added)
                .min(self.capacity_bytes);
            self.last_refill_ms = now_ms;
        }
    }
}

/// Shared relay-wide memory accounting budget (`NET-006`, `OPS-004`).
#[derive(Debug, Clone)]
pub struct GlobalRelayMemoryBudget {
    max_bytes: usize,
    current_bytes: Rc<Cell<usize>>,
    peak_bytes: Rc<Cell<usize>>,
}

impl GlobalRelayMemoryBudget {
    /// Create a new relay-wide memory budget capped at `max_bytes`.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.max(1),
            current_bytes: Rc::new(Cell::new(0)),
            peak_bytes: Rc::new(Cell::new(0)),
        }
    }

    /// Configured global byte ceiling.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Currently allocated queue bytes across all sessions sharing this budget.
    pub fn current_bytes(&self) -> usize {
        self.current_bytes.get()
    }

    /// Peak concurrent queue bytes observed across all sessions sharing this budget.
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes.get()
    }

    fn try_reserve(&self, bytes: usize) -> bool {
        let cur = self.current_bytes.get();
        let Some(next) = cur.checked_add(bytes) else {
            return false;
        };
        if next > self.max_bytes {
            return false;
        }
        self.current_bytes.set(next);
        if next > self.peak_bytes.get() {
            self.peak_bytes.set(next);
        }
        true
    }

    fn release(&self, bytes: usize) {
        let cur = self.current_bytes.get();
        self.current_bytes.set(cur.saturating_sub(bytes));
    }
}

/// Per-session bounded send queue enforcing frame count, session byte cap, global relay memory cap,
/// and token-bucket bandwidth limit (`T-064`, `NET-006`, `OPS-004`).
#[derive(Debug)]
pub struct BoundedSessionQueue {
    max_queue_frames: usize,
    max_queue_bytes: usize,
    global_budget: GlobalRelayMemoryBudget,
    bandwidth_limiter: TokenBucketLimiter,
    queue: VecDeque<Vec<u8>>,
    metrics: RelayLimitMetrics,
}

impl BoundedSessionQueue {
    /// Construct a bounded session queue attached to `global_budget`.
    pub fn new(
        max_queue_frames: usize,
        max_queue_bytes: usize,
        global_budget: GlobalRelayMemoryBudget,
        bandwidth_limiter: TokenBucketLimiter,
    ) -> Self {
        Self {
            max_queue_frames: max_queue_frames.max(1),
            max_queue_bytes: max_queue_bytes.max(1),
            global_budget,
            bandwidth_limiter,
            queue: VecDeque::new(),
            metrics: RelayLimitMetrics::default(),
        }
    }

    /// Borrow current queue and drop metrics.
    pub fn metrics(&self) -> &RelayLimitMetrics {
        &self.metrics
    }

    /// Number of frames currently buffered.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True if no frames are buffered.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Attempt to enqueue `frame` at `now_ms`.
    ///
    /// Checks all capacity and bandwidth bounds **before** allocating or copying `frame` into the
    /// queue (`NET-006`, `AGENTS.md` §4).
    pub fn try_enqueue(&mut self, frame: &[u8], now_ms: u64) -> Result<(), BackpressureDropReason> {
        let frame_len = frame.len();

        // 1. Per-session frame count bound.
        if self.queue.len() >= self.max_queue_frames {
            self.record_drop(BackpressureDropReason::SessionQueueFramesFull);
            return Err(BackpressureDropReason::SessionQueueFramesFull);
        }

        // 2. Per-session byte bound.
        let Some(next_session_bytes) = self.metrics.queued_bytes.checked_add(frame_len) else {
            self.record_drop(BackpressureDropReason::SessionQueueBytesFull);
            return Err(BackpressureDropReason::SessionQueueBytesFull);
        };
        if next_session_bytes > self.max_queue_bytes {
            self.record_drop(BackpressureDropReason::SessionQueueBytesFull);
            return Err(BackpressureDropReason::SessionQueueBytesFull);
        }

        // 3. Global relay memory cap (`NET-006`, `OPS-004`).
        if !self.global_budget.try_reserve(frame_len) {
            self.record_drop(BackpressureDropReason::GlobalMemoryCapReached);
            return Err(BackpressureDropReason::GlobalMemoryCapReached);
        }

        // 4. Token-bucket bandwidth limit (`OPS-004`).
        if !self.bandwidth_limiter.try_consume(frame_len, now_ms) {
            self.global_budget.release(frame_len);
            self.record_drop(BackpressureDropReason::BandwidthLimitExceeded);
            return Err(BackpressureDropReason::BandwidthLimitExceeded);
        }

        self.queue.push_back(frame.to_vec());
        self.metrics.queued_frames = self.queue.len();
        self.metrics.queued_bytes = next_session_bytes;
        if next_session_bytes > self.metrics.peak_session_queued_bytes {
            self.metrics.peak_session_queued_bytes = next_session_bytes;
        }
        self.metrics.frames_enqueued = self.metrics.frames_enqueued.saturating_add(1);
        self.metrics.bytes_enqueued = self
            .metrics
            .bytes_enqueued
            .saturating_add(u64::try_from(frame_len).unwrap_or(0));
        Ok(())
    }

    /// Pop the next frame from the queue and release its byte reservation in both the session and
    /// the global relay memory budget.
    pub fn pop_front(&mut self) -> Option<Vec<u8>> {
        let frame = self.queue.pop_front()?;
        let len = frame.len();
        self.metrics.queued_frames = self.queue.len();
        self.metrics.queued_bytes = self.metrics.queued_bytes.saturating_sub(len);
        self.global_budget.release(len);
        Some(frame)
    }

    /// Clear all queued frames and release their global memory reservation (`NET-008`).
    pub fn clear(&mut self) {
        let released = self.metrics.queued_bytes;
        self.queue.clear();
        self.metrics.queued_frames = 0;
        self.metrics.queued_bytes = 0;
        self.global_budget.release(released);
    }

    fn record_drop(&mut self, reason: BackpressureDropReason) {
        self.metrics.frames_dropped = self.metrics.frames_dropped.saturating_add(1);
        match reason {
            BackpressureDropReason::SessionQueueFramesFull => {
                self.metrics.dropped_queue_frames_full =
                    self.metrics.dropped_queue_frames_full.saturating_add(1);
            }
            BackpressureDropReason::SessionQueueBytesFull => {
                self.metrics.dropped_queue_bytes_full =
                    self.metrics.dropped_queue_bytes_full.saturating_add(1);
            }
            BackpressureDropReason::GlobalMemoryCapReached => {
                self.metrics.dropped_global_memory_cap =
                    self.metrics.dropped_global_memory_cap.saturating_add(1);
            }
            BackpressureDropReason::BandwidthLimitExceeded => {
                self.metrics.dropped_bandwidth_exceeded =
                    self.metrics.dropped_bandwidth_exceeded.saturating_add(1);
            }
        }
    }
}

impl Drop for BoundedSessionQueue {
    fn drop(&mut self) {
        self.clear();
    }
}
