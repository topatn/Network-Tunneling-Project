//! Typed control-plane HTTP API client with bounded timeouts, retry/backoff, and one-shot
//! token refresh (`contracts.md` §§3, 4, 5, 6, 8, 14 and `REL-003`, T-047).
//!
//! ## Contract Guarantees (`contracts.md` §4, `REL-003`, `SEC-010`, `PRIV-002`)
//!
//! 1. **HTTPS only (`SEC-010`):** [`ClientConfig::validate`] refuses any `base_url` that does
//!    not start with `"https://"`. TLS certificate validation failures ([`TransportError::TlsCertificateInvalid`])
//!    abort immediately and are never retried.
//! 2. **Short bounded timeouts (`contracts.md` §4, `AGENTS.md` §4):** Every request carries explicit
//!    non-zero `connect_timeout` (`<= 30s`) and `request_timeout` (`<= 60s`), plus a pre-allocation
//!    `max_response_body_bytes` cap (`<= 1 MiB`).
//! 3. **Retry rule (`contracts.md` §4):** `GET`, `DELETE`, and idempotent `POST` (carrying a
//!    validated [`IdempotencyKey`] of 8–128 visible ASCII characters) retry on transport timeouts
//!    and network errors, HTTP `429` (`rate_limited`), and HTTP `502`/`503`/`504`, using
//!    exponential backoff with bounded jitter and honoring the server's `Retry-After` header
//!    (clamped to `max_retry_after`), for **at most 3 attempts** ([`MAX_ATTEMPTS`]).
//! 4. **No-retry rule (`contracts.md` §4):** `POST` without an [`IdempotencyKey`], any `4xx`
//!    other than `429` (`400`, `403`, `404`, `409`, `422`, `426`), and HTTP `500` **never retry**
//!    and stop after the first attempt.
//! 5. **Token refresh on `401` (`contracts.md` §4):** On HTTP `401` (`unauthenticated`) from an
//!    authenticated route, the client invokes [`TokenRefresher::refresh_access_token`] **at most
//!    once** per call. If refresh succeeds, the request is re-sent once with the new bearer token;
//!    if refresh fails or the re-sent request still returns `401`, the client stops and returns
//!    [`ApiClientError::ReauthenticationRequired`] so the caller returns the user to sign-in.
//! 6. **Forward compatibility (`contracts.md` §14):** Unknown fields on server responses are
//!    ignored while `INV-1` (no local-destination fields) and domain invariants (`INV-7`) remain
//!    strictly enforced; unknown error code strings map to [`ApiErrorCode::Internal`].
//! 7. **Secret redaction (`PRIV-002`, `AGENTS.md` §7):** `Debug` implementations on [`HttpRequest`]
//!    and [`ControlPlaneClient`] redact bearer tokens so credentials never appear in logs.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use gtt_protocol::{
    is_valid_rfc3339_utc, is_valid_uuid, scan_json_for_local_target, AllocatedPortDto,
    AllocationDto, ApiErrorCode, ClaimInvitationRequest, ClaimInvitationResponse, ClaimRelayDto,
    CreateInvitationRequest, CreateInvitationResponse, CreateTunnelRequest, ErrorBody, ErrorDetail,
    ErrorResponse, Protocol, RelayEndpointDto, StartTunnelResponse, Tunnel, TunnelMappingDto,
    TunnelState, ValidationError,
};

/// Hard attempt ceiling per `contracts.md` §4 (*"at most 3 attempts"*).
pub const MAX_ATTEMPTS: u8 = 3;

/// Minimum `Idempotency-Key` length (`contracts.md` §8: 8–128 characters).
pub const MIN_IDEMPOTENCY_KEY_LEN: usize = 8;

/// Maximum `Idempotency-Key` length (`contracts.md` §8: 8–128 characters).
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

/// Default connect timeout (`5s`, *(proposal)*).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default per-request timeout (`10s`, *(proposal)*).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper ceiling allowed for `connect_timeout` (`30s`).
pub const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper ceiling allowed for `request_timeout` (`60s`).
pub const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Default initial exponential backoff delay (`200ms`, *(proposal)*).
pub const DEFAULT_BASE_BACKOFF: Duration = Duration::from_millis(200);

/// Default maximum exponential backoff delay (`5s`, *(proposal)*).
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Upper ceiling allowed for `max_backoff` (`30s`).
pub const MAX_BACKOFF_CEILING: Duration = Duration::from_secs(30);

/// Default cap on server-supplied `Retry-After` headers (`30s`).
pub const DEFAULT_MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Upper ceiling allowed for `max_retry_after` (`60s`).
pub const MAX_RETRY_AFTER_CEILING: Duration = Duration::from_secs(60);

/// Default response body cap (`256 KiB`).
pub const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 256 * 1024;

/// Maximum configurable response body cap (`1 MiB`).
pub const MAX_RESPONSE_BODY_CEILING: usize = 1024 * 1024;

/// Maximum length of `base_url` in characters.
pub const MAX_BASE_URL_LEN: usize = 512;

