//! Unit, security, and failure-injection tests for `T-045` (`SEC-008`, `SEC-015`),
//! `T-046` (`SEC-013`, `PRIV-002`), and `T-048` (`REL-004`).

use chrono::{Duration, Utc};
use uuid::Uuid;

use gtt_control_plane::db::audit::AuditAction;
use gtt_control_plane::db::entities::{AllocationPort, GuestAdmission, RelayAllocation, RelayNode};
use gtt_control_plane::keys::{KeyId, SigningKeyMaterial, SigningKeyRing, DEFAULT_TOKEN_ISSUER};
use gtt_control_plane::limiter::{
    enforce_quota, QuotaConfig, RateLimitBucketConfig, RateLimitClass, RateLimitConfig,
    RateLimitDecision, RateLimiter,
};
use gtt_control_plane::redaction::{mask_ip_for_owner, redact_log_line, scan_for_sensitive_leaks};
use gtt_control_plane::routes::relay_internal::{
    build_relay_state_snapshot, parse_relay_report_request, parse_state_cursor,
};
use gtt_protocol::http::{ApiErrorCode, Protocol};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn rate_limiter_enforces_per_key_global_and_bounded_key_table_and_quotas() -> TestResult {
    let config = RateLimitConfig {
        strict: RateLimitBucketConfig {
            max_requests: 2,
            window_seconds: 60,
        },
        normal: RateLimitBucketConfig {
            max_requests: 3,
            window_seconds: 60,
        },
        lenient: RateLimitBucketConfig {
            max_requests: 5,
            window_seconds: 60,
        },
        global_ceiling_per_window: 6,
        max_tracked_keys: 3,
    };
    let mut limiter = RateLimiter::new(config);
    let t0 = 1_700_000_000i64;

    // 1. Strict class allows 2 requests for IP A, then returns 429 rate_limited on the 3rd.
    assert_eq!(
        limiter.check_and_record(RateLimitClass::Strict, "203.0.113.10", t0),
        RateLimitDecision::Allowed { remaining: 1 }
    );
    assert_eq!(
        limiter.check_and_record(RateLimitClass::Strict, "203.0.113.10", t0 + 10),
        RateLimitDecision::Allowed { remaining: 0 }
    );
    let third = limiter.check_and_record(RateLimitClass::Strict, "203.0.113.10", t0 + 15);
    assert_eq!(
        third,
        RateLimitDecision::RateLimited {
            retry_after_seconds: 45
        }
    );
    let failure = third
        .into_result("req_rl_1")
        .err()
        .ok_or("expected 429 rate_limited failure")?;
    assert_eq!(failure.failure.status, 429);
    assert_eq!(failure.failure.code(), ApiErrorCode::RateLimited);
    assert_eq!(failure.retry_after_seconds, 45);
    failure.failure.response.validate()?;

    // 2. Window rollover resets the counter.
    assert_eq!(
        limiter.check_and_record(RateLimitClass::Strict, "203.0.113.10", t0 + 61),
        RateLimitDecision::Allowed { remaining: 1 }
    );

    // 3. Key-table bound (`max_tracked_keys = 3`) evicts old keys so memory stays bounded.
    let _ = limiter.check_and_record(RateLimitClass::Normal, "user-1", t0);
    let _ = limiter.check_and_record(RateLimitClass::Normal, "user-2", t0 + 1);
    let _ = limiter.check_and_record(RateLimitClass::Normal, "user-3", t0 + 2);
    let _ = limiter.check_and_record(RateLimitClass::Normal, "user-4", t0 + 3);
    assert!(
        limiter.tracked_key_count() <= 3,
        "tracked keys must never exceed max_tracked_keys (AGENTS.md §4)"
    );

    // 4. Account quota enforcement returns 403 `quota_exceeded` (`SEC-015`).
    let quota_cfg = QuotaConfig::default();
    assert!(enforce_quota(
        quota_cfg.max_live_tunnels_per_user - 1,
        quota_cfg.max_live_tunnels_per_user,
        "Live tunnel quota reached.",
        "req_q_ok"
    )
    .is_ok());

    let quota_err = enforce_quota(
        quota_cfg.max_live_tunnels_per_user,
        quota_cfg.max_live_tunnels_per_user,
        "Live tunnel quota reached.",
        "req_q_exceeded",
    )
    .err()
    .ok_or("expected 403 quota_exceeded")?;
    assert_eq!(quota_err.status, 403);
    assert_eq!(quota_err.code(), ApiErrorCode::QuotaExceeded);
    quota_err.response.validate()?;

    Ok(())
}

