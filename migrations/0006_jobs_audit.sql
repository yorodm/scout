-- Durable job queue (claimed with FOR UPDATE SKIP LOCKED) and the audit log.
-- audit_log.org_id is nullable for deployment-level actions such as the
-- operator bootstrap.

CREATE TABLE jobs (
    id uuid PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('group_event', 'classify_event', 'reconcile')),
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    event_id uuid REFERENCES events (id) ON DELETE CASCADE,
    state text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at timestamptz NOT NULL DEFAULT now(),
    locked_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX jobs_claim_idx ON jobs (state, available_at);
CREATE INDEX jobs_project_idx ON jobs (project_id);

CREATE TABLE audit_log (
    id uuid PRIMARY KEY,
    org_id uuid REFERENCES organisations (id) ON DELETE SET NULL,
    project_id uuid,
    actor_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    action text NOT NULL,
    target text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_log_org_idx ON audit_log (org_id, at DESC);
CREATE INDEX audit_log_project_idx ON audit_log (project_id, at DESC);