/// Validated `Idempotency-Key` header value (`contracts.md` §8: 8–128 visible ASCII chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validate and construct an [`IdempotencyKey`].
    pub fn new(raw: impl Into<String>) -> Result<Self, ApiClientError> {
        let value = raw.into();
        let len = value.len();
        if !(MIN_IDEMPOTENCY_KEY_LEN..=MAX_IDEMPOTENCY_KEY_LEN).contains(&len) {
            return Err(ApiClientError::InvalidInput {
                field: "Idempotency-Key",
                issue: "must be 8 to 128 ASCII characters",
            });
        }
        if !value.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
            return Err(ApiClientError::InvalidInput {
                field: "Idempotency-Key",
                issue: "must contain only visible ASCII characters (0x21..=0x7E)",
            });
        }
        Ok(Self(value))
    }

    /// Borrow the validated header value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Configuration for [`ControlPlaneClient`] (`contracts.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// HTTPS base URL of the control plane (e.g. `"https://api.example.invalid"`).
    pub base_url: String,
    /// Per-attempt TCP/TLS connect timeout (`1ms..=30s`).
    pub connect_timeout: Duration,
    /// Per-attempt total request timeout (`connect_timeout..=60s`).
    pub request_timeout: Duration,
    /// Base delay for exponential backoff (`1ms..=30s`).
    pub base_backoff: Duration,
    /// Maximum exponential backoff delay (`base_backoff..=30s`).
    pub max_backoff: Duration,
    /// Upper bound applied to server `Retry-After` headers (`1ms..=60s`).
    pub max_retry_after: Duration,
    /// Maximum response payload size accepted before allocation (`1024..=1_048_576`).
    pub max_response_body_bytes: usize,
}

impl ClientConfig {
    /// Construct a [`ClientConfig`] with `contracts.md` §4 default proposal bounds.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            base_backoff: DEFAULT_BASE_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            max_retry_after: DEFAULT_MAX_RETRY_AFTER,
            max_response_body_bytes: DEFAULT_MAX_RESPONSE_BODY_BYTES,
        }
    }

    /// Validate all timeout, backoff, size, and `SEC-010` HTTPS constraints.
    pub fn validate(&self) -> Result<(), ApiClientError> {
        let trimmed = self.base_url.trim_end_matches('/');
        if !trimmed.starts_with("https://") {
            return Err(ApiClientError::InvalidConfig(
                "base_url must begin with https:// (SEC-010)",
            ));
        }
        if trimmed.len() <= "https://".len() || trimmed.len() > MAX_BASE_URL_LEN {
            return Err(ApiClientError::InvalidConfig(
                "base_url must include a host and be at most 512 characters",
            ));
        }
        if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(ApiClientError::InvalidConfig(
                "base_url must not contain whitespace or control characters",
            ));
        }
        if self.connect_timeout.is_zero() || self.connect_timeout > MAX_CONNECT_TIMEOUT {
            return Err(ApiClientError::InvalidConfig(
                "connect_timeout must be non-zero and at most 30 seconds",
            ));
        }
        if self.request_timeout < self.connect_timeout || self.request_timeout > MAX_REQUEST_TIMEOUT
        {
            return Err(ApiClientError::InvalidConfig(
                "request_timeout must be >= connect_timeout and at most 60 seconds",
            ));
        }
        if self.base_backoff.is_zero() || self.base_backoff > MAX_BACKOFF_CEILING {
            return Err(ApiClientError::InvalidConfig(
                "base_backoff must be non-zero and at most 30 seconds",
            ));
        }
        if self.max_backoff < self.base_backoff || self.max_backoff > MAX_BACKOFF_CEILING {
            return Err(ApiClientError::InvalidConfig(
                "max_backoff must be >= base_backoff and at most 30 seconds",
            ));
        }
        if self.max_retry_after.is_zero() || self.max_retry_after > MAX_RETRY_AFTER_CEILING {
            return Err(ApiClientError::InvalidConfig(
                "max_retry_after must be non-zero and at most 60 seconds",
            ));
        }
        if !(1024..=MAX_RESPONSE_BODY_CEILING).contains(&self.max_response_body_bytes) {
            return Err(ApiClientError::InvalidConfig(
                "max_response_body_bytes must be between 1024 and 1048576 bytes",
            ));
        }
        Ok(())
    }
}

/// HTTP verb used by the control-plane API (`api.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// `GET` request.
    Get,
    /// `POST` request.
    Post,
    /// `DELETE` request.
    Delete,
}

impl HttpMethod {
    /// Wire method string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

/// Outbound HTTP request passed to [`HttpTransport::send`].
///
/// Custom `Debug` implementation redacts the `Authorization` header (`PRIV-002`).
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// HTTP verb.
    pub method: HttpMethod,
    /// Full request URL (`https://...`).
    pub url: String,
    /// Request headers (`(name, value)`).
    pub headers: Vec<(String, String)>,
    /// Optional serialized JSON body.
    pub body: Option<Vec<u8>>,
    /// Per-attempt connect timeout.
    pub connect_timeout: Duration,
    /// Per-attempt total request timeout.
    pub request_timeout: Duration,
    /// Upper bound on response body bytes.
    pub max_response_body_bytes: usize,
}

impl HttpRequest {
    /// Look up a header value case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                if k.eq_ignore_ascii_case("authorization") {
                    (k.as_str(), "<redacted>")
                } else {
                    (k.as_str(), v.as_str())
                }
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &redacted_headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("max_response_body_bytes", &self.max_response_body_bytes)
            .finish()
    }
}

/// Raw HTTP response returned by [`HttpTransport::send`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code (`100..=599`).
    pub status: u16,
    /// Response headers (`(name, value)`).
    pub headers: Vec<(String, String)>,
    /// Raw response body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Look up a response header case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Transport-layer failure returned by [`HttpTransport::send`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// Connection establishment timed out (`connect_timeout`).
    ConnectTimeout,
    /// Request or response read timed out (`request_timeout`).
    RequestTimeout,
    /// Transient network or socket I/O error.
    Network(String),
    /// TLS certificate or hostname verification failed (`SEC-010`: never retried).
    TlsCertificateInvalid(String),
    /// Response payload exceeded `max_response_body_bytes` (`AGENTS.md` §4: never retried).
    ResponseTooLarge {
        /// Configured byte limit.
        limit: usize,
    },
}