#[test]
fn audit_actions_and_log_redaction_leave_zero_tokens_secrets_or_raw_ips() -> TestResult {
    // 1. Every canonical audit action required by SEC-013 is non-empty and distinct.
    assert_eq!(AuditAction::ALL.len(), 17);
    for (idx, action) in AuditAction::ALL.iter().enumerate() {
        assert!(!action.as_str().is_empty());
        if let Some(earlier) = AuditAction::ALL.get(..idx) {
            assert!(!earlier.iter().any(|prev| prev.as_str() == action.as_str()));
        }
    }

    // 2. Owner-facing IP masking (`contracts.md` §5.13).
    assert_eq!(
        mask_ip_for_owner("203.0.113.42"),
        Some("203.0.113.0/24".to_owned())
    );
    assert_eq!(
        mask_ip_for_owner("2001:db8:abcd:1234::1"),
        Some("2001:db8:abcd::/48".to_owned())
    );
    assert_eq!(mask_ip_for_owner("not-an-ip"), None);

    // 3. Leak scanner catches planted secrets, tokens, and raw IPs, and confirms zero leaks
    //    after `redact_log_line` (`PRIV-002`, `SEC-013`).
    let raw_text_log = "INFO request Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c secret=AbCdEfGhIjKlMnOpQrStUv client=203.0.113.42:25565 ipv6=2001:db8::1";
    let pre_findings = scan_for_sensitive_leaks(raw_text_log);
    assert!(
        !pre_findings.is_empty(),
        "leak scanner must detect unredacted bearer token, secret, and raw IPs"
    );

    let redacted_text = redact_log_line(raw_text_log);
    assert!(!redacted_text.contains("AbCdEfGhIjKlMnOpQrStUv"));
    assert!(!redacted_text.contains("203.0.113.42"));
    assert!(!redacted_text.contains("2001:db8::1"));
    assert_eq!(
        scan_for_sensitive_leaks(&redacted_text),
        Vec::<&str>::new(),
        "redacted text log must have zero sensitive leaks"
    );

    let raw_json_log = r#"{"event":"claim","secret":"AbCdEfGhIjKlMnOpQrStUv","refresh_token":"rt_secret_value","allocation_token":"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0dW5uZWwifQ.signature_part_12345","source_ip":"198.51.100.7"}"#;
    assert!(!scan_for_sensitive_leaks(raw_json_log).is_empty());
    let redacted_json = redact_log_line(raw_json_log);
    assert!(!redacted_json.contains("AbCdEfGhIjKlMnOpQrStUv"));
    assert!(!redacted_json.contains("rt_secret_value"));
    assert!(!redacted_json.contains("198.51.100.7"));
    assert_eq!(
        scan_for_sensitive_leaks(&redacted_json),
        Vec::<&str>::new(),
        "redacted JSON log must have zero sensitive leaks"
    );

    Ok(())
}

