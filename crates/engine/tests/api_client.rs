//! Fault-injection unit tests for the control-plane HTTP API client (`T-047`, `REL-003`,
//! `contracts.md` §§3, 4, 5, 6, 8, 14, `SEC-010`, `PRIV-002`).

use std::collections::VecDeque;
use std::time::Duration;

use gtt_engine::{
    compute_retry_delay, is_request_retryable, is_status_retryable, parse_retry_after,
    ApiClientError, ClientConfig, ControlPlaneClient, DeterministicBackoff, HttpMethod,
    HttpRequest, HttpResponse, HttpTransport, IdempotencyKey, NoTokenRefresher, TokenRefreshError,
    TokenRefresher, TransportError, MAX_ATTEMPTS,
};
use gtt_protocol::{
    ApiErrorCode, ClaimInvitationRequest, CreateInvitationRequest, CreateTunnelRequest,
    MappingRequest, Protocol, TunnelState,
};

const SAMPLE_TUNNEL_ID: &str = "3f2b8c1e-5a47-4d0b-9c1e-0a6f1d2e7b44";

#[derive(Debug, Default)]
struct ScriptedTransport {
    outcomes: VecDeque<Result<HttpResponse, TransportError>>,
    recorded_requests: Vec<HttpRequest>,
}

impl ScriptedTransport {
    fn new(outcomes: impl IntoIterator<Item = Result<HttpResponse, TransportError>>) -> Self {
        Self {
            outcomes: outcomes.into_iter().collect(),
            recorded_requests: Vec::new(),
        }
    }
}

impl HttpTransport for ScriptedTransport {
    fn send(&mut self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.recorded_requests.push(request.clone());
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(TransportError::Network(
                "scripted queue exhausted".to_owned(),
            ))
        })
    }
}

#[derive(Debug, Default)]
struct ScriptedRefresher {
    outcomes: VecDeque<Result<String, TokenRefreshError>>,
    calls: usize,
}

impl ScriptedRefresher {
    fn new(outcomes: impl IntoIterator<Item = Result<String, TokenRefreshError>>) -> Self {
        Self {
            outcomes: outcomes.into_iter().collect(),
            calls: 0,
        }
    }
}

impl TokenRefresher for ScriptedRefresher {
    fn refresh_access_token(&mut self) -> Result<String, TokenRefreshError> {
        self.calls += 1;
        self.outcomes
            .pop_front()
            .unwrap_or_else(|| Err(TokenRefreshError::new("scripted refresher queue exhausted")))
    }
}

fn json_response(status: u16, body_json: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status,
        headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body: body_json.as_bytes().to_vec(),
    })
}

fn json_response_with_headers(
    status: u16,
    headers: Vec<(&str, &str)>,
    body_json: &str,
) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status,
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        body: body_json.as_bytes().to_vec(),
    })
}

fn sample_tunnel_json(state: &str, stopped_at: &str) -> String {
    format!(
        r#"{{
            "id": "{SAMPLE_TUNNEL_ID}",
            "name": "Friday Terraria",
            "game_profile_id": "terraria",
            "profile_version": 1,
            "state": "{state}",
            "mappings": [
                {{"mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true}}
            ],
            "created_at": "2026-10-19T12:00:00Z",
            "started_at": null,
            "stopped_at": {stopped_at},
            "expires_at": "2026-10-19T18:00:00Z",
            "termination_reason": null
        }}"#
    )
}

fn sample_error_json(code: &str, message: &str) -> String {
    format!(
        r#"{{
            "error": {{
                "code": "{code}",
                "message": "{message}",
                "request_id": "req_t047_test"
            }}
        }}"#
    )
}

fn sample_create_tunnel_request() -> CreateTunnelRequest {
    CreateTunnelRequest {
        name: "Friday Terraria".to_owned(),
        game_profile_id: "terraria".to_owned(),
        mappings: vec![MappingRequest {
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            enabled: true,
        }],
    }
}

