-- schema-draft.sql
-- DRAFT physical schema for the control-plane database (PostgreSQL).
-- Source of design truth: data-model.md. Once migrations/ exists, migrations are authoritative
-- for the physical schema and this file is retired (see schema.md).
--
-- VERIFICATION STATUS 2026-10-17: applied successfully to an EMPTY database on
-- PostgreSQL 16.10 and 17.6, producing 12 tables, 35 indexes, 57 constraints and
-- 13 foreign keys on both, with zero errors. See schema.md for the version matrix
-- and the exact method. It was NOT applied under a normal client connection: forked
-- backends crash in the authoring environment with STATUS_DLL_INIT_FAILED, so
-- `postgres --single` was used, which does not fork.
-- Numeric bounds marked "proposal" are still not validated values. The four
-- "proposal"-bounded CHECK constraints have now been exercised by negative tests
-- and are enforced, but the bounds themselves remain unvalidated.

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    idp_issuer    text NOT NULL,
    idp_subject   text NOT NULL,
    display_name  text,
    status        text NOT NULL DEFAULT 'active',
    created_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    CONSTRAINT users_status_chk CHECK (status IN ('active', 'suspended')),
    CONSTRAINT users_display_name_len_chk CHECK (display_name IS NULL OR char_length(display_name) BETWEEN 1 AND 64),
    CONSTRAINT users_idp_uniq UNIQUE (idp_issuer, idp_subject)
);

CREATE TABLE auth_sessions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id             uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    family_id           uuid NOT NULL,
    refresh_token_hash  bytea NOT NULL,
    device_label        text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    last_used_at        timestamptz,
    expires_at          timestamptz NOT NULL,
    revoked_at          timestamptz,
    CONSTRAINT auth_sessions_token_uniq UNIQUE (refresh_token_hash),
    CONSTRAINT auth_sessions_expiry_chk CHECK (expires_at > created_at),
    CONSTRAINT auth_sessions_label_len_chk CHECK (device_label IS NULL OR char_length(device_label) <= 64)
);
CREATE INDEX auth_sessions_user_active_idx ON auth_sessions (user_id) WHERE revoked_at IS NULL;
CREATE INDEX auth_sessions_family_idx ON auth_sessions (family_id);

CREATE TABLE tunnels (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id            uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name                text NOT NULL,
    game_profile_id     text NOT NULL,
    profile_version     integer NOT NULL,
    state               text NOT NULL DEFAULT 'created',
    created_at          timestamptz NOT NULL DEFAULT now(),
    started_at          timestamptz,
    stopped_at          timestamptz,
    expires_at          timestamptz NOT NULL,
    termination_reason  text,
    CONSTRAINT tunnels_state_chk CHECK (state IN ('created', 'active', 'stopped', 'terminated', 'expired')),
    CONSTRAINT tunnels_name_len_chk CHECK (char_length(name) BETWEEN 1 AND 64),
    CONSTRAINT tunnels_profile_version_chk CHECK (profile_version >= 1),
    CONSTRAINT tunnels_stopped_consistency_chk CHECK (
        (state IN ('stopped', 'terminated', 'expired')) = (stopped_at IS NOT NULL)
    ),
    CONSTRAINT tunnels_expiry_chk CHECK (expires_at > created_at)
);
CREATE INDEX tunnels_owner_state_idx ON tunnels (owner_id, state);
CREATE INDEX tunnels_active_expiry_idx ON tunnels (expires_at) WHERE state = 'active';

-- NOTE: no local target address or port is stored anywhere in the control plane (ADR-003).
CREATE TABLE tunnel_mappings (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tunnel_id   uuid NOT NULL REFERENCES tunnels (id) ON DELETE CASCADE,
    mapping_key text NOT NULL,
    protocol    text NOT NULL,
    position    smallint NOT NULL,
    enabled     boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT tunnel_mappings_protocol_chk CHECK (protocol IN ('tcp', 'udp')),
    CONSTRAINT tunnel_mappings_position_chk CHECK (position >= 0),
    CONSTRAINT tunnel_mappings_key_uniq UNIQUE (tunnel_id, mapping_key),
    CONSTRAINT tunnel_mappings_position_uniq UNIQUE (tunnel_id, position),
    CONSTRAINT tunnel_mappings_id_protocol_uniq UNIQUE (id, protocol)
);

CREATE TABLE relay_nodes (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name              text NOT NULL,
    region            text NOT NULL,
    public_host       text NOT NULL,
    status            text NOT NULL DEFAULT 'provisioning',
    max_allocations   integer NOT NULL,
    port_range_start  integer NOT NULL,
    port_range_end    integer NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    last_health_at    timestamptz,
    CONSTRAINT relay_nodes_name_uniq UNIQUE (name),
    CONSTRAINT relay_nodes_status_chk CHECK (status IN ('provisioning', 'healthy', 'draining', 'unhealthy', 'retired')),
    CONSTRAINT relay_nodes_capacity_chk CHECK (max_allocations > 0),
    CONSTRAINT relay_nodes_ports_chk CHECK (
        port_range_start BETWEEN 1024 AND 65535
        AND port_range_end BETWEEN 1024 AND 65535
        AND port_range_start <= port_range_end
    )
);

