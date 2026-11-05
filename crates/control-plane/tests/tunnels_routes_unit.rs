//! Unit and schema-validation tests for `api.md` Routes 6–11 (`T-042`, `T-043`,
//! `contracts.md` §§5.6–5.10, 8, `REL-002`) that do not require a live PostgreSQL connection.

use std::path::Path;

use chrono::{Duration, Utc};
use gtt_control_plane::db::entities::{
    AllocationPort, Protocol as DbProtocol, RelayNode, Tunnel as DbTunnel, TunnelMapping,
    UserStatus,
};
use gtt_control_plane::db::profile::ProfileSpec;
use gtt_control_plane::keys::{
    verify_allocation_token, KeyId, SigningKeyMaterial, SigningKeyRing, DEFAULT_TOKEN_ISSUER,
};
use gtt_control_plane::policy::Actor;
use gtt_control_plane::routes::tunnels::{
    assemble_tunnel_dto, assign_free_ports, build_allocation_token_claims,
    hash_create_tunnel_request, hash_start_tunnel_request, parse_create_tunnel_request,
    select_candidate_relays, validate_idempotency_key, TokenClaimsInput, MAX_IDEMPOTENCY_KEY_LEN,
    MAX_REQUEST_BODY_BYTES, MIN_IDEMPOTENCY_KEY_LEN,
};
use gtt_profiles::Registry;
use gtt_protocol::http::{
    AllocatedPortDto, AllocationDto, ApiErrorCode, CreateTunnelRequest, IssueTunnelTokenResponse,
    MappingRequest, Protocol, RelayEndpointDto, StartTunnelResponse, TunnelState,
};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn parse_create_tunnel_request_accepts_valid_body_and_enforces_schema_bounds() -> TestResult {
    let valid = r#"{"name":"Friday Terraria","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#;
    let parsed = parse_create_tunnel_request(valid, "req_valid")
        .map_err(|e| format!("expected valid request, got: {:?}", e))?;
    assert_eq!(parsed.name, "Friday Terraria");
    assert_eq!(parsed.game_profile_id, "terraria");
    assert_eq!(parsed.mappings.len(), 1);

    // Reject INV-1 local target field at top level and nested inside mappings.
    for local_payload in [
        r#"{"name":"Bad","game_profile_id":"terraria","local_port":7777,"mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#,
        r#"{"name":"Bad","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true,"target_addr":"127.0.0.1:7777"}]}"#,
    ] {
        let err = parse_create_tunnel_request(local_payload, "req_inv1")
            .err()
            .ok_or("expected INV-1 local target field rejection")?;
        assert_eq!(err.status, 400);
        assert_eq!(err.code(), ApiErrorCode::ValidationFailed);
        err.response.validate()?;
    }

    // Reject unknown field (`additionalProperties: false`), empty mappings, duplicate mapping_key,
    // control characters in name, and oversized payloads.
    for bad_payload in [
        r#"{"name":"Bad","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}],"extra":1}"#,
        r#"{"name":"Bad","game_profile_id":"terraria","mappings":[]}"#,
        r#"{"name":"Bad","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true},{"mapping_key":"game","protocol":"udp","enabled":false}]}"#,
        "{\"name\":\"Bad\\u0007Name\",\"game_profile_id\":\"terraria\",\"mappings\":[{\"mapping_key\":\"game\",\"protocol\":\"tcp\",\"enabled\":true}]}",
        r#"{"name":"   ","game_profile_id":"terraria","mappings":[{"mapping_key":"game","protocol":"tcp","enabled":true}]}"#,
    ] {
        let err = parse_create_tunnel_request(bad_payload, "req_bad")
            .err()
            .ok_or_else(|| format!("expected rejection for payload: {bad_payload}"))?;
        assert_eq!(err.status, 400);
        assert_eq!(err.code(), ApiErrorCode::ValidationFailed);
        err.response.validate()?;
    }

    let oversized = "x".repeat(MAX_REQUEST_BODY_BYTES + 1);
    let err = parse_create_tunnel_request(&oversized, "req_oversize")
        .err()
        .ok_or("expected oversize body rejection")?;
    assert_eq!(err.status, 400);
    assert_eq!(err.code(), ApiErrorCode::ValidationFailed);

    Ok(())
}

