//! Unit and security tests for local target policy and dial target resolution (`T-071`, `FR-003`,
//! `SEC-006`, `GAME-008`, `contracts.md` §§11, 13).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use gtt_engine::{
    configure_mappings, is_sensitive_local_port, validate_mapping_target, DialRefusalReason,
    MappingTargetInput, TargetPolicyViolation, SENSITIVE_LOCAL_PORTS,
};
use gtt_profiles::{Evidence, Mapping, Profile, Protocol as ProfileProtocol, Registry};
use gtt_protocol::{HelloAckMapping, Protocol, TcpOpenStatus};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("profiles")
}

#[test]
fn loopback_and_confirmed_lan_targets_succeed_while_unconfirmed_non_loopback_is_rejected(
) -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    // 1. Loopback IPv4 (127.0.0.1), IPv6 (::1), and "localhost" succeed without confirmation.
    for host in ["127.0.0.1", "127.0.0.42", "::1", "localhost", "LOCALHOST"] {
        let input = MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: host.to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        };
        let validated = validate_mapping_target(mc_java, &input)
            .map_err(|e| format!("expected {host} to succeed, got {e}"))?;
        assert!(validated.local_addr().ip().is_loopback());
        assert_eq!(validated.local_addr().port(), 25565);
    }

    // 2. Non-loopback unicast (LAN IPv4, IPv4-mapped IPv6, and IPv6 ULA) without confirmation is
    //    rejected with NonLoopbackRequiresConfirmation (`contracts.md` §13).
    for host in [
        "192.168.1.50",
        "10.0.0.12",
        "::ffff:192.168.1.50",
        "fd00::1234",
    ] {
        let unconfirmed = MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: host.to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        };
        assert_eq!(
            validate_mapping_target(mc_java, &unconfirmed),
            Err(TargetPolicyViolation::NonLoopbackRequiresConfirmation),
            "host {host} must require explicit non-loopback confirmation"
        );

        // With explicit confirmation, the same non-loopback unicast address is permitted.
        let confirmed = MappingTargetInput {
            confirm_non_loopback: true,
            ..unconfirmed
        };
        let validated = validate_mapping_target(mc_java, &confirmed)
            .map_err(|e| format!("expected confirmed {host} to succeed, got {e}"))?;
        assert_eq!(validated.local_addr().port(), 25565);
    }

    Ok(())
}

#[test]
fn unspecified_multicast_broadcast_and_link_local_addresses_are_always_rejected() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    let forbidden_cases: &[(&str, &str)] = &[
        ("0.0.0.0", "unspecified"),
        ("::", "unspecified"),
        ("::ffff:0.0.0.0", "unspecified"),
        ("224.0.0.1", "multicast"),
        ("239.255.255.250", "multicast"),
        ("ff02::1", "multicast"),
        ("255.255.255.255", "broadcast"),
        ("::ffff:255.255.255.255", "broadcast"),
        ("169.254.1.10", "link_local"),
        ("169.254.254.254", "link_local"),
        ("::ffff:169.254.10.20", "link_local"),
        ("fe80::1", "link_local"),
        ("febf::abcd", "link_local"),
    ];

    for &(host, expected_category) in forbidden_cases {
        let input = MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: host.to_owned(),
            local_port: 25565,
            enabled: true,
            // Even if confirm_non_loopback is true, these categories must be rejected (`contracts.md` §13).
            confirm_non_loopback: true,
        };
        assert_eq!(
            validate_mapping_target(mc_java, &input),
            Err(TargetPolicyViolation::ForbiddenAddressCategory {
                category: expected_category
            }),
            "expected {host} to be rejected as {expected_category}"
        );
    }

    // Invalid host strings and port 0 are also rejected.
    let bad_host = MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "not-an-ip.example.com".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: true,
    };
    assert_eq!(
        validate_mapping_target(mc_java, &bad_host),
        Err(TargetPolicyViolation::InvalidHostAddress)
    );

    let zero_port = MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 0,
        enabled: true,
        confirm_non_loopback: false,
    };
    assert_eq!(
        validate_mapping_target(mc_java, &zero_port),
        Err(TargetPolicyViolation::InvalidPortZero)
    );

    Ok(())
}