#[test]
fn client_config_enforces_https_and_bounded_timeouts_and_backoffs() {
    let valid = ClientConfig::new("https://api.example.invalid");
    assert!(valid.validate().is_ok());

    // SEC-010: plaintext HTTP is refused.
    let http_cfg = ClientConfig::new("http://api.example.invalid");
    assert!(matches!(
        http_cfg.validate(),
        Err(ApiClientError::InvalidConfig(_))
    ));

    // Empty host after scheme is refused.
    let empty_host = ClientConfig::new("https://");
    assert!(matches!(
        empty_host.validate(),
        Err(ApiClientError::InvalidConfig(_))
    ));

    // Zero or unbounded connect_timeout is refused.
    let mut zero_connect = valid.clone();
    zero_connect.connect_timeout = Duration::ZERO;
    assert!(zero_connect.validate().is_err());

    let mut huge_connect = valid.clone();
    huge_connect.connect_timeout = Duration::from_secs(31);
    assert!(huge_connect.validate().is_err());

    // request_timeout < connect_timeout or > 60s is refused.
    let mut inverted_req = valid.clone();
    inverted_req.connect_timeout = Duration::from_secs(10);
    inverted_req.request_timeout = Duration::from_secs(5);
    assert!(inverted_req.validate().is_err());

    let mut huge_req = valid.clone();
    huge_req.request_timeout = Duration::from_secs(61);
    assert!(huge_req.validate().is_err());

    // Zero base_backoff or max_backoff < base_backoff is refused.
    let mut zero_backoff = valid.clone();
    zero_backoff.base_backoff = Duration::ZERO;
    assert!(zero_backoff.validate().is_err());

    let mut inverted_backoff = valid;
    inverted_backoff.base_backoff = Duration::from_secs(6);
    inverted_backoff.max_backoff = Duration::from_secs(5);
    assert!(inverted_backoff.validate().is_err());
}

#[test]
fn idempotency_key_enforces_8_to_128_visible_ascii_bounds() {
    assert!(IdempotencyKey::new("short77").is_err());
    assert!(IdempotencyKey::new("valid-key-8").is_ok());
    assert!(IdempotencyKey::new("k".repeat(128)).is_ok());
    assert!(IdempotencyKey::new("k".repeat(129)).is_err());
    assert!(IdempotencyKey::new("bad space key").is_err());
    assert!(IdempotencyKey::new("bad\nnewline").is_err());
}

#[test]
fn idempotent_get_and_post_with_key_retry_on_network_errors_and_stop_at_attempt_limit() {
    // 1. GET retries on ConnectTimeout + RequestTimeout and succeeds on attempt 3.
    let transport = ScriptedTransport::new([
        Err(TransportError::ConnectTimeout),
        Err(TransportError::RequestTimeout),
        json_response(200, &sample_tunnel_json("created", "null")),
    ]);
    let client = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_initial_valid".to_owned()),
        transport,
        DeterministicBackoff::with_jitter_bps(10_000),
        NoTokenRefresher,
    );
    assert!(client.is_ok());
    if let Ok(mut client) = client {
        let res = client.get_tunnel(SAMPLE_TUNNEL_ID);
        assert!(res.is_ok());
        if let Ok(tunnel) = res {
            assert_eq!(tunnel.id, SAMPLE_TUNNEL_ID);
            assert_eq!(tunnel.state, TunnelState::Created);
        }
        assert_eq!(client.transport().recorded_requests.len(), 3);
        assert_eq!(
            client.backoff().recorded_delays,
            vec![Duration::from_millis(200), Duration::from_millis(400)]
        );
    }

    // 2. POST with Idempotency-Key retries on transient network errors and stops at MAX_ATTEMPTS (3).
    let transport_exhaust = ScriptedTransport::new([
        Err(TransportError::Network("connection reset".to_owned())),
        Err(TransportError::ConnectTimeout),
        Err(TransportError::RequestTimeout),
        // A 4th outcome that must NEVER be reached because MAX_ATTEMPTS == 3.
        json_response(201, &sample_tunnel_json("created", "null")),
    ]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_initial_valid".to_owned()),
        transport_exhaust,
        DeterministicBackoff::with_jitter_bps(10_000),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let idem = IdempotencyKey::new("idem-create-tunnel-0001");
        assert!(idem.is_ok());
        if let Ok(idem) = idem {
            let err = client.create_tunnel(&sample_create_tunnel_request(), &idem);
            assert!(matches!(
                err,
                Err(ApiClientError::Transport {
                    source: TransportError::RequestTimeout,
                    attempts: MAX_ATTEMPTS,
                })
            ));
            assert_eq!(
                client.transport().recorded_requests.len(),
                usize::from(MAX_ATTEMPTS)
            );
            // All 3 attempts must carry the exact same Idempotency-Key header.
            for req in &client.transport().recorded_requests {
                assert_eq!(
                    req.header("Idempotency-Key"),
                    Some("idem-create-tunnel-0001")
                );
            }
            assert_eq!(client.backoff().recorded_delays.len(), 2);
        }
    }
}