impl TransportError {
    /// True if this transport failure is eligible for backoff retry on idempotent requests
    /// (`contracts.md` §4, `SEC-010`).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ConnectTimeout | Self::RequestTimeout | Self::Network(_) => true,
            Self::TlsCertificateInvalid(_) | Self::ResponseTooLarge { .. } => false,
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectTimeout => f.write_str("connection timed out"),
            Self::RequestTimeout => f.write_str("request timed out"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::TlsCertificateInvalid(msg) => {
                write!(f, "TLS certificate validation failed: {msg}")
            }
            Self::ResponseTooLarge { limit } => {
                write!(f, "response body exceeded maximum of {limit} bytes")
            }
        }
    }
}

impl std::error::Error for TransportError {}

/// Pluggable HTTP transport seam enabling deterministic fault-injection unit tests (T-047)
/// without contacting external services (`AGENTS.md` §6.4).
pub trait HttpTransport {
    /// Execute a single HTTP request attempt.
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// Pluggable backoff sleeper and jitter source so retry delays can be recorded and asserted
/// deterministically in unit tests without wall-clock sleeps.
pub trait BackoffPolicy {
    /// Return a jitter factor in basis points (`0..=10_000`) for `retry_number` (`1` or `2`).
    fn jitter_bps(&mut self, retry_number: u8) -> u16;

    /// Record or sleep for `delay` before retry `retry_number` (`1` or `2`).
    fn sleep(&mut self, retry_number: u8, delay: Duration);
}

/// Default deterministic equal-jitter policy (50% + 25% = 75% of exponential window, no-op sleep
/// hook for synchronous unit tests or `std::thread::sleep` in production).
#[derive(Debug, Clone, Default)]
pub struct DeterministicBackoff {
    /// Recorded sleep delays in order.
    pub recorded_delays: Vec<Duration>,
    /// Fixed jitter basis points (`0..=10_000`). Defaults to `5_000` (midpoint jitter).
    pub fixed_jitter_bps: u16,
}

impl DeterministicBackoff {
    /// Create a backoff policy with the given jitter basis points (`0..=10_000`).
    pub fn with_jitter_bps(fixed_jitter_bps: u16) -> Self {
        Self {
            recorded_delays: Vec::new(),
            fixed_jitter_bps: fixed_jitter_bps.min(10_000),
        }
    }
}

impl BackoffPolicy for DeterministicBackoff {
    fn jitter_bps(&mut self, _retry_number: u8) -> u16 {
        self.fixed_jitter_bps.min(10_000)
    }

    fn sleep(&mut self, _retry_number: u8, delay: Duration) {
        self.recorded_delays.push(delay);
    }
}

/// Error returned when a one-shot token refresh (`POST /v1/auth/refresh`) fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRefreshError {
    /// Reason the refresh failed (never contains token strings, `PRIV-002`).
    pub reason: String,
}

impl TokenRefreshError {
    /// Create a [`TokenRefreshError`].
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for TokenRefreshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "token refresh failed: {}", self.reason)
    }
}

impl std::error::Error for TokenRefreshError {}

/// Pluggable one-shot token refresher (`contracts.md` §4: *"On 401 once; if refresh fails,
/// return the user to sign-in"*).
pub trait TokenRefresher {
    /// Exchange the current refresh token for a fresh access token.
    fn refresh_access_token(&mut self) -> Result<String, TokenRefreshError>;
}

/// No-op refresher used when no refresh callback is configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTokenRefresher;

impl TokenRefresher for NoTokenRefresher {
    fn refresh_access_token(&mut self) -> Result<String, TokenRefreshError> {
        Err(TokenRefreshError::new("no token refresher configured"))
    }
}

/// Response body for `POST /v1/tunnels/{id}/token` (`contracts.md` §5.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocationTokenResponse {
    /// Fresh signed allocation token.
    pub allocation_token: String,
    /// Expiry timestamp (RFC 3339 UTC).
    pub expires_at: String,
}

impl AllocationTokenResponse {
    /// Validate `contracts.md` §5.9 field bounds.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.allocation_token.trim().is_empty() || self.allocation_token.len() > 4096 {
            return Err(ValidationError::InvalidField {
                field: "allocation_token",
                issue: "must be 1-4096 characters",
            });
        }
        if !is_valid_rfc3339_utc(&self.expires_at) {
            return Err(ValidationError::InvalidField {
                field: "expires_at",
                issue: "must be a valid RFC 3339 UTC timestamp",
            });
        }
        Ok(())
    }
}

/// Error returned by [`ControlPlaneClient`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiClientError {
    /// Client configuration violated timeout, backoff, or HTTPS rules.
    InvalidConfig(&'static str),
    /// Outgoing request parameter failed local validation before network send.
    InvalidInput {
        /// Field or parameter name.
        field: &'static str,
        /// Description of the validation failure.
        issue: &'static str,
    },
    /// Request failed with `401 unauthenticated` and the single token-refresh attempt either
    /// failed or the re-issued request still returned `401` (`contracts.md` §4: caller must
    /// return the user to sign-in).
    ReauthenticationRequired {
        /// Diagnostic reason (no secrets).
        reason: String,
        /// Number of HTTP attempts executed before stopping.
        attempts: u8,
    },
    /// Server returned a structured API error (`contracts.md` §3).
    ServerError {
        /// HTTP status code.
        status: u16,
        /// Parsed error envelope (unknown codes mapped to [`ApiErrorCode::Internal`] per §14).
        response: ErrorResponse,
        /// Total attempts executed before returning.
        attempts: u8,
    },
    /// Transport failure after exhausting allowed attempts.
    Transport {
        /// Last transport error encountered.
        source: TransportError,
        /// Total attempts executed before returning.
        attempts: u8,
    },
    /// Server returned a malformed or contract-violating payload (`INV-1`, `INV-7`, or invalid JSON).
    InvalidResponse {
        /// Description of the response defect.
        reason: String,
        /// Total attempts executed.
        attempts: u8,
    },
}