#[test]
fn sensitive_port_deny_list_rejects_sensitive_services_unless_explicitly_allowed_by_profile(
) -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    // Every port on SENSITIVE_LOCAL_PORTS is rejected for Minecraft Java (`default_port` 25565).
    for &port in SENSITIVE_LOCAL_PORTS {
        assert!(is_sensitive_local_port(port));
        let input = MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: port,
            enabled: true,
            confirm_non_loopback: false,
        };
        assert_eq!(
            validate_mapping_target(mc_java, &input),
            Err(TargetPolicyViolation::SensitivePortDenied { port }),
            "expected sensitive port {port} to be denied"
        );
    }

    // Custom non-sensitive port (e.g., 25566 or 30000) is allowed (`GAME-009`).
    let custom_port = MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 25566,
        enabled: true,
        confirm_non_loopback: false,
    };
    assert!(validate_mapping_target(mc_java, &custom_port).is_ok());

    // If a profile explicitly declares a port from the deny-list as its `default_port`, that exact
    // port is permitted for that mapping (`contracts.md` §13).
    let custom_profile = Profile {
        id: "custom-web-game".to_owned(),
        version: 1,
        display_name: "Custom Web Game".to_owned(),
        mappings: vec![Mapping {
            key: "game".to_owned(),
            protocol: ProfileProtocol::Tcp,
            default_port: 443,
            optional: false,
            evidence: Evidence::Verified,
        }],
    };
    let allowed_by_profile = MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "127.0.0.1".to_owned(),
        local_port: 443,
        enabled: true,
        confirm_non_loopback: false,
    };
    assert!(validate_mapping_target(&custom_profile, &allowed_by_profile).is_ok());

    // Other sensitive ports (e.g. SSH 22) remain blocked on that same profile.
    let still_denied = MappingTargetInput {
        local_port: 22,
        ..allowed_by_profile
    };
    assert_eq!(
        validate_mapping_target(&custom_profile, &still_denied),
        Err(TargetPolicyViolation::SensitivePortDenied { port: 22 })
    );

    Ok(())
}

#[test]
fn relay_messages_cannot_change_local_targets_or_enable_disabled_mappings_sec_006() -> TestResult {
    let registry = Registry::load_dir(&repo_profiles_dir())?;
    let mc_java = registry
        .get("minecraft-java")
        .ok_or("missing minecraft-java profile")?;

    // Host configures `game` (enabled on 127.0.0.1:25565) and `rcon` (disabled on 127.0.0.1:25575,
    // `GAME-008`).
    let inputs = vec![
        MappingTargetInput {
            mapping_key: "game".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25565,
            enabled: true,
            confirm_non_loopback: false,
        },
        MappingTargetInput {
            mapping_key: "rcon".to_owned(),
            local_host: "127.0.0.1".to_owned(),
            local_port: 25575,
            enabled: false,
            confirm_non_loopback: false,
        },
    ];

    let mut table =
        configure_mappings(mc_java, &inputs).map_err(|e| format!("unexpected failure: {e:?}"))?;

    let expected_game_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565);
    let expected_rcon_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25575);

    // Relay sends `hello_ack` assigning wire index 3 to `game` (with public_port 41234), wire
    // index 4 to disabled `rcon`, and wire index 5 to an unconfigured `query` mapping.
    let relay_ack_mappings = vec![
        HelloAckMapping {
            index: 3,
            mapping_key: "game".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 41234,
        },
        HelloAckMapping {
            index: 4,
            mapping_key: "rcon".to_owned(),
            protocol: Protocol::Tcp,
            public_port: 41235,
        },
        HelloAckMapping {
            index: 5,
            mapping_key: "query".to_owned(),
            protocol: Protocol::Udp,
            public_port: 41236,
        },
    ];
    table.bind_hello_ack_mappings(&relay_ack_mappings);

    // 1. Local target addresses remain strictly equal to the host's configuration; `public_port`
    //    41234 from the relay never overwrites local_port 25565 (`SEC-006`).
    assert_eq!(
        table.resolve_dial_target(3, Protocol::Tcp),
        Ok(expected_game_addr)
    );
    assert_eq!(
        table.get_by_key("rcon").ok_or("missing rcon")?.local_addr(),
        expected_rcon_addr
    );

    // 2. Wrong protocol on wire index 3 is refused.
    assert_eq!(
        table.resolve_dial_target(3, Protocol::Udp),
        Err(DialRefusalReason::ProtocolMismatch { mapping_index: 3 })
    );

    // 3. Disabled mapping `rcon` (index 4) and unconfigured mapping `query` (index 5) and unknown
    //    index 99 are all refused with TcpOpenStatus::Refused (`0x01`, `SEC-006`, `GAME-002`).
    for refused_idx in [0_u16, 4, 5, 99] {
        let err = table
            .resolve_dial_target(refused_idx, Protocol::Tcp)
            .err()
            .ok_or("expected dial refusal")?;
        assert_eq!(err.to_tcp_open_status(), TcpOpenStatus::Refused);
    }

    // 4. Verify ipv6 loopback works identically.
    let v6_inputs = vec![MappingTargetInput {
        mapping_key: "game".to_owned(),
        local_host: "::1".to_owned(),
        local_port: 25565,
        enabled: true,
        confirm_non_loopback: false,
    }];
    let v6_table = configure_mappings(mc_java, &v6_inputs)
        .map_err(|e| format!("unexpected v6 failure: {e:?}"))?;
    assert_eq!(
        v6_table.resolve_dial_target(0, Protocol::Tcp),
        Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 25565))
    );

    Ok(())
}