#[test]
fn idempotent_requests_retry_on_429_502_503_504_with_exponential_backoff_and_retry_after() {
    let start_ok_json = format!(
        r#"{{
            "tunnel_id": "{SAMPLE_TUNNEL_ID}",
            "allocation_token": "alloc_tok_signed_example",
            "allocation": {{
                "id": "8a9b0c1d-2e3f-4a5b-8c9d-0e1f2a3b4c5d",
                "expires_at": "2026-10-19T13:00:00Z",
                "relay": {{
                    "region": "us-east",
                    "host": "relay1.example.invalid",
                    "tunnel_port": 4433
                }},
                "ports": [
                    {{"mapping_key": "game", "protocol": "tcp", "public_port": 25565}}
                ]
            }}
        }}"#
    );

    // 1. POST /v1/tunnels/{id}/start with Idempotency-Key retries on 429 (with Retry-After: 3)
    //    and 503 (relay_unavailable), succeeding on attempt 3.
    let transport = ScriptedTransport::new([
        json_response_with_headers(
            429,
            vec![("Retry-After", "3")],
            &sample_error_json("rate_limited", "Too many requests."),
        ),
        json_response(
            503,
            &sample_error_json("relay_unavailable", "No healthy relay."),
        ),
        json_response(200, &start_ok_json),
    ]);

    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_user_1".to_owned()),
        transport,
        DeterministicBackoff::with_jitter_bps(10_000),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let idem = IdempotencyKey::new("idem-start-tunnel-0001");
        assert!(idem.is_ok());
        if let Ok(idem) = idem {
            let started = client.start_tunnel(SAMPLE_TUNNEL_ID, &idem);
            assert!(started.is_ok());
            assert_eq!(client.transport().recorded_requests.len(), 3);
            assert_eq!(
                client.backoff().recorded_delays,
                vec![Duration::from_secs(3), Duration::from_millis(400)]
            );
        }
    }

    // 2. GET /v1/tunnels retries on 502 and 504, and stops at attempt 3 if 504 persists.
    let transport_5xx = ScriptedTransport::new([
        json_response(502, &sample_error_json("internal", "Bad gateway.")),
        json_response_with_headers(
            504,
            vec![("Retry-After", "999999")],
            &sample_error_json("internal", "Gateway timeout."),
        ),
        json_response(504, &sample_error_json("internal", "Gateway timeout.")),
        json_response(200, "[]"),
    ]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_user_1".to_owned()),
        transport_5xx,
        DeterministicBackoff::with_jitter_bps(0),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let err = client.list_tunnels();
        assert!(matches!(
            err,
            Err(ApiClientError::ServerError {
                status: 504,
                attempts: MAX_ATTEMPTS,
                ..
            })
        ));
        assert_eq!(client.transport().recorded_requests.len(), 3);
        // Jitter 0 bps gives 50% of 200ms = 100ms on retry 1, and Retry-After: 999999 is clamped
        // to DEFAULT_MAX_RETRY_AFTER (30s) on retry 2.
        assert_eq!(
            client.backoff().recorded_delays,
            vec![Duration::from_millis(100), Duration::from_secs(30)]
        );
    }
}