#[test]
fn idempotency_key_validation_and_request_hashing_match_contracts_section_8() -> TestResult {
    // Boundary checks on 8..=128 ASCII graphic characters.
    let min_key = "k".repeat(MIN_IDEMPOTENCY_KEY_LEN);
    let max_key = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN);
    assert!(validate_idempotency_key(&min_key).is_ok());
    assert!(validate_idempotency_key(&max_key).is_ok());

    let too_short = "k".repeat(MIN_IDEMPOTENCY_KEY_LEN - 1);
    let too_long = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
    assert!(validate_idempotency_key(&too_short).is_err());
    assert!(validate_idempotency_key(&too_long).is_err());
    assert!(validate_idempotency_key("has space in key").is_err());
    assert!(validate_idempotency_key("control\x07char").is_err());

    let req_a = CreateTunnelRequest {
        name: "Friday Terraria".to_owned(),
        game_profile_id: "terraria".to_owned(),
        mappings: vec![MappingRequest {
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            enabled: true,
        }],
    };
    let req_a_clone = req_a.clone();
    let mut req_b = req_a.clone();
    req_b.name = "Saturday Terraria".to_owned();

    let mut req_c = req_a.clone();
    req_c.mappings = vec![MappingRequest {
        mapping_key: "game".to_owned(),
        protocol: Protocol::Tcp,
        enabled: false,
    }];

    assert_eq!(
        hash_create_tunnel_request(&req_a),
        hash_create_tunnel_request(&req_a_clone),
        "identical requests must hash identically for idempotent replay"
    );
    assert_ne!(
        hash_create_tunnel_request(&req_a),
        hash_create_tunnel_request(&req_b),
        "different tunnel name must produce a different hash for idempotency_mismatch"
    );
    assert_ne!(
        hash_create_tunnel_request(&req_a),
        hash_create_tunnel_request(&req_c),
        "different mapping enabled flag must produce a different hash"
    );

    Ok(())
}

#[test]
fn profile_spec_conversion_bridges_shipped_profiles_into_control_plane() -> TestResult {
    let profiles_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    let registry = Registry::load_dir(&profiles_dir)?;
    assert_eq!(registry.len(), 5);

    let mc = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;
    let spec = ProfileSpec::from_loaded(mc);
    assert!(spec.matches("minecraft-java", 1));
    assert!(spec.mapping("game").is_some());
    assert!(spec.mapping("query").is_some());
    assert!(spec.mapping("rcon").is_some());

    // Confirm Actor::User status wiring compiles cleanly with route types.
    let _actor = Actor::User {
        id: Uuid::nil(),
        status: UserStatus::Active,
    };

    Ok(())
}

#[test]
fn select_candidate_relays_skips_unhealthy_draining_full_and_excluded_relays_and_orders_by_load(
) -> TestResult {
    let now = Utc::now();
    let make_node = |name: &str, status: &str, max_allocations: i32| RelayNode {
        id: Uuid::new_v4(),
        name: name.to_owned(),
        region: "ap-southeast-1".to_owned(),
        public_host: format!("{name}.relay.example.invalid"),
        status: status.to_owned(),
        max_allocations,
        port_range_start: 20000,
        port_range_end: 20100,
        created_at: now,
        last_health_at: Some(now),
    };

    let r_unhealthy = make_node("relay-unhealthy", "unhealthy", 10);
    let r_draining = make_node("relay-draining", "draining", 10);
    let r_provisioning = make_node("relay-provisioning", "provisioning", 10);
    let r_retired = make_node("relay-retired", "retired", 10);
    let r_full = make_node("relay-full", "healthy", 2);
    let r_busy = make_node("relay-busy", "healthy", 10);
    let r_idle = make_node("relay-idle", "healthy", 10);

    let relays = vec![
        r_unhealthy.clone(),
        r_draining.clone(),
        r_provisioning,
        r_retired,
        r_full.clone(),
        r_busy.clone(),
        r_idle.clone(),
    ];
    let loads = vec![
        (r_unhealthy.id, 0, 10),
        (r_draining.id, 0, 10),
        (r_full.id, 2, 2),
        (r_busy.id, 5, 10),
        (r_idle.id, 1, 10),
    ];

    let candidates = select_candidate_relays(&relays, &loads, None);
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        candidates.first().map(|r| r.id),
        Some(r_idle.id),
        "least-loaded healthy relay must be chosen first"
    );
    assert_eq!(candidates.get(1).map(|r| r.id), Some(r_busy.id));

    // Failover exclusion skips the failed relay and picks the remaining healthy candidate.
    let failover_candidates = select_candidate_relays(&relays, &loads, Some(r_idle.id));
    assert_eq!(failover_candidates.len(), 1);
    assert_eq!(failover_candidates.first().map(|r| r.id), Some(r_busy.id));

    Ok(())
}

