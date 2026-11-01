-- 0008: production readiness.
--
-- Additive only. Every statement is idempotent so re-running the file by hand is
-- safe, and the migrator still records it exactly once.
--
-- Contents:
--   1. Demo confinement marker (FR-011, SC-014).
--   2. Account lockout state (FR-012, FR-032, SC-023).
--   3. Human-readable actor labels for the audit log and issue decisions.
--   4. Optional per-organisation OIDC connections and in-progress login flows.
--   5. Trigram indexes for the unified search view (FR-021).

-- 1. Demo organisations are marked so demo sessions can be scoped away from real
--    tenants and demo resets cannot touch real data.
ALTER TABLE organisations ADD COLUMN IF NOT EXISTS is_demo boolean NOT NULL DEFAULT false;

-- 2. Five consecutive failures lock an account regardless of elapsed time, so
--    only a counter and the lock are stored. The counter resets on a successful
--    sign-in or an operator unlock.
ALTER TABLE users ADD COLUMN IF NOT EXISTS locked_at timestamptz;
ALTER TABLE users ADD COLUMN IF NOT EXISTS failed_sign_in_count integer NOT NULL DEFAULT 0
    CHECK (failed_sign_in_count >= 0);

-- 3. Actor labels survive user removal, unlike a foreign key alone.
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_label text NOT NULL DEFAULT '';
ALTER TABLE issues ADD COLUMN IF NOT EXISTS decision_by_label text;

-- 4. Optional OIDC per organisation. The client secret is replayable to the
--    provider, so it cannot be hashed; it is never serialized or logged.
CREATE TABLE IF NOT EXISTS oidc_connections (
    org_id        uuid PRIMARY KEY REFERENCES organisations (id) ON DELETE CASCADE,
    issuer        text NOT NULL
        CHECK (issuer LIKE 'https://%' OR issuer LIKE 'http://%'),
    client_id     text NOT NULL CHECK (client_id <> ''),
    client_secret text NOT NULL CHECK (client_secret <> ''),
    scopes        text NOT NULL DEFAULT 'openid email profile',
    created_by    uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS oidc_login_flows (
    state         text PRIMARY KEY,
    org_id        uuid NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    nonce         text NOT NULL,
    pkce_verifier text NOT NULL,
    return_to     text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS oidc_login_flows_expiry_idx ON oidc_login_flows (expires_at);

-- 4. Case-insensitive containment search.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX IF NOT EXISTS issues_title_trgm_idx ON issues USING gin (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS log_lines_message_trgm_idx ON log_lines USING gin (message gin_trgm_ops);