#[test]
fn post_without_idempotency_key_never_retries_on_network_429_or_5xx() {
    assert!(!is_request_retryable(HttpMethod::Post, false));
    assert!(is_request_retryable(HttpMethod::Post, true));
    assert!(is_request_retryable(HttpMethod::Get, false));
    assert!(is_request_retryable(HttpMethod::Delete, false));

    // 1. POST /v1/tunnels/{id}/token (no Idempotency-Key) on network timeout -> 1 attempt only.
    let transport_timeout = ScriptedTransport::new([
        Err(TransportError::ConnectTimeout),
        json_response(
            200,
            r#"{"allocation_token":"tok","expires_at":"2026-10-19T13:00:00Z"}"#,
        ),
    ]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_user_1".to_owned()),
        transport_timeout,
        DeterministicBackoff::default(),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let res = client.refresh_allocation_token(SAMPLE_TUNNEL_ID);
        assert!(matches!(
            res,
            Err(ApiClientError::Transport {
                source: TransportError::ConnectTimeout,
                attempts: 1,
            })
        ));
        assert_eq!(client.transport().recorded_requests.len(), 1);
        assert!(client.backoff().recorded_delays.is_empty());
    }

    // 2. POST /v1/invitations/claim (no Idempotency-Key) on 429 or 503 -> 1 attempt only.
    for status in [429_u16, 502, 503, 504] {
        let code = if status == 429 {
            "rate_limited"
        } else {
            "relay_unavailable"
        };
        let transport = ScriptedTransport::new([
            json_response(status, &sample_error_json(code, "Transient failure.")),
            json_response(200, "{}"),
        ]);
        let client_res = ControlPlaneClient::new(
            ClientConfig::new("https://api.example.invalid"),
            None,
            transport,
            DeterministicBackoff::default(),
            NoTokenRefresher,
        );
        assert!(client_res.is_ok());
        if let Ok(mut client) = client_res {
            let claim_req = ClaimInvitationRequest {
                secret: "AbCdEfGhIjKlMnOpQrStUv".to_owned(),
            };
            let res = client.claim_invitation(&claim_req);
            assert!(matches!(
                res,
                Err(ApiClientError::ServerError {
                    status: s,
                    attempts: 1,
                    ..
                }) if s == status
            ));
            assert_eq!(client.transport().recorded_requests.len(), 1);
            assert!(client.backoff().recorded_delays.is_empty());
        }
    }
}

#[test]
fn non_429_4xx_and_500_and_tls_errors_never_retry_even_on_idempotent_requests() {
    let non_retryable_statuses = [
        (400_u16, "validation_failed"),
        (403, "forbidden"),
        (403, "quota_exceeded"),
        (404, "not_found"),
        (409, "conflict"),
        (422, "idempotency_mismatch"),
        (426, "client_upgrade_required"),
        (500, "internal"),
    ];

    for (status, code) in non_retryable_statuses {
        assert!(!is_status_retryable(status));
        let transport = ScriptedTransport::new([
            json_response(status, &sample_error_json(code, "Non-retryable status.")),
            json_response(201, &sample_tunnel_json("created", "null")),
        ]);
        let client_res = ControlPlaneClient::new(
            ClientConfig::new("https://api.example.invalid"),
            Some("tok_user_1".to_owned()),
            transport,
            DeterministicBackoff::default(),
            NoTokenRefresher,
        );
        assert!(client_res.is_ok());
        if let Ok(mut client) = client_res {
            let idem = IdempotencyKey::new("idem-non-retryable-01");
            assert!(idem.is_ok());
            if let Ok(idem) = idem {
                let res = client.create_tunnel(&sample_create_tunnel_request(), &idem);
                assert!(matches!(
                    res,
                    Err(ApiClientError::ServerError {
                        status: s,
                        attempts: 1,
                        ..
                    }) if s == status
                ));
                assert_eq!(client.transport().recorded_requests.len(), 1);
                assert!(client.backoff().recorded_delays.is_empty());
            }
        }
    }

    // SEC-010: TLS certificate validation failure aborts immediately without retry.
    let tls_transport = ScriptedTransport::new([
        Err(TransportError::TlsCertificateInvalid(
            "hostname mismatch".to_owned(),
        )),
        json_response(200, &sample_tunnel_json("created", "null")),
    ]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_user_1".to_owned()),
        tls_transport,
        DeterministicBackoff::default(),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let res = client.get_tunnel(SAMPLE_TUNNEL_ID);
        assert!(matches!(
            res,
            Err(ApiClientError::Transport {
                source: TransportError::TlsCertificateInvalid(_),
                attempts: 1,
            })
        ));
        assert_eq!(client.transport().recorded_requests.len(), 1);
        assert!(client.backoff().recorded_delays.is_empty());
    }
}

