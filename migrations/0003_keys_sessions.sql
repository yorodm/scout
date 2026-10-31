-- Ingest keys, account sessions, recovery codes, and one-time invitations.
-- Only hashes are stored; plaintext tokens are shown once at creation.

CREATE TABLE ingest_keys (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    label text NOT NULL,
    key_hash text NOT NULL UNIQUE,
    key_prefix text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    last_used_at timestamptz
);

CREATE INDEX ingest_keys_project_idx ON ingest_keys (project_id) WHERE revoked_at IS NULL;

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL DEFAULT now() + interval '7 days',
    last_seen_at timestamptz
);

CREATE INDEX sessions_user_idx ON sessions (user_id);
CREATE INDEX sessions_expiry_idx ON sessions (expires_at);

CREATE TABLE recovery_codes (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at timestamptz,
    revoked_at timestamptz
);

CREATE INDEX recovery_codes_user_idx ON recovery_codes (user_id)
    WHERE used_at IS NULL AND revoked_at IS NULL;

CREATE TABLE invitations (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    kind text NOT NULL DEFAULT 'project_invite'
        CHECK (kind IN ('project_invite', 'org_admin_setup')),
    project_id uuid REFERENCES projects (id) ON DELETE CASCADE,
    email citext NOT NULL,
    role text CHECK (role IS NULL OR role IN ('owner', 'admin', 'triager', 'viewer')),
    token_hash text NOT NULL UNIQUE,
    created_by uuid NOT NULL REFERENCES users (id),
    expires_at timestamptz NOT NULL DEFAULT now() + interval '7 days',
    accepted_at timestamptz,
    accepted_user_id uuid REFERENCES users (id),
    revoked_at timestamptz,
    CHECK ((kind = 'project_invite' AND project_id IS NOT NULL AND role IS NOT NULL)
        OR (kind = 'org_admin_setup' AND project_id IS NULL AND role IS NULL))
);

CREATE INDEX invitations_org_idx ON invitations (org_id);
