-- T-049: Admin identity and sessions (ADR-019, SEC-018).
-- Admin is a separate identity, not a role on users.
CREATE TABLE admins (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text NOT NULL UNIQUE,
    display_name text NOT NULL,
    passkey_credential_id bytea NOT NULL,
    passkey_public_key bytea NOT NULL,
    passkey_counter bigint NOT NULL DEFAULT 0,
    recovery_code_hash text NOT NULL,
    recovery_used_at timestamptz NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE admin_sessions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    token_hash text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    step_up_until timestamptz NULL,
    revoked_at timestamptz NULL
);

CREATE INDEX admin_sessions_token_hash_idx ON admin_sessions(token_hash);
CREATE INDEX admin_sessions_admin_id_idx ON admin_sessions(admin_id);