#[test]
fn http_401_triggers_token_refresh_at_most_once_and_returns_to_sign_in_on_failure() {
    // Case A: 401 on first attempt -> refresh succeeds once -> re-sent request with new bearer
    // token and preserved Idempotency-Key succeeds on attempt 2.
    let transport_ok = ScriptedTransport::new([
        json_response(
            401,
            &sample_error_json("unauthenticated", "Access token expired."),
        ),
        json_response(201, &sample_tunnel_json("created", "null")),
    ]);
    let refresher_ok = ScriptedRefresher::new([Ok("tok_refreshed_fresh_2".to_owned())]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_expired_1".to_owned()),
        transport_ok,
        DeterministicBackoff::default(),
        refresher_ok,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let idem = IdempotencyKey::new("idem-401-refresh-once");
        assert!(idem.is_ok());
        if let Ok(idem) = idem {
            let tunnel = client.create_tunnel(&sample_create_tunnel_request(), &idem);
            assert!(tunnel.is_ok());
            assert_eq!(client.refresher().calls, 1);
            assert_eq!(client.transport().recorded_requests.len(), 2);
            assert_eq!(
                client
                    .transport()
                    .recorded_requests
                    .first()
                    .and_then(|r| r.header("Authorization")),
                Some("Bearer tok_expired_1")
            );
            assert_eq!(
                client
                    .transport()
                    .recorded_requests
                    .get(1)
                    .and_then(|r| r.header("Authorization")),
                Some("Bearer tok_refreshed_fresh_2")
            );
            assert_eq!(
                client
                    .transport()
                    .recorded_requests
                    .get(1)
                    .and_then(|r| r.header("Idempotency-Key")),
                Some("idem-401-refresh-once")
            );
        }
    }

    // Case B: 401 on attempt 1 -> refresh succeeds -> attempt 2 STILL returns 401 -> stops
    // immediately without refreshing a second time and returns ReauthenticationRequired.
    let transport_double_401 = ScriptedTransport::new([
        json_response(
            401,
            &sample_error_json("unauthenticated", "Access token expired."),
        ),
        json_response(
            401,
            &sample_error_json("unauthenticated", "Still unauthenticated."),
        ),
        json_response(200, &sample_tunnel_json("created", "null")),
    ]);
    let refresher_twice = ScriptedRefresher::new([
        Ok("tok_refreshed_once".to_owned()),
        Ok("tok_should_never_be_requested".to_owned()),
    ]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_expired_1".to_owned()),
        transport_double_401,
        DeterministicBackoff::default(),
        refresher_twice,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let res = client.get_tunnel(SAMPLE_TUNNEL_ID);
        assert!(matches!(
            res,
            Err(ApiClientError::ReauthenticationRequired { attempts: 2, .. })
        ));
        assert_eq!(client.refresher().calls, 1);
        assert_eq!(client.transport().recorded_requests.len(), 2);
    }

    // Case C: 401 on attempt 1 -> refresh fails (e.g. rotated refresh token reuse) -> stops
    // immediately at attempt 1 and returns ReauthenticationRequired.
    let transport_refresh_fail = ScriptedTransport::new([
        json_response(
            401,
            &sample_error_json("unauthenticated", "Access token expired."),
        ),
        json_response(200, &sample_tunnel_json("created", "null")),
    ]);
    let refresher_fail =
        ScriptedRefresher::new([Err(TokenRefreshError::new("session family revoked"))]);
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some("tok_expired_1".to_owned()),
        transport_refresh_fail,
        DeterministicBackoff::default(),
        refresher_fail,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        let res = client.get_tunnel(SAMPLE_TUNNEL_ID);
        assert!(matches!(
            res,
            Err(ApiClientError::ReauthenticationRequired { attempts: 1, .. })
        ));
        assert_eq!(client.refresher().calls, 1);
        assert_eq!(client.transport().recorded_requests.len(), 1);
    }
}

