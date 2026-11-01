# Phase 1 Data Model: Production Readiness

This document records only what this feature adds or must change. The authoritative product model
remains [feature 001's data model](../../001-error-tracking-log-ingestion/data-model.md); the storage
mapping below states how each existing entity becomes durable and where the schema and the Rust structs
currently disagree.

## 1. Storage mapping

| Model (`src/models.rs`) | Table | Notes |
|---|---|---|
| `Organisation` | `organisations` | `slug` from `0007`; `is_demo` added in `0008`; `sso_provider`/`sso_config` retained for the legacy field, OIDC uses `oidc_connections` |
| `OrgRole`, `Role` | `org_members.role`, `project_members.role` | Enum ⇄ `text` with checked values |
| `Project` (+`ModelConfig`) | `projects` | `model.endpoint` → `model_endpoint`, `model.model` → `model_name`, `model.api_key` → `model_api_key_ref` (replayable secret: never serialized, never logged, never returned; at-rest exposure recorded in the plan's Complexity Tracking), `model.confidence_threshold` → `confidence_threshold` |
| `IngestKey` | `ingest_keys` | `revoked` ⇄ `revoked_at IS NOT NULL`; `last_used_at` written on successful ingest-key lookup |
| `User` | `users` | `is_operator` unique partial index already enforces one operator; `locked_at` and `failed_sign_in_count` (added in `0008`) hold the lockout state |
| `Session` | `sessions` | Keyed by `token_hash`; `sweep_sessions` becomes `DELETE … WHERE expires_at <= now()` |
| `Invitation` | `invitations` | `token_hash` unique; `expires_at` defaults to `issued_at + 7 days` (FR-013); `is_pending()` = not accepted, not revoked, not expired; expired is reported as `410`, unknown as `404` |
| `RecoveryCode` | `recovery_codes` | `issue_recovery_codes` revokes unused rows in the same transaction |
| `ApiToken` | `api_tokens` | From `0007`; live roles are always re-derived, never stored on the token |
| `Issue` | `issues` + `issue_state_changes` | `tags`/`frames`/`breadcrumbs` as `jsonb`; `suggestion_*` and `decision_*` columns flatten `Suggestion`/`CategorisationDecision` |
| `Event`, `Attachment` | `events`, `attachments` | **No payload column by design** — see §3 |
| `LogLine` | `log_lines` | `attributes` as `jsonb` |
| `AuditEntry` | `audit_log` | `actor` → new `actor_label` column; `actor_user_id` kept when the actor resolves to a user |
| `worker::Job` | `jobs` | Durable claim, see §4 |
| — (new) `OidcConnection`, `OidcLoginFlow` | `oidc_connections`, `oidc_login_flows` | See §5 |

## 2. Migration `0008_production_readiness.sql`

Additive and `IF NOT EXISTS`-idempotent, applied by the startup migration engine.

```sql
-- Demo confinement (FR-011, SC-014)
ALTER TABLE organisations ADD COLUMN IF NOT EXISTS is_demo boolean NOT NULL DEFAULT false;

-- Account lockout (FR-012, FR-032, SC-023): five consecutive failures lock the
-- account regardless of elapsed time, so only a counter and the lock are stored.
ALTER TABLE users ADD COLUMN IF NOT EXISTS locked_at timestamptz;
ALTER TABLE users ADD COLUMN IF NOT EXISTS failed_sign_in_count integer NOT NULL DEFAULT 0
    CHECK (failed_sign_in_count >= 0);

-- Actor labels so the API can show the human who acted without a join that may
-- fail after a user is removed.
ALTER TABLE audit_log ADD COLUMN IF NOT EXISTS actor_label text NOT NULL DEFAULT '';
ALTER TABLE issues    ADD COLUMN IF NOT EXISTS decision_by_label text;

-- Optional per-organisation OIDC (FR-018)
CREATE TABLE IF NOT EXISTS oidc_connections (
    org_id          uuid PRIMARY KEY REFERENCES organisations (id) ON DELETE CASCADE,
    issuer          text NOT NULL CHECK (issuer LIKE 'https://%' OR issuer LIKE 'http://%'),
    client_id       text NOT NULL,
    client_secret   text NOT NULL,          -- replayable; never serialized or logged
    scopes          text NOT NULL DEFAULT 'openid email profile',
    created_by      uuid REFERENCES users (id),
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS oidc_login_flows (
    state           text PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    nonce           text NOT NULL,
    pkce_verifier   text NOT NULL,
    return_to       text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS oidc_login_flows_expiry_idx ON oidc_login_flows (expires_at);

-- Case-insensitive containment search for the unified search view (FR-021).
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX IF NOT EXISTS issues_title_trgm_idx ON issues USING gin (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS log_lines_message_trgm_idx ON log_lines USING gin (message gin_trgm_ops);
```

`0001`–`0007` are unchanged and are executed for the first time by `src/schema.rs`. The startup engine
applies every pending migration before the service reports ready (FR-003, SC-021); re-application,
concurrent-start behaviour, and rollback are operator concerns and are not feature guarantees.

## 3. Why the events table carries no payload

The constitution makes raw payloads immutable and authoritative, and the object store already holds the
exact bytes at `raw/{project}/{event}`. Duplicating them into PostgreSQL would create a second copy that
could drift and would multiply storage.

Consequently the grouping worker loads the payload from object storage by
`events.raw_object_key`, derives the issue, and writes only derived metadata. If the object is missing,
the job fails with `last_error` naming the key and the event, and retries to its bound; the event row
and issue history are never fabricated from partial data.

`raw_expired_at IS NOT NULL` or `raw_object_key IS NULL` is the API's expired state (FR-019).

## 4. Job state machine

```text
pending ──claim──► running ──ok──────────► done
   ▲                   │
   └──backoff──────────┴──error──► failed (attempts < 5)
                                       │
                                       └──► exhausted (attempts = 5, no further claim)
```

- Ingest inserts `group_event` in the same transaction as the event.
- Grouping completion inserts `classify_event` only when a model is configured and the issue has no
  suggestion, no decision, and has not been presented (`first_presented_at IS NULL`, FR-019 of 001).
- `reconcile` is reserved and not scheduled by this feature.
- A `running` job older than the lock timeout is reclaimable, which is how a crashed instance's work
  returns to the queue.

## 5. OIDC entities

`OidcConnection` — one per organisation, optional:

| Field | Rule |
|---|---|
| `org_id` | Primary key, cascades with the organisation |
| `issuer` | Absolute URL; discovery must succeed when saved |
| `client_id` | Non-empty |
| `client_secret` | Non-empty; excluded from every response struct and log field |
| `scopes` | Must contain `openid email`; `profile` optional |

`OidcLoginFlow` — one per in-progress sign-in:

| Field | Rule |
|---|---|
| `state` | 256-bit random, primary key, single use |
| `nonce` | Bound into the ID token check |
| `pkce_verifier` | S256 challenge issued at start |
| `expires_at` | `created_at + 10 minutes`; expired rows are ignored and swept |

Flow rows are never exposed through the API. A callback with an unknown, expired, or already-consumed
`state` is rejected without revealing which case applied.

## 6. Invariants preserved from features 001/002

- At most one `users` row with `is_operator`.
- `organisations.slug` unique case-insensitively; `issues (project_id, number)` unique; `issues
  (project_id, fingerprint)` unique; `events (project_id, sentry_event_id)` unique.
- `invitations.token_hash`, `sessions.token_hash`, `recovery_codes.code_hash`, `ingest_keys.key_hash`,
  and `api_tokens.token_hash` are the only credential values stored; plaintext never persists.
- Retention clears raw references and deletes objects; it never deletes issue rows, counters, or state
  history, and never rewrites a stored payload.
- Soft-deleted projects keep every row and object.
- `deleted_at IS NULL` is required for a project to resolve from a DSN or an API request.
- Every project has at least one owner from the moment it is created; the operator names that owner,
  who must already belong to the owning organisation (FR-034, SC-026).
- `users.locked_at IS NOT NULL` means every sign-in attempt returns the generic invalid-credentials
  response, regardless of the password, until the deployment operator clears it. The count is
  consecutive and resets on any successful sign-in and on unlock; no window is stored. A lock never ends
  an existing session; disabling an account does (FR-032, SC-023).
- A dashboard session ends at seven days after issue or after 12 hours without activity; compatibility
  API tokens follow their own revocation rules and ignore session limits (FR-033, SC-024).
- The audit trail is stored durably and completely, and this feature exposes no route or screen for
  reading it (FR-007); completeness is verified by tests and direct queries.
- Operational counters are process-local and readable only by the deployment operator (FR-025); they
  are never served on a project-scoped or member-accessible route.
