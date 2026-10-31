-- Grouped issues and their append-only state history.
-- events.last_event_id is wired up in 0005 once the events table exists.

CREATE TABLE issues (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    fingerprint text NOT NULL,
    title text NOT NULL,
    culprit text,
    level text NOT NULL DEFAULT 'error',
    first_seen timestamptz NOT NULL,
    last_seen timestamptz NOT NULL,
    count bigint NOT NULL DEFAULT 1 CHECK (count >= 1),
    state text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'resolved', 'ignored')),
    release text,
    environment text,
    tags jsonb NOT NULL DEFAULT '{}'::jsonb,
    frames jsonb NOT NULL DEFAULT '[]'::jsonb,
    breadcrumbs jsonb NOT NULL DEFAULT '[]'::jsonb,
    suggestion_category text
        CHECK (suggestion_category IS NULL OR suggestion_category IN ('issue', 'log', 'raw_event')),
    suggestion_confidence double precision
        CHECK (suggestion_confidence IS NULL OR suggestion_confidence BETWEEN 0 AND 1),
    suggestion_source text,
    suggestion_at timestamptz,
    decision_category text
        CHECK (decision_category IS NULL OR decision_category IN ('issue', 'log', 'raw_event')),
    decision_by uuid REFERENCES users (id),
    decision_at timestamptz,
    decision_overridden boolean NOT NULL DEFAULT false,
    first_presented_at timestamptz,
    last_event_id uuid,
    UNIQUE (project_id, fingerprint)
);

CREATE INDEX issues_state_idx ON issues (project_id, state, last_seen DESC);
CREATE INDEX issues_decision_idx ON issues (project_id, decision_category, last_seen DESC);
CREATE INDEX issues_count_idx ON issues (project_id, count DESC);
CREATE INDEX issues_tags_idx ON issues USING gin (tags);

CREATE TABLE issue_state_changes (
    id uuid PRIMARY KEY,
    issue_id uuid NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    state text NOT NULL CHECK (state IN ('open', 'resolved', 'ignored')),
    actor_user_id uuid REFERENCES users (id),
    actor_label text NOT NULL,
    at timestamptz NOT NULL
);

CREATE INDEX issue_state_changes_issue_idx ON issue_state_changes (issue_id, at DESC);