#[test]
fn relay_restart_recovery_restores_full_snapshot_and_cursor_poll_returns_deltas() -> TestResult {
    let t0 = Utc::now();
    let relay_id = Uuid::new_v4();
    let tunnel_id = Uuid::new_v4();
    let alloc_live_id = Uuid::new_v4();
    let alloc_old_released_id = Uuid::new_v4();
    let mapping_id = Uuid::new_v4();
    let adm_live_id = Uuid::new_v4();

    let relay_node = RelayNode {
        id: relay_id,
        name: "relay-sg-1".to_owned(),
        region: "ap-southeast-1".to_owned(),
        public_host: "sg1.relay.example.invalid".to_owned(),
        status: "healthy".to_owned(),
        max_allocations: 50,
        port_range_start: 20000,
        port_range_end: 20100,
        created_at: t0 - Duration::hours(2),
        last_health_at: Some(t0),
    };

    let kid = KeyId::new("kid-t048")?;
    let mat = SigningKeyMaterial::from_bytes([0x48; 32])?;
    let key_ring = SigningKeyRing::new(DEFAULT_TOKEN_ISSUER, kid, mat, t0.timestamp());

    let alloc_live = RelayAllocation {
        id: alloc_live_id,
        tunnel_id,
        relay_id,
        state: "active".to_owned(),
        issued_at: t0 - Duration::minutes(10),
        expires_at: t0 + Duration::minutes(50),
        released_at: None,
    };
    let port_live = AllocationPort {
        allocation_id: alloc_live_id,
        mapping_id,
        relay_id,
        protocol: "tcp".to_owned(),
        public_port: 20001,
        released_at: None,
    };

    let alloc_old_released = RelayAllocation {
        id: alloc_old_released_id,
        tunnel_id,
        relay_id,
        state: "released".to_owned(),
        issued_at: t0 - Duration::hours(1),
        expires_at: t0 - Duration::minutes(10),
        released_at: Some(t0 - Duration::minutes(30)),
    };

    let adm_live = GuestAdmission {
        id: adm_live_id,
        invitation_id: Uuid::new_v4(),
        tunnel_id,
        source_ip: "203.0.113.55".to_owned(),
        created_at: t0 - Duration::minutes(5),
        expires_at: t0 + Duration::minutes(55),
        revoked_at: None,
    };

    let allocations = vec![
        (alloc_live.clone(), vec![port_live.clone()]),
        (alloc_old_released, Vec::new()),
    ];
    let admissions = vec![adm_live.clone()];

    // 1. Simulated relay restart (`since = None`): returns full snapshot of live reservations,
    //    ports, admissions, and verification keys (`REL-004`).
    let recovery =
        build_relay_state_snapshot(&relay_node, &key_ring, &allocations, &admissions, None, t0)
            .map_err(|e| format!("recovery snapshot failed: {e:?}"))?;

    assert!(recovery.full_snapshot);
    assert!(!recovery.draining);
    assert_eq!(recovery.verification_keys.len(), 1);
    assert_eq!(recovery.reservations.len(), 1);
    let res0 = recovery
        .reservations
        .first()
        .ok_or("missing restored reservation")?;
    assert_eq!(res0.allocation_id, alloc_live_id.to_string());
    assert_eq!(res0.ports.len(), 1);
    assert_eq!(
        res0.ports.first().map(|p| (p.protocol, p.public_port)),
        Some((Protocol::Tcp, 20001))
    );
    assert_eq!(recovery.admissions.len(), 1);
    assert_eq!(recovery.admissions.first().map(|a| a.active), Some(true));

    // 2. Subsequent delta poll using `recovery.next_cursor` after `adm_live` is revoked at `t0 + 5s`.
    let cursor_ts = parse_state_cursor(&recovery.next_cursor)?;
    let t1 = t0 + Duration::seconds(5);
    let mut adm_revoked = adm_live;
    adm_revoked.revoked_at = Some(t1);

    let delta = build_relay_state_snapshot(
        &relay_node,
        &key_ring,
        &allocations,
        &[adm_revoked],
        Some(cursor_ts),
        t1,
    )
    .map_err(|e| format!("delta snapshot failed: {e:?}"))?;

    assert!(!delta.full_snapshot);
    assert!(
        delta.reservations.is_empty(),
        "unchanged reservation must not be repeated in delta poll"
    );
    assert_eq!(delta.admissions.len(), 1);
    assert_eq!(
        delta.admissions.first().map(|a| a.active),
        Some(false),
        "revoked admission must be surfaced with active=false in delta poll"
    );

    // 3. Validate RelayReportRequest parser (`INV-1` and counter bounds).
    let valid_report = format!(
        r#"{{"healthy":true,"allocations":[{{"allocation_id":"{alloc_live_id}","tunnel_id":"{tunnel_id}","connected":true,"interval_start":"2026-10-16T12:00:00Z","bytes_from_guests":1024,"bytes_to_guests":2048,"peak_flows":3,"datagrams_dropped":0}}]}}"#
    );
    let parsed_report = parse_relay_report_request(&valid_report, "req_rep_ok")
        .map_err(|e| format!("expected valid report, got: {e:?}"))?;
    assert!(parsed_report.healthy);
    assert_eq!(parsed_report.allocations.len(), 1);

    let inv1_report = r#"{"healthy":true,"local_port":25565,"allocations":[]}"#;
    assert!(parse_relay_report_request(inv1_report, "req_rep_inv1").is_err());

    let neg_report = format!(
        r#"{{"healthy":true,"allocations":[{{"allocation_id":"{alloc_live_id}","tunnel_id":"{tunnel_id}","connected":true,"interval_start":"2026-10-16T12:00:00Z","bytes_from_guests":-1,"bytes_to_guests":0,"peak_flows":0,"datagrams_dropped":0}}]}}"#
    );
    assert!(parse_relay_report_request(&neg_report, "req_rep_neg").is_err());

    Ok(())
}

