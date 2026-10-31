-- Projects and project-scoped roles.
-- Soft-delete keeps every row and raw object; only deleted_at/deleted_by change.

CREATE TABLE projects (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    slug text NOT NULL,
    name text NOT NULL,
    dsn_id text NOT NULL UNIQUE,
    retention_days integer NOT NULL DEFAULT 30
        CHECK (retention_days BETWEEN 1 AND 3650),
    model_endpoint text
        CHECK (model_endpoint IS NULL
               OR model_endpoint LIKE 'http://%'
               OR model_endpoint LIKE 'https://%'),
    model_name text,
    model_api_key_ref text,
    confidence_threshold double precision NOT NULL DEFAULT 0.5
        CHECK (confidence_threshold BETWEEN 0 AND 1),
    deleted_at timestamptz,
    deleted_by uuid REFERENCES users (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (model_endpoint IS NULL OR (model_name IS NOT NULL AND model_name <> '')),
    UNIQUE (org_id, slug)
);

CREATE INDEX projects_active_idx ON projects (org_id) WHERE deleted_at IS NULL;

CREATE TABLE project_members (
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('owner', 'admin', 'triager', 'viewer')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX project_members_user_idx ON project_members (user_id);