#[test]
fn forward_compatibility_ignores_unknown_response_fields_maps_unknown_error_codes_and_enforces_inv1_and_redaction(
) {
    // 1. Server response with unknown additive fields (`contracts.md` §14) succeeds.
    let tunnel_with_extra_fields = format!(
        r#"{{
            "id": "{SAMPLE_TUNNEL_ID}",
            "name": "Friday Terraria",
            "game_profile_id": "terraria",
            "profile_version": 1,
            "state": "stopped",
            "mappings": [
                {{
                    "mapping_key": "game",
                    "protocol": "tcp",
                    "position": 0,
                    "enabled": true,
                    "future_mapping_flag": "ignored"
                }}
            ],
            "created_at": "2026-10-19T12:00:00Z",
            "started_at": "2026-10-19T12:01:00Z",
            "stopped_at": "2026-10-19T13:00:00Z",
            "expires_at": "2026-10-19T18:00:00Z",
            "termination_reason": "owner_stopped",
            "future_v1_additive_field": {{"nested": 42}}
        }}"#
    );

    // 2. Unknown error code in ErrorResponse maps to ApiErrorCode::Internal (`contracts.md` §14).
    let unknown_error_code_json = sample_error_json("future_v1_error_code", "New error condition.");

    // 3. Response containing a forbidden local-target field (`local_port`) is rejected (`INV-1`).
    let inv1_violating_json = format!(
        r#"{{
            "id": "{SAMPLE_TUNNEL_ID}",
            "name": "Friday Terraria",
            "game_profile_id": "terraria",
            "profile_version": 1,
            "state": "created",
            "mappings": [
                {{"mapping_key": "game", "protocol": "tcp", "position": 0, "enabled": true, "local_port": 25565}}
            ],
            "created_at": "2026-10-19T12:00:00Z",
            "started_at": null,
            "stopped_at": null,
            "expires_at": "2026-10-19T18:00:00Z",
            "termination_reason": null
        }}"#
    );

    let transport = ScriptedTransport::new([
        json_response(200, &tunnel_with_extra_fields),
        json_response(400, &unknown_error_code_json),
        json_response(200, &inv1_violating_json),
    ]);

    let secret_token = "super_secret_bearer_token_do_not_leak";
    let client_res = ControlPlaneClient::new(
        ClientConfig::new("https://api.example.invalid"),
        Some(secret_token.to_owned()),
        transport,
        DeterministicBackoff::default(),
        NoTokenRefresher,
    );
    assert!(client_res.is_ok());
    if let Ok(mut client) = client_res {
        // Call 1: forward-compatible Tunnel succeeds.
        let stop_idem = IdempotencyKey::new("idem-stop-tunnel-0001");
        assert!(stop_idem.is_ok());
        if let Ok(stop_idem) = stop_idem {
            let stopped = client.stop_tunnel(SAMPLE_TUNNEL_ID, Some(&stop_idem));
            assert!(stopped.is_ok());
            if let Ok(tunnel) = stopped {
                assert_eq!(tunnel.state, TunnelState::Stopped);
            }
        }

        // Call 2: unknown error code maps to ApiErrorCode::Internal.
        let inv_idem = IdempotencyKey::new("idem-create-inv-00001");
        assert!(inv_idem.is_ok());
        if let Ok(inv_idem) = inv_idem {
            let inv_req = CreateInvitationRequest {
                expires_in_seconds: 3600,
                max_claims: 4,
            };
            let err = client.create_invitation(SAMPLE_TUNNEL_ID, &inv_req, &inv_idem);
            assert!(matches!(
                err,
                Err(ApiClientError::ServerError {
                    status: 400,
                    response,
                    attempts: 1,
                }) if response.error.code == ApiErrorCode::Internal
            ));
        }

        // Call 3: INV-1 violation in response is caught.
        let inv1_res = client.get_tunnel(SAMPLE_TUNNEL_ID);
        assert!(matches!(
            inv1_res,
            Err(ApiClientError::InvalidResponse { attempts: 1, .. })
        ));

        // PRIV-002: Debug output on client and recorded HttpRequests never leaks secret_token.
        let client_debug = format!("{client:?}");
        assert!(!client_debug.contains(secret_token));
        assert!(client_debug.contains("<redacted>"));
        for req in &client.transport().recorded_requests {
            let req_debug = format!("{req:?}");
            assert!(!req_debug.contains(secret_token));
            assert!(req_debug.contains("<redacted>"));
        }
    }
}

#[test]
fn retry_after_and_exponential_delay_bounds_hold() {
    let cfg = ClientConfig::new("https://api.example.invalid");
    assert_eq!(
        parse_retry_after("12", cfg.max_retry_after),
        Some(Duration::from_secs(12))
    );
    assert_eq!(
        parse_retry_after("9999", cfg.max_retry_after),
        Some(cfg.max_retry_after)
    );
    assert_eq!(parse_retry_after("-5", cfg.max_retry_after), None);
    assert_eq!(parse_retry_after("abc", cfg.max_retry_after), None);

    // Retry 1 with 0..=10_000 bps jitter is within [100ms, 200ms].
    let d_min = compute_retry_delay(&cfg, 1, 0, None);
    let d_max = compute_retry_delay(&cfg, 1, 10_000, None);
    assert_eq!(d_min, Duration::from_millis(100));
    assert_eq!(d_max, Duration::from_millis(200));
}