#[test]
fn assign_free_ports_reserves_ports_for_enabled_mappings_and_avoids_live_collisions() -> TestResult
{
    let now = Utc::now();
    let tunnel_id = Uuid::new_v4();
    let relay_id = Uuid::new_v4();
    let m_game = TunnelMapping {
        id: Uuid::new_v4(),
        tunnel_id,
        mapping_key: "game".to_owned(),
        protocol: "tcp".to_owned(),
        position: 0,
        enabled: true,
        created_at: now,
    };
    let m_query = TunnelMapping {
        id: Uuid::new_v4(),
        tunnel_id,
        mapping_key: "query".to_owned(),
        protocol: "udp".to_owned(),
        position: 1,
        enabled: true,
        created_at: now,
    };
    let m_rcon_disabled = TunnelMapping {
        id: Uuid::new_v4(),
        tunnel_id,
        mapping_key: "rcon".to_owned(),
        protocol: "tcp".to_owned(),
        position: 2,
        enabled: false,
        created_at: now,
    };
    let mappings = vec![m_game.clone(), m_query.clone(), m_rcon_disabled];

    // Port 25000 is currently held by a live allocation; port 25001 was released (`released_at: Some`).
    let live_ports = vec![
        AllocationPort {
            allocation_id: Uuid::new_v4(),
            mapping_id: Uuid::new_v4(),
            relay_id,
            protocol: "tcp".to_owned(),
            public_port: 25000,
            released_at: None,
        },
        AllocationPort {
            allocation_id: Uuid::new_v4(),
            mapping_id: Uuid::new_v4(),
            relay_id,
            protocol: "tcp".to_owned(),
            public_port: 25001,
            released_at: Some(now),
        },
    ];

    let assigned = assign_free_ports(25000, 25005, &live_ports, &mappings)
        .ok_or("expected port assignment to succeed")?;
    assert_eq!(
        assigned.len(),
        2,
        "disabled rcon mapping must not receive a public port (GAME-008)"
    );
    let first = assigned.first().ok_or("missing first port")?;
    let second = assigned.get(1).ok_or("missing second port")?;
    assert_eq!(first.mapping_id, m_game.id);
    assert_eq!(first.protocol, DbProtocol::Tcp);
    assert_eq!(
        first.public_port, 25001,
        "live port 25000 must be skipped and released port 25001 reused"
    );
    assert_eq!(second.mapping_id, m_query.id);
    assert_eq!(second.protocol, DbProtocol::Udp);
    assert_eq!(second.public_port, 25002);

    // Exhausted TCP port range returns None.
    let exhausted_live = vec![AllocationPort {
        allocation_id: Uuid::new_v4(),
        mapping_id: Uuid::new_v4(),
        relay_id,
        protocol: "tcp".to_owned(),
        public_port: 25000,
        released_at: None,
    }];
    assert!(assign_free_ports(25000, 25000, &exhausted_live, &mappings).is_none());

    Ok(())
}