CREATE TABLE relay_allocations (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tunnel_id    uuid NOT NULL REFERENCES tunnels (id) ON DELETE CASCADE,
    relay_id     uuid NOT NULL REFERENCES relay_nodes (id) ON DELETE RESTRICT,
    state        text NOT NULL DEFAULT 'reserved',
    issued_at    timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    released_at  timestamptz,
    CONSTRAINT relay_allocations_state_chk CHECK (state IN ('reserved', 'active', 'released', 'expired')),
    CONSTRAINT relay_allocations_expiry_chk CHECK (expires_at > issued_at),
    CONSTRAINT relay_allocations_id_relay_uniq UNIQUE (id, relay_id)
);
-- At most one live allocation per tunnel.
CREATE UNIQUE INDEX relay_allocations_one_live_idx ON relay_allocations (tunnel_id) WHERE state IN ('reserved', 'active');
CREATE INDEX relay_allocations_relay_live_idx ON relay_allocations (relay_id) WHERE state IN ('reserved', 'active');

CREATE TABLE allocation_ports (
    allocation_id  uuid NOT NULL,
    mapping_id     uuid NOT NULL,
    relay_id       uuid NOT NULL,
    protocol       text NOT NULL,
    public_port    integer NOT NULL,
    released_at    timestamptz,
    PRIMARY KEY (allocation_id, mapping_id),
    CONSTRAINT allocation_ports_alloc_fk FOREIGN KEY (allocation_id, relay_id)
        REFERENCES relay_allocations (id, relay_id) ON DELETE CASCADE,
    CONSTRAINT allocation_ports_mapping_fk FOREIGN KEY (mapping_id, protocol)
        REFERENCES tunnel_mappings (id, protocol) ON DELETE CASCADE,
    CONSTRAINT allocation_ports_port_chk CHECK (public_port BETWEEN 1024 AND 65535)
);
-- A public (relay, protocol, port) can be held by only one unreleased mapping.
CREATE UNIQUE INDEX allocation_ports_live_port_idx ON allocation_ports (relay_id, protocol, public_port) WHERE released_at IS NULL;

CREATE TABLE invitations (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tunnel_id   uuid NOT NULL REFERENCES tunnels (id) ON DELETE CASCADE,
    created_by  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash  bytea NOT NULL,
    max_claims  integer NOT NULL DEFAULT 1,
    claim_count integer NOT NULL DEFAULT 0,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    CONSTRAINT invitations_token_uniq UNIQUE (token_hash),
    CONSTRAINT invitations_claims_chk CHECK (max_claims BETWEEN 1 AND 50 AND claim_count >= 0 AND claim_count <= max_claims), -- bounds: proposal
    CONSTRAINT invitations_expiry_chk CHECK (expires_at > created_at),
    CONSTRAINT invitations_id_tunnel_uniq UNIQUE (id, tunnel_id)
);
CREATE INDEX invitations_tunnel_idx ON invitations (tunnel_id);

CREATE TABLE guest_admissions (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    invitation_id  uuid NOT NULL,
    tunnel_id      uuid NOT NULL,
    source_ip      inet NOT NULL,   -- sensitive; short retention
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL,
    revoked_at     timestamptz,
    CONSTRAINT guest_admissions_invitation_fk FOREIGN KEY (invitation_id, tunnel_id)
        REFERENCES invitations (id, tunnel_id) ON DELETE CASCADE,
    CONSTRAINT guest_admissions_expiry_chk CHECK (expires_at > created_at)
);
CREATE UNIQUE INDEX guest_admissions_live_ip_idx ON guest_admissions (tunnel_id, source_ip) WHERE revoked_at IS NULL;
CREATE INDEX guest_admissions_expiry_idx ON guest_admissions (expires_at);

CREATE TABLE usage_records (
    tunnel_id           uuid NOT NULL REFERENCES tunnels (id) ON DELETE CASCADE,
    relay_id            uuid NOT NULL REFERENCES relay_nodes (id) ON DELETE RESTRICT,
    interval_start      timestamptz NOT NULL,
    bytes_from_guests   bigint NOT NULL DEFAULT 0,
    bytes_to_guests     bigint NOT NULL DEFAULT 0,
    peak_flows          integer NOT NULL DEFAULT 0,
    datagrams_dropped   bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (tunnel_id, relay_id, interval_start),
    CONSTRAINT usage_records_nonneg_chk CHECK (
        bytes_from_guests >= 0 AND bytes_to_guests >= 0 AND peak_flows >= 0 AND datagrams_dropped >= 0
    )
);
CREATE INDEX usage_records_interval_idx ON usage_records (interval_start);

-- Deliberately no foreign keys: audit history must outlive the rows it describes.
CREATE TABLE audit_events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at   timestamptz NOT NULL DEFAULT now(),
    actor_type    text NOT NULL,
    actor_id      uuid,
    action        text NOT NULL,
    subject_type  text NOT NULL,
    subject_id    uuid,
    request_id    text,
    metadata      jsonb NOT NULL DEFAULT '{}'::jsonb,   -- never tokens, payloads, or raw guest IPs
    CONSTRAINT audit_events_actor_type_chk CHECK (actor_type IN ('user', 'admin', 'system', 'relay'))
);
CREATE INDEX audit_events_time_idx ON audit_events (occurred_at);
CREATE INDEX audit_events_subject_idx ON audit_events (subject_type, subject_id);

CREATE TABLE idempotency_keys (
    user_id          uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    route            text NOT NULL,
    idem_key         text NOT NULL,
    request_hash     bytea NOT NULL,
    response_status  integer,
    response_body    jsonb,
    created_at       timestamptz NOT NULL DEFAULT now(),
    expires_at       timestamptz NOT NULL,
    PRIMARY KEY (user_id, route, idem_key),
    CONSTRAINT idempotency_keys_len_chk CHECK (char_length(idem_key) BETWEEN 8 AND 128)
);
CREATE INDEX idempotency_keys_expiry_idx ON idempotency_keys (expires_at);