impl fmt::Display for ApiClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "invalid client configuration: {msg}"),
            Self::InvalidInput { field, issue } => {
                write!(f, "invalid input for {field}: {issue}")
            }
            Self::ReauthenticationRequired { reason, attempts } => {
                write!(
                    f,
                    "reauthentication required after {attempts} attempt(s): {reason}"
                )
            }
            Self::ServerError {
                status,
                response,
                attempts,
            } => write!(
                f,
                "server error {} (HTTP {status}) after {attempts} attempt(s): {}",
                response.error.code.as_str(),
                response.error.message
            ),
            Self::Transport { source, attempts } => {
                write!(f, "transport error after {attempts} attempt(s): {source}")
            }
            Self::InvalidResponse { reason, attempts } => {
                write!(
                    f,
                    "invalid server response after {attempts} attempt(s): {reason}"
                )
            }
        }
    }
}

impl std::error::Error for ApiClientError {}

/// Determine whether a request is eligible for automatic retry under `contracts.md` §4:
///
/// * `GET` and `DELETE` are idempotent and retryable.
/// * `POST` is retryable **if and only if** an `Idempotency-Key` header is present.
pub fn is_request_retryable(method: HttpMethod, has_idempotency_key: bool) -> bool {
    match method {
        HttpMethod::Get | HttpMethod::Delete => true,
        HttpMethod::Post => has_idempotency_key,
    }
}

/// Determine whether an HTTP status code triggers backoff retry on an idempotent request
/// (`contracts.md` §4: *"retry on network errors, 429, 502/503/504"*).
pub fn is_status_retryable(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Parse an HTTP `Retry-After` header value (delta-seconds per RFC 9110 §10.2.3) and clamp it to
/// `max_retry_after` (`AGENTS.md` §4).
pub fn parse_retry_after(header_value: &str, max_retry_after: Duration) -> Option<Duration> {
    let trimmed = header_value.trim();
    if trimmed.is_empty() || trimmed.len() > 20 || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds = trimmed.parse::<u64>().ok()?;
    let raw = Duration::from_secs(seconds);
    Some(raw.min(max_retry_after))
}

/// Compute the retry delay for `retry_number` (`1` after attempt 1, `2` after attempt 2) using
/// exponential backoff + equal jitter in `[exp / 2, exp]`, honoring `Retry-After` when present
/// (`contracts.md` §4).
pub fn compute_retry_delay(
    config: &ClientConfig,
    retry_number: u8,
    jitter_bps: u16,
    retry_after_header: Option<&str>,
) -> Duration {
    let shift = u32::from(retry_number.saturating_sub(1)).min(16);
    let factor = 1u32.checked_shl(shift).unwrap_or(u32::MAX);
    let exp = config
        .base_backoff
        .saturating_mul(factor)
        .min(config.max_backoff);

    let exp_nanos = exp.as_nanos();
    let half_nanos = exp_nanos / 2;
    let clamped_bps = u128::from(jitter_bps.min(10_000));
    let jitter_nanos = ((exp_nanos - half_nanos) * clamped_bps) / 10_000;
    let total_nanos = half_nanos + jitter_nanos;
    let computed = Duration::from_nanos(u64::try_from(total_nanos).unwrap_or(u64::MAX));

    if let Some(raw_header) = retry_after_header {
        if let Some(server_delay) = parse_retry_after(raw_header, config.max_retry_after) {
            return computed.max(server_delay);
        }
    }
    computed
}

/// Typed control-plane HTTP API client (`T-047`, `contracts.md` §§3, 4, 5, 6, 8, 14).
pub struct ControlPlaneClient<T, B = DeterministicBackoff, R = NoTokenRefresher> {
    config: ClientConfig,
    access_token: Option<String>,
    transport: T,
    backoff: B,
    refresher: R,
}

impl<T: fmt::Debug, B: fmt::Debug, R: fmt::Debug> fmt::Debug for ControlPlaneClient<T, B, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlPlaneClient")
            .field("config", &self.config)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "<redacted>"),
            )
            .field("transport", &self.transport)
            .field("backoff", &self.backoff)
            .field("refresher", &self.refresher)
            .finish()
    }
}