#[test]
fn build_allocation_token_claims_and_start_token_responses_validate() -> TestResult {
    let now = Utc::now();
    let exp = now + Duration::seconds(900);
    let relay_id = Uuid::new_v4();
    let tunnel_id = Uuid::new_v4();
    let other_tunnel_id = Uuid::new_v4();
    let allocation_id = Uuid::new_v4();
    let mapping_id = Uuid::new_v4();

    assert_ne!(
        hash_start_tunnel_request(tunnel_id),
        hash_start_tunnel_request(other_tunnel_id),
        "start idempotency digest must bind to the specific tunnel_id"
    );

    let mappings = vec![TunnelMapping {
        id: mapping_id,
        tunnel_id,
        mapping_key: "game".to_owned(),
        protocol: "tcp".to_owned(),
        position: 0,
        enabled: true,
        created_at: now,
    }];
    let ports = vec![AllocationPort {
        allocation_id,
        mapping_id,
        relay_id,
        protocol: "tcp".to_owned(),
        public_port: 25565,
        released_at: None,
    }];

    let claims = build_allocation_token_claims(&TokenClaimsInput {
        issuer: DEFAULT_TOKEN_ISSUER,
        relay_id,
        tunnel_id,
        allocation_id,
        mappings: &mappings,
        ports: &ports,
        now,
        exp,
        jti: Uuid::new_v4(),
    })
    .map_err(|e| format!("claims build failed: {e:?}"))?;

    let kid = KeyId::new("kid-t043-unit")?;
    let material = SigningKeyMaterial::from_bytes([0x43; 32])?;
    let key_ring = SigningKeyRing::new(DEFAULT_TOKEN_ISSUER, kid, material, now.timestamp());
    let token = key_ring.sign_allocation_token(&claims)?;

    let (verified_header, verified) = verify_allocation_token(
        &token,
        &key_ring.export_verification_keys(now.timestamp()),
        DEFAULT_TOKEN_ISSUER,
        &relay_id.to_string(),
        now.timestamp(),
    )
    .map_err(|e| format!("token verification failed: {e:?}"))?;
    assert_eq!(verified_header.kid, "kid-t043-unit");
    assert_eq!(verified.sub, tunnel_id.to_string());
    assert_eq!(verified.aid, allocation_id.to_string());

    let start_resp = StartTunnelResponse {
        tunnel_id: tunnel_id.to_string(),
        allocation_token: token.clone(),
        allocation: AllocationDto {
            id: allocation_id.to_string(),
            expires_at: exp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            relay: RelayEndpointDto {
                region: "ap-southeast-1".to_owned(),
                host: "relay.example.invalid".to_owned(),
                tunnel_port: 4433,
            },
            ports: vec![AllocatedPortDto {
                mapping_key: "game".to_owned(),
                protocol: Protocol::Tcp,
                public_port: 25565,
            }],
        },
    };
    start_resp.validate()?;

    let token_resp = IssueTunnelTokenResponse {
        allocation_token: token,
        expires_at: exp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    token_resp.validate()?;

    Ok(())
}

#[test]
fn assemble_tunnel_dto_validates_all_states_and_schema_constraints() -> TestResult {
    let now = Utc::now();
    let tunnel_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();

    let mapping = TunnelMapping {
        id: Uuid::new_v4(),
        tunnel_id,
        mapping_key: "game".to_owned(),
        protocol: "tcp".to_owned(),
        position: 0,
        enabled: true,
        created_at: now,
    };
    let mappings = vec![mapping];

    // 1. Created state
    let mut db_tunnel = DbTunnel {
        id: tunnel_id,
        owner_id,
        name: "Test Tunnel".to_owned(),
        game_profile_id: "minecraft-java".to_owned(),
        profile_version: 1,
        state: "created".to_owned(),
        created_at: now,
        started_at: None,
        stopped_at: None,
        expires_at: now + Duration::hours(24),
        termination_reason: None,
    };
    let dto = assemble_tunnel_dto(&db_tunnel, &mappings, "req_created")
        .map_err(|e| format!("expected created dto: {:?}", e))?;
    assert_eq!(dto.state, TunnelState::Created);
    assert_eq!(dto.started_at, None);
    assert_eq!(dto.stopped_at, None);
    dto.validate()?;

    // 2. Active state
    db_tunnel.state = "active".to_owned();
    db_tunnel.started_at = Some(now);
    let dto = assemble_tunnel_dto(&db_tunnel, &mappings, "req_active")
        .map_err(|e| format!("expected active dto: {:?}", e))?;
    assert_eq!(dto.state, TunnelState::Active);
    assert!(dto.started_at.is_some());
    assert_eq!(dto.stopped_at, None);
    dto.validate()?;

    // 3. Stopped state
    db_tunnel.state = "stopped".to_owned();
    db_tunnel.stopped_at = Some(now + Duration::minutes(30));
    let dto = assemble_tunnel_dto(&db_tunnel, &mappings, "req_stopped")
        .map_err(|e| format!("expected stopped dto: {:?}", e))?;
    assert_eq!(dto.state, TunnelState::Stopped);
    assert!(dto.stopped_at.is_some());
    dto.validate()?;

    // 4. Terminated state with reason
    db_tunnel.state = "terminated".to_owned();
    db_tunnel.termination_reason = Some("quota_exceeded".to_owned());
    let dto = assemble_tunnel_dto(&db_tunnel, &mappings, "req_term")
        .map_err(|e| format!("expected terminated dto: {:?}", e))?;
    assert_eq!(dto.state, TunnelState::Terminated);
    assert_eq!(dto.termination_reason.as_deref(), Some("quota_exceeded"));
    dto.validate()?;

    // 5. Expired state
    db_tunnel.state = "expired".to_owned();
    db_tunnel.termination_reason = None;
    let dto = assemble_tunnel_dto(&db_tunnel, &mappings, "req_expired")
        .map_err(|e| format!("expected expired dto: {:?}", e))?;
    assert_eq!(dto.state, TunnelState::Expired);
    dto.validate()?;

    // 6. Unknown / invalid database state returns Internal error
    db_tunnel.state = "invalid_state".to_owned();
    let err = assemble_tunnel_dto(&db_tunnel, &mappings, "req_inv_state")
        .err()
        .ok_or("expected rejection of invalid state")?;
    assert_eq!(err.code(), ApiErrorCode::Internal);

    // 7. Invalid mapping protocol returns Internal error
    db_tunnel.state = "created".to_owned();
    let bad_mapping = TunnelMapping {
        id: Uuid::new_v4(),
        tunnel_id,
        mapping_key: "game".to_owned(),
        protocol: "sctp".to_owned(),
        position: 0,
        enabled: true,
        created_at: now,
    };
    let err = assemble_tunnel_dto(&db_tunnel, &[bad_mapping], "req_bad_proto")
        .err()
        .ok_or("expected rejection of invalid mapping protocol")?;
    assert_eq!(err.code(), ApiErrorCode::Internal);

    Ok(())
}
