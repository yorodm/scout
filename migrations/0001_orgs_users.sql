-- Organisations, local accounts, and organisation membership.
-- The deployment operator is a normal users row with is_operator = true.

CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id uuid PRIMARY KEY,
    email citext NOT NULL UNIQUE,
    name text NOT NULL,
    password_hash text,
    is_operator boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    disabled_at timestamptz
);

-- At most one deployment operator may exist.
CREATE UNIQUE INDEX users_single_operator
    ON users (is_operator)
    WHERE is_operator;

CREATE TABLE organisations (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    sso_provider text,
    sso_config jsonb,
    created_by uuid REFERENCES users (id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE org_members (
    org_id uuid NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('org_admin', 'org_member')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

CREATE INDEX org_members_user_idx ON org_members (user_id);