impl<T, B, R> ControlPlaneClient<T, B, R>
where
    T: HttpTransport,
    B: BackoffPolicy,
    R: TokenRefresher,
{
    /// Construct a validated [`ControlPlaneClient`].
    pub fn new(
        config: ClientConfig,
        access_token: Option<String>,
        transport: T,
        backoff: B,
        refresher: R,
    ) -> Result<Self, ApiClientError> {
        config.validate()?;
        if let Some(token) = &access_token {
            validate_access_token_format(token)?;
        }
        Ok(Self {
            config,
            access_token,
            transport,
            backoff,
            refresher,
        })
    }

    /// Borrow the client configuration.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Borrow the underlying transport (useful for inspecting recorded requests in tests).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Borrow the backoff policy (useful for inspecting recorded delays in tests).
    pub fn backoff(&self) -> &B {
        &self.backoff
    }

    /// Borrow the token refresher.
    pub fn refresher(&self) -> &R {
        &self.refresher
    }

    /// Route 6: `POST /v1/tunnels` (`contracts.md` §5.6).
    ///
    /// Idempotent via `Idempotency-Key` (`contracts.md` §8); retries on network/timeout errors,
    /// `429`, and `502`/`503`/`504` up to [`MAX_ATTEMPTS`].
    pub fn create_tunnel(
        &mut self,
        request: &CreateTunnelRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Tunnel, ApiClientError> {
        request
            .validate()
            .map_err(|e| map_validation_error("create_tunnel", &e))?;
        let body = serialize_request_body(request)?;
        let resp = self.execute(
            HttpMethod::Post,
            "/v1/tunnels",
            true,
            Some(idempotency_key),
            Some(body),
        )?;
        parse_tunnel_response(&resp.body, resp.attempts)
    }

    /// Route 7: `GET /v1/tunnels` (`contracts.md` §5.7).
    pub fn list_tunnels(&mut self) -> Result<Vec<Tunnel>, ApiClientError> {
        let resp = self.execute(HttpMethod::Get, "/v1/tunnels", true, None, None)?;
        let value = parse_json_checked(&resp.body, resp.attempts)?;
        let items = value
            .as_array()
            .ok_or_else(|| ApiClientError::InvalidResponse {
                reason: "expected JSON array of tunnels".to_owned(),
                attempts: resp.attempts,
            })?;
        let mut tunnels = Vec::with_capacity(items.len());
        for item in items {
            tunnels.push(parse_tunnel_value(item, resp.attempts)?);
        }
        Ok(tunnels)
    }

    /// Route 8: `GET /v1/tunnels/{id}` (`contracts.md` §5.7).
    pub fn get_tunnel(&mut self, tunnel_id: &str) -> Result<Tunnel, ApiClientError> {
        validate_uuid_param("tunnel_id", tunnel_id)?;
        let path = format!("/v1/tunnels/{tunnel_id}");
        let resp = self.execute(HttpMethod::Get, &path, true, None, None)?;
        parse_tunnel_response(&resp.body, resp.attempts)
    }

    /// Route 9: `POST /v1/tunnels/{id}/start` (`contracts.md` §5.8).
    ///
    /// Idempotent via `Idempotency-Key` (`contracts.md` §8).
    pub fn start_tunnel(
        &mut self,
        tunnel_id: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Result<StartTunnelResponse, ApiClientError> {
        validate_uuid_param("tunnel_id", tunnel_id)?;
        let path = format!("/v1/tunnels/{tunnel_id}/start");
        let resp = self.execute(HttpMethod::Post, &path, true, Some(idempotency_key), None)?;
        parse_start_tunnel_response(&resp.body, resp.attempts)
    }

    /// Route 10: `POST /v1/tunnels/{id}/token` (`contracts.md` §5.9).
    ///
    /// Non-idempotent `POST` without `Idempotency-Key`: **never retries** on network/429/5xx
    /// (`contracts.md` §4).
    pub fn refresh_allocation_token(
        &mut self,
        tunnel_id: &str,
    ) -> Result<AllocationTokenResponse, ApiClientError> {
        validate_uuid_param("tunnel_id", tunnel_id)?;
        let path = format!("/v1/tunnels/{tunnel_id}/token");
        let resp = self.execute(HttpMethod::Post, &path, true, None, None)?;
        parse_allocation_token_response(&resp.body, resp.attempts)
    }

    /// Route 11: `POST /v1/tunnels/{id}/stop` (`contracts.md` §5.10).
    ///
    /// Accepts an optional [`IdempotencyKey`]; per `contracts.md` §4, `POST` without an
    /// `Idempotency-Key` does not retry on transport/5xx errors.
    pub fn stop_tunnel(
        &mut self,
        tunnel_id: &str,
        idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<Tunnel, ApiClientError> {
        validate_uuid_param("tunnel_id", tunnel_id)?;
        let path = format!("/v1/tunnels/{tunnel_id}/stop");
        let resp = self.execute(HttpMethod::Post, &path, true, idempotency_key, None)?;
        parse_tunnel_response(&resp.body, resp.attempts)
    }

    /// Route 12: `POST /v1/tunnels/{id}/invitations` (`contracts.md` §5.11).
    ///
    /// Idempotent via `Idempotency-Key` (`contracts.md` §8).
    pub fn create_invitation(
        &mut self,
        tunnel_id: &str,
        request: &CreateInvitationRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateInvitationResponse, ApiClientError> {
        validate_uuid_param("tunnel_id", tunnel_id)?;
        request
            .validate()
            .map_err(|e| map_validation_error("create_invitation", &e))?;
        let path = format!("/v1/tunnels/{tunnel_id}/invitations");
        let body = serialize_request_body(request)?;
        let resp = self.execute(
            HttpMethod::Post,
            &path,
            true,
            Some(idempotency_key),
            Some(body),
        )?;
        parse_create_invitation_response(&resp.body, resp.attempts)
    }

    /// Route 14: `DELETE /v1/invitations/{id}` (`contracts.md` §5.12).
    pub fn revoke_invitation(&mut self, invitation_id: &str) -> Result<(), ApiClientError> {
        validate_uuid_param("invitation_id", invitation_id)?;
        let path = format!("/v1/invitations/{invitation_id}");
        let _ = self.execute(HttpMethod::Delete, &path, true, None, None)?;
        Ok(())
    }

    /// Route 18: `POST /v1/invitations/claim` (`contracts.md` §6).
    ///
    /// Unauthenticated, non-idempotent `POST` without `Idempotency-Key`: **never retries** and
    /// **never triggers token refresh**.
    pub fn claim_invitation(
        &mut self,
        request: &ClaimInvitationRequest,
    ) -> Result<ClaimInvitationResponse, ApiClientError> {
        request
            .validate()
            .map_err(|e| map_validation_error("claim_invitation", &e))?;
        let body = serialize_request_body(request)?;
        let resp = self.execute(
            HttpMethod::Post,
            "/v1/invitations/claim",
            false,
            None,
            Some(body),
        )?;
        parse_claim_invitation_response(&resp.body, resp.attempts)
    }

    /// Execute an HTTP request enforcing `contracts.md` §4 timeout, retry, backoff,
    /// `Retry-After`, and one-shot `401` token-refresh rules.
    pub fn execute(
        &mut self,
        method: HttpMethod,
        path: &str,
        requires_auth: bool,
        idempotency_key: Option<&IdempotencyKey>,
        body: Option<Vec<u8>>,
    ) -> Result<ExecutedResponse, ApiClientError> {
        if !path.starts_with('/') {
            return Err(ApiClientError::InvalidInput {
                field: "path",
                issue: "must start with '/'",
            });
        }

        let retryable_request = is_request_retryable(method, idempotency_key.is_some());
        let max_attempts = if retryable_request { MAX_ATTEMPTS } else { 1 };

        let mut total_attempts: u8 = 0;
        let mut retry_attempts_in_epoch: u8 = 0;
        let mut refreshed_token_once = false;

        loop {
            total_attempts = total_attempts.saturating_add(1);
            retry_attempts_in_epoch = retry_attempts_in_epoch.saturating_add(1);

            let request = self.build_http_request(
                method,
                path,
                requires_auth,
                idempotency_key,
                body.clone(),
            )?;

            match self.transport.send(&request) {
                Ok(response) => {
                    if response.body.len() > self.config.max_response_body_bytes {
                        return Err(ApiClientError::Transport {
                            source: TransportError::ResponseTooLarge {
                                limit: self.config.max_response_body_bytes,
                            },
                            attempts: total_attempts,
                        });
                    }

                    // Success status (2xx)
                    if (200..300).contains(&response.status) {
                        return Ok(ExecutedResponse {
                            status: response.status,
                            body: response.body,
                            attempts: total_attempts,
                        });
                    }

                    // HTTP 401 on an authenticated route: one-shot token refresh rule (`contracts.md` §4).
                    if response.status == 401 && requires_auth {
                        if refreshed_token_once {
                            return Err(ApiClientError::ReauthenticationRequired {
                                reason: "request still returned 401 after token refresh".to_owned(),
                                attempts: total_attempts,
                            });
                        }
                        refreshed_token_once = true;
                        match self.refresher.refresh_access_token() {
                            Ok(new_token) => {
                                if validate_access_token_format(&new_token).is_err() {
                                    return Err(ApiClientError::ReauthenticationRequired {
                                        reason: "token refresher returned an invalid access token"
                                            .to_owned(),
                                        attempts: total_attempts,
                                    });
                                }
                                self.access_token = Some(new_token);
                                // Reset the retry epoch so the re-authenticated request runs cleanly.
                                retry_attempts_in_epoch = 0;
                                continue;
                            }
                            Err(err) => {
                                return Err(ApiClientError::ReauthenticationRequired {
                                    reason: err.reason,
                                    attempts: total_attempts,
                                });
                            }
                        }
                    }

                    // Check if status is retryable (429, 502, 503, 504) and budget remains.
                    if retryable_request
                        && is_status_retryable(response.status)
                        && retry_attempts_in_epoch < max_attempts
                    {
                        let retry_number = retry_attempts_in_epoch;
                        let jitter = self.backoff.jitter_bps(retry_number);
                        let retry_after = response.header("Retry-After");
                        let delay =
                            compute_retry_delay(&self.config, retry_number, jitter, retry_after);
                        self.backoff.sleep(retry_number, delay);
                        continue;
                    }

                    // Non-retryable HTTP status or attempt budget exhausted: parse error response.
                    let error_response =
                        parse_error_response(response.status, &response.body, total_attempts)?;
                    return Err(ApiClientError::ServerError {
                        status: response.status,
                        response: error_response,
                        attempts: total_attempts,
                    });
                }
                Err(transport_err) => {
                    if retryable_request
                        && transport_err.is_retryable()
                        && retry_attempts_in_epoch < max_attempts
                    {
                        let retry_number = retry_attempts_in_epoch;
                        let jitter = self.backoff.jitter_bps(retry_number);
                        let delay = compute_retry_delay(&self.config, retry_number, jitter, None);
                        self.backoff.sleep(retry_number, delay);
                        continue;
                    }
                    return Err(ApiClientError::Transport {
                        source: transport_err,
                        attempts: total_attempts,
                    });
                }
            }
        }
    }

    fn build_http_request(
        &self,
        method: HttpMethod,
        path: &str,
        requires_auth: bool,
        idempotency_key: Option<&IdempotencyKey>,
        body: Option<Vec<u8>>,
    ) -> Result<HttpRequest, ApiClientError> {
        let base = self.config.base_url.trim_end_matches('/');
        let url = format!("{base}{path}");
        let mut headers = Vec::with_capacity(4);
        headers.push(("Accept".to_owned(), "application/json".to_owned()));
        if body.is_some() {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        if requires_auth {
            let token = self.access_token.as_deref().ok_or_else(|| {
                ApiClientError::ReauthenticationRequired {
                    reason: "missing access token for authenticated route".to_owned(),
                    attempts: 0,
                }
            })?;
            headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
        }
        if let Some(key) = idempotency_key {
            headers.push(("Idempotency-Key".to_owned(), key.as_str().to_owned()));
        }

        Ok(HttpRequest {
            method,
            url,
            headers,
            body,
            connect_timeout: self.config.connect_timeout,
            request_timeout: self.config.request_timeout,
            max_response_body_bytes: self.config.max_response_body_bytes,
        })
    }
}

/// Successful HTTP response metadata returned by [`ControlPlaneClient::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedResponse {
    /// HTTP status code (`200..=299`).
    pub status: u16,
    /// Raw response body bytes.
    pub body: Vec<u8>,
    /// Total HTTP attempts made (including any 401 refresh replay).
    pub attempts: u8,
}

fn validate_access_token_format(token: &str) -> Result<(), ApiClientError> {
    if token.trim().is_empty() || token.len() > 4096 {
        return Err(ApiClientError::InvalidInput {
            field: "access_token",
            issue: "must be 1-4096 characters",
        });
    }
    if token.bytes().any(|b| b <= 0x20 || b >= 0x7F) {
        return Err(ApiClientError::InvalidInput {
            field: "access_token",
            issue: "must contain only visible ASCII characters",
        });
    }
    Ok(())
}

fn validate_uuid_param(field: &'static str, value: &str) -> Result<(), ApiClientError> {
    if !is_valid_uuid(value) {
        return Err(ApiClientError::InvalidInput {
            field,
            issue: "must be a valid UUID",
        });
    }
    Ok(())
}

fn map_validation_error(context: &'static str, err: &ValidationError) -> ApiClientError {
    match err {
        ValidationError::InvalidField { field, issue } => {
            ApiClientError::InvalidInput { field, issue }
        }
        ValidationError::LocalTargetField(_) => ApiClientError::InvalidInput {
            field: context,
            issue: "local destination fields are forbidden (INV-1)",
        },
        ValidationError::JsonTooDeep => ApiClientError::InvalidInput {
            field: context,
            issue: "JSON nesting depth exceeded",
        },
    }
}

fn serialize_request_body<S: Serialize>(value: &S) -> Result<Vec<u8>, ApiClientError> {
    serde_json::to_vec(value).map_err(|_| ApiClientError::InvalidInput {
        field: "body",
        issue: "failed to serialize request body",
    })
}

fn parse_json_checked(bytes: &[u8], attempts: u8) -> Result<serde_json::Value, ApiClientError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid JSON: {e}"),
            attempts,
        })?;
    scan_json_for_local_target(&value).map_err(|e| ApiClientError::InvalidResponse {
        reason: e.to_string(),
        attempts,
    })?;
    Ok(value)
}

/// Parse an error body while tolerating unknown fields and mapping unknown `error.code` values to
/// [`ApiErrorCode::Internal`] per `contracts.md` §14.
fn parse_error_response(
    status: u16,
    bytes: &[u8],
    attempts: u8,
) -> Result<ErrorResponse, ApiClientError> {
    let value = parse_json_checked(bytes, attempts)?;
    let error_obj = value
        .get("error")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ApiClientError::InvalidResponse {
            reason: format!("HTTP {status} response missing 'error' object"),
            attempts,
        })?;

    let code_str = error_obj
        .get("code")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiClientError::InvalidResponse {
            reason: "error.code must be a string".to_owned(),
            attempts,
        })?;

    let code =
        serde_json::from_value::<ApiErrorCode>(serde_json::Value::String(code_str.to_owned()))
            .unwrap_or(ApiErrorCode::Internal);

    let message = error_obj
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiClientError::InvalidResponse {
            reason: "error.message must be a string".to_owned(),
            attempts,
        })?
        .to_owned();

    let request_id = error_obj
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiClientError::InvalidResponse {
            reason: "error.request_id must be a string".to_owned(),
            attempts,
        })?
        .to_owned();

    let details = match error_obj.get("details") {
        Some(serde_json::Value::Array(items)) => {
            let mut parsed_details = Vec::with_capacity(items.len());
            for item in items {
                let obj = item
                    .as_object()
                    .ok_or_else(|| ApiClientError::InvalidResponse {
                        reason: "error.details item must be an object".to_owned(),
                        attempts,
                    })?;
                let field = obj
                    .get("field")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| ApiClientError::InvalidResponse {
                        reason: "error.details.field must be a string".to_owned(),
                        attempts,
                    })?
                    .to_owned();
                let issue = obj
                    .get("issue")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| ApiClientError::InvalidResponse {
                        reason: "error.details.issue must be a string".to_owned(),
                        attempts,
                    })?
                    .to_owned();
                parsed_details.push(ErrorDetail { field, issue });
            }
            Some(parsed_details)
        }
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err(ApiClientError::InvalidResponse {
                reason: "error.details must be an array".to_owned(),
                attempts,
            })
        }
    };

    let response = ErrorResponse {
        error: ErrorBody {
            code,
            message,
            request_id,
            details,
        },
    };
    response
        .validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(response)
}

