-- 0007: Sentry read-API compatibility support.
--
-- Adds the organization slug, per-project issue numbers, and API tokens. Run this
-- file inside a single transaction (sqlx's migrator does); every backfill
-- completes before its unique index so existing rows cannot collide on a default.

-- 1. Organisation slugs.
ALTER TABLE organisations ADD COLUMN IF NOT EXISTS slug TEXT;

UPDATE organisations AS o
   SET slug = s.slug
  FROM (
      SELECT id,
             CASE WHEN rn = 1 THEN base ELSE base || '-' || rn END AS slug
        FROM (
            SELECT id,
                   COALESCE(
                       NULLIF(
                           trim(both '-' from
                               lower(regexp_replace(name, '[^a-zA-Z0-9]+', '-', 'g'))
                           ),
                           ''
                       ),
                       'org'
                   ) AS base,
                   row_number() OVER (
                       PARTITION BY COALESCE(
                           NULLIF(
                               trim(both '-' from
                                   lower(regexp_replace(name, '[^a-zA-Z0-9]+', '-', 'g'))
                               ),
                               ''
                           ),
                           'org'
                       )
                       ORDER BY created_at, id
                   ) AS rn
              FROM organisations
             WHERE slug IS NULL
        ) numbered
  ) AS s
 WHERE o.id = s.id;

CREATE UNIQUE INDEX IF NOT EXISTS organisations_slug_key ON organisations (lower(slug));

-- 2. Issue numbers, unique per project.
ALTER TABLE issues ADD COLUMN IF NOT EXISTS number BIGINT;

UPDATE issues AS i
   SET number = n.rn
  FROM (
      SELECT id,
             row_number() OVER (PARTITION BY project_id ORDER BY first_seen, id) AS rn
        FROM issues
       WHERE number IS NULL
  ) AS n
 WHERE i.id = n.id;

ALTER TABLE issues ALTER COLUMN number SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS issues_project_number_key ON issues (project_id, number);

-- 3. API tokens for the compatibility surface.
CREATE TABLE IF NOT EXISTS api_tokens (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    label        TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS api_tokens_user_id_idx ON api_tokens (user_id);
