-- Events (metadata survives retention; only raw references are cleared),
-- attachments, and searchable log lines.

CREATE TABLE events (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    issue_id uuid REFERENCES issues (id) ON DELETE SET NULL,
    sentry_event_id text NOT NULL,
    received_at timestamptz NOT NULL,
    raw_object_key text,
    raw_size bigint NOT NULL DEFAULT 0 CHECK (raw_size >= 0),
    raw_expired_at timestamptz,
    attachment_count integer NOT NULL DEFAULT 0 CHECK (attachment_count >= 0),
    UNIQUE (project_id, sentry_event_id)
);

CREATE INDEX events_received_idx ON events (project_id, received_at DESC);
CREATE INDEX events_issue_idx ON events (issue_id);

ALTER TABLE issues
    ADD CONSTRAINT issues_last_event_fk
    FOREIGN KEY (last_event_id) REFERENCES events (id) ON DELETE SET NULL;

CREATE TABLE attachments (
    id uuid PRIMARY KEY,
    event_id uuid NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    filename text NOT NULL,
    content_type text NOT NULL,
    size bigint NOT NULL CHECK (size BETWEEN 0 AND 10485760),
    object_key text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX attachments_event_idx ON attachments (event_id);

CREATE TABLE log_lines (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    timestamp timestamptz NOT NULL,
    level text NOT NULL DEFAULT 'info',
    message text NOT NULL,
    source text NOT NULL DEFAULT 'ingest',
    attributes jsonb NOT NULL DEFAULT '{}'::jsonb,
    received_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX log_lines_time_idx ON log_lines (project_id, timestamp DESC);
CREATE INDEX log_lines_attributes_idx ON log_lines USING gin (attributes);