/// Forward-compatible helper that ignores unknown response fields (`contracts.md` §14) while
/// still validating all required fields and `INV-7` on [`Tunnel`].
fn parse_tunnel_response(bytes: &[u8], attempts: u8) -> Result<Tunnel, ApiClientError> {
    let value = parse_json_checked(bytes, attempts)?;
    parse_tunnel_value(&value, attempts)
}

fn parse_tunnel_value(value: &serde_json::Value, attempts: u8) -> Result<Tunnel, ApiClientError> {
    #[derive(Deserialize)]
    struct RelaxedTunnelMapping {
        mapping_key: String,
        protocol: Protocol,
        position: u8,
        enabled: bool,
    }

    #[derive(Deserialize)]
    struct RelaxedTunnel {
        id: String,
        name: String,
        game_profile_id: String,
        profile_version: i32,
        state: TunnelState,
        mappings: Vec<RelaxedTunnelMapping>,
        created_at: String,
        #[serde(default)]
        started_at: Option<String>,
        #[serde(default)]
        stopped_at: Option<String>,
        expires_at: String,
        #[serde(default)]
        termination_reason: Option<String>,
    }

    let relaxed: RelaxedTunnel =
        serde_json::from_value(value.clone()).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid Tunnel payload: {e}"),
            attempts,
        })?;

    let tunnel = Tunnel {
        id: relaxed.id,
        name: relaxed.name,
        game_profile_id: relaxed.game_profile_id,
        profile_version: relaxed.profile_version,
        state: relaxed.state,
        mappings: relaxed
            .mappings
            .into_iter()
            .map(|m| TunnelMappingDto {
                mapping_key: m.mapping_key,
                protocol: m.protocol,
                position: m.position,
                enabled: m.enabled,
            })
            .collect(),
        created_at: relaxed.created_at,
        started_at: relaxed.started_at,
        stopped_at: relaxed.stopped_at,
        expires_at: relaxed.expires_at,
        termination_reason: relaxed.termination_reason,
    };
    tunnel
        .validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(tunnel)
}