// ---------------------------------------------------------------------------
// T-044 -- Invitation secret generation/hashing (SEC-007) and request validation
// ---------------------------------------------------------------------------

#[test]
fn t044_invitation_secret_entropy_hashing_and_request_validation() -> TestResult {
    use gtt_control_plane::routes::invitations::{
        generate_invitation_secret, hash_invitation_secret, parse_claim_invitation_request,
        parse_create_invitation_request, INVITATION_SECRET_PREFIX,
    };
    use gtt_protocol::validate::is_valid_invitation_secret;

    // 1. Generated secrets carry the `gtt_inv_` prefix, are 40 characters (`>= 128` bits of
    //    CSPRNG entropy), match `^[A-Za-z0-9_-]{22,64}$`, and never collide across 64 samples.
    let mut seen_secrets = std::collections::HashSet::new();
    let mut seen_hashes = std::collections::HashSet::new();
    for _ in 0..64 {
        let secret = generate_invitation_secret();
        assert!(secret.starts_with(INVITATION_SECRET_PREFIX));
        assert_eq!(secret.len(), 40);
        assert!(is_valid_invitation_secret(&secret));
        assert!(seen_secrets.insert(secret.clone()));

        let hash = hash_invitation_secret(&secret);
        assert_ne!(hash, [0u8; 32]);
        assert_eq!(hash, hash_invitation_secret(&secret));
        assert!(seen_hashes.insert(hash));
    }

    // 2. CreateInvitationRequest parser enforces bounds (`60..=604800`, `1..=50`) and INV-1.
    let valid_create = r#"{"expires_in_seconds":3600,"max_claims":4}"#;
    let parsed_create = parse_create_invitation_request(valid_create, "req_inv_ok")
        .map_err(|e| format!("expected valid create invitation request, got: {e:?}"))?;
    assert_eq!(parsed_create.expires_in_seconds, 3600);
    assert_eq!(parsed_create.max_claims, 4);

    for invalid_create in [
        r#"{"expires_in_seconds":59,"max_claims":4}"#,
        r#"{"expires_in_seconds":604801,"max_claims":4}"#,
        r#"{"expires_in_seconds":3600,"max_claims":0}"#,
        r#"{"expires_in_seconds":3600,"max_claims":51}"#,
        r#"{"expires_in_seconds":3600,"max_claims":4,"local_port":25565}"#,
    ] {
        assert!(
            parse_create_invitation_request(invalid_create, "req_inv_bad").is_err(),
            "expected rejection for {invalid_create}"
        );
    }

    // 3. ClaimInvitationRequest parser enforces secret format and INV-1.
    let sample_secret = generate_invitation_secret();
    let valid_claim = format!(r#"{{"secret":"{sample_secret}"}}"#);
    let parsed_claim = parse_claim_invitation_request(&valid_claim, "req_claim_ok")
        .map_err(|e| format!("expected valid claim request, got: {e:?}"))?;
    assert_eq!(parsed_claim.secret, sample_secret);

    for invalid_claim in [
        r#"{"secret":"too-short"}"#,
        r#"{"secret":"gtt_inv_1234567890123456789012","local_host":"127.0.0.1"}"#,
    ] {
        assert!(
            parse_claim_invitation_request(invalid_claim, "req_claim_bad").is_err(),
            "expected rejection for {invalid_claim}"
        );
    }

    Ok(())
}

#[test]
fn t109_retention_policy_config_defaults_and_cutoff_computations() -> TestResult {
    use gtt_control_plane::db::RetentionPolicyConfig;

    let cfg = RetentionPolicyConfig::default();

    // Verify concrete retention intervals (PRIV-003, T-109).
    assert_eq!(cfg.guest_admissions, Duration::hours(24));
    assert_eq!(cfg.idempotency_keys, Duration::hours(24));
    assert_eq!(cfg.auth_sessions, Duration::days(7));
    assert_eq!(cfg.terminal_tunnels, Duration::days(30));
    assert_eq!(cfg.usage_records, Duration::days(90));
    assert_eq!(cfg.audit_events, Duration::days(90));
    assert_eq!(cfg.soft_deleted_users, Duration::days(30));

    // Verify cutoff computation for a fixed timestamp.
    let now = Utc::now();
    let guest_cutoff = now
        .checked_sub_signed(cfg.guest_admissions)
        .ok_or("underflow")?;
    let audit_cutoff = now
        .checked_sub_signed(cfg.audit_events)
        .ok_or("underflow")?;

    assert!(guest_cutoff < now);
    assert!(audit_cutoff < guest_cutoff);
    assert_eq!((now - guest_cutoff).num_seconds(), 24 * 3600);
    assert_eq!((now - audit_cutoff).num_seconds(), 90 * 24 * 3600);

    Ok(())
}

#[test]
fn t098_audit_events_and_logs_redaction_and_data_minimization() -> TestResult {
    use gtt_control_plane::redaction::{
        mask_ip_for_owner, redact_log_line, scan_for_sensitive_leaks,
    };

    // 1. IP masking for owner display (PRIV-002, contracts.md §5.13)
    let masked_v4 = mask_ip_for_owner("203.0.113.195").ok_or("masking failed")?;
    assert_eq!(masked_v4, "203.0.113.0/24");

    let masked_v6 =
        mask_ip_for_owner("2001:0db8:85a3:0000:0000:8a2e:0370:7334").ok_or("masking failed")?;
    assert_eq!(masked_v6, "2001:db8:85a3::/48");

    // 2. Sensitive fields in JSON logs are redacted
    let raw_log = serde_json::json!({
        "event": "user_sign_in",
        "access_token": "secret_access_token_12345",
        "refresh_token": "secret_refresh_token_67890",
        "peer_ip": "198.51.100.42",
        "actor_id": "00000000-0000-0000-0000-000000000001"
    })
    .to_string();

    let redacted_log = redact_log_line(&raw_log);
    let leaks = scan_for_sensitive_leaks(&redacted_log);
    assert!(
        leaks.is_empty(),
        "redacted log must contain zero leaks: {leaks:?}"
    );
    assert!(!redacted_log.contains("secret_access_token_12345"));
    assert!(!redacted_log.contains("secret_refresh_token_67890"));
    assert!(!redacted_log.contains("198.51.100.42"));

    // 3. Bearer token and raw JWT redaction in plain text logs
    let raw_text = "Handling request with Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyMSJ9.signature from guest at 192.0.2.1";
    let redacted_text = redact_log_line(raw_text);
    let leaks_text = scan_for_sensitive_leaks(&redacted_text);
    assert!(
        leaks_text.is_empty(),
        "redacted text must contain zero leaks: {leaks_text:?}"
    );

    Ok(())
}

#[test]
fn t098_account_deletion_policy_and_cascading_rules() -> TestResult {
    use gtt_control_plane::db::RetentionPolicyConfig;

    let cfg = RetentionPolicyConfig::default();

    // PRIV-004: Soft-deleted user accounts have a 30-day retention grace period
    assert_eq!(cfg.soft_deleted_users, Duration::days(30));

    // When an account is soft deleted, terminal tunnels have 30 days retention,
    // guest admissions have 24 hours retention, and auth sessions have 7 days.
    assert!(cfg.guest_admissions < cfg.soft_deleted_users);
    assert!(cfg.auth_sessions < cfg.soft_deleted_users);

    Ok(())
}