fn parse_start_tunnel_response(
    bytes: &[u8],
    attempts: u8,
) -> Result<StartTunnelResponse, ApiClientError> {
    #[derive(Deserialize)]
    struct RelaxedRelayEndpoint {
        region: String,
        host: String,
        tunnel_port: u16,
    }
    #[derive(Deserialize)]
    struct RelaxedAllocatedPort {
        mapping_key: String,
        protocol: Protocol,
        public_port: u16,
    }
    #[derive(Deserialize)]
    struct RelaxedAllocation {
        id: String,
        expires_at: String,
        relay: RelaxedRelayEndpoint,
        ports: Vec<RelaxedAllocatedPort>,
    }
    #[derive(Deserialize)]
    struct RelaxedStartTunnel {
        tunnel_id: String,
        allocation_token: String,
        allocation: RelaxedAllocation,
    }

    let value = parse_json_checked(bytes, attempts)?;
    let relaxed: RelaxedStartTunnel =
        serde_json::from_value(value).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid StartTunnelResponse payload: {e}"),
            attempts,
        })?;

    let dto = StartTunnelResponse {
        tunnel_id: relaxed.tunnel_id,
        allocation_token: relaxed.allocation_token,
        allocation: AllocationDto {
            id: relaxed.allocation.id,
            expires_at: relaxed.allocation.expires_at,
            relay: RelayEndpointDto {
                region: relaxed.allocation.relay.region,
                host: relaxed.allocation.relay.host,
                tunnel_port: relaxed.allocation.relay.tunnel_port,
            },
            ports: relaxed
                .allocation
                .ports
                .into_iter()
                .map(|p| AllocatedPortDto {
                    mapping_key: p.mapping_key,
                    protocol: p.protocol,
                    public_port: p.public_port,
                })
                .collect(),
        },
    };
    dto.validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(dto)
}

fn parse_allocation_token_response(
    bytes: &[u8],
    attempts: u8,
) -> Result<AllocationTokenResponse, ApiClientError> {
    let value = parse_json_checked(bytes, attempts)?;
    let dto: AllocationTokenResponse =
        serde_json::from_value(value).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid AllocationTokenResponse payload: {e}"),
            attempts,
        })?;
    dto.validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(dto)
}

fn parse_create_invitation_response(
    bytes: &[u8],
    attempts: u8,
) -> Result<CreateInvitationResponse, ApiClientError> {
    #[derive(Deserialize)]
    struct RelaxedCreateInvitation {
        id: String,
        tunnel_id: String,
        secret: String,
        max_claims: u8,
        claim_count: u8,
        created_at: String,
        expires_at: String,
    }

    let value = parse_json_checked(bytes, attempts)?;
    let relaxed: RelaxedCreateInvitation =
        serde_json::from_value(value).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid CreateInvitationResponse payload: {e}"),
            attempts,
        })?;

    let dto = CreateInvitationResponse {
        id: relaxed.id,
        tunnel_id: relaxed.tunnel_id,
        secret: relaxed.secret,
        max_claims: relaxed.max_claims,
        claim_count: relaxed.claim_count,
        created_at: relaxed.created_at,
        expires_at: relaxed.expires_at,
    };
    dto.validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(dto)
}

fn parse_claim_invitation_response(
    bytes: &[u8],
    attempts: u8,
) -> Result<ClaimInvitationResponse, ApiClientError> {
    #[derive(Deserialize)]
    struct RelaxedAllocatedPort {
        mapping_key: String,
        protocol: Protocol,
        public_port: u16,
    }
    #[derive(Deserialize)]
    struct RelaxedClaimRelay {
        region: String,
        host: String,
        ports: Vec<RelaxedAllocatedPort>,
    }
    #[derive(Deserialize)]
    struct RelaxedClaimInvitation {
        tunnel_id: String,
        game_profile_id: String,
        relay: RelaxedClaimRelay,
        admitted_until: String,
    }

    let value = parse_json_checked(bytes, attempts)?;
    let relaxed: RelaxedClaimInvitation =
        serde_json::from_value(value).map_err(|e| ApiClientError::InvalidResponse {
            reason: format!("invalid ClaimInvitationResponse payload: {e}"),
            attempts,
        })?;

    let dto = ClaimInvitationResponse {
        tunnel_id: relaxed.tunnel_id,
        game_profile_id: relaxed.game_profile_id,
        relay: ClaimRelayDto {
            region: relaxed.relay.region,
            host: relaxed.relay.host,
            ports: relaxed
                .relay
                .ports
                .into_iter()
                .map(|p| AllocatedPortDto {
                    mapping_key: p.mapping_key,
                    protocol: p.protocol,
                    public_port: p.public_port,
                })
                .collect(),
        },
        admitted_until: relaxed.admitted_until,
    };
    dto.validate()
        .map_err(|e| ApiClientError::InvalidResponse {
            reason: e.to_string(),
            attempts,
        })?;
    Ok(dto)
}
