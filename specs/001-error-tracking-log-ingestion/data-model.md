# Phase 1 Data Model: Scout — Self-Hosted Error Tracking & Log Ingestion

**Feature**: `001-error-tracking-log-ingestion` | **Date**: 2026-10-31 | **Plan**: [plan.md](./plan.md)

PostgreSQL is the metadata store; object storage holds immutable payloads. All primary keys are UUIDs,
all timestamps are `timestamptz`, and every query is filtered by `project_id` (or `org_id` for
organisation-level administration) before data is returned.

---

## Entity overview

```text
Organisation 1 ──< Project 1 ──< Issue 1 ──< Event
     │                │             │          └──< Attachment
     │                │             └──< IssueStateChange
     │                ├──< ProjectMember >── User
     │                ├──< IngestKey
     │                ├──< Invitation >── User (creator)
     │                └──< LogLine
     ├──< OrgMember >── User
     └──< Invitation, AuditLog

User 1 ──< Session
Job ── references Project + Event
```

## Tables

### `organisations`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `name` | text | not null, 1–120 chars |
| `sso_provider` | text | nullable; provider identifier when OIDC is configured |
| `sso_config` | jsonb | nullable; issuer, client id, and secret reference (never a plaintext secret) |
| `created_by` | uuid | nullable; FK → users. The deployment operator that created the organisation |
| `created_at` | timestamptz | not null, default now |

### `users`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `email` | citext | not null, unique; stored lowercased |
| `name` | text | not null |
| `password_hash` | text | nullable only for accounts that authenticate exclusively through OIDC |
| `is_operator` | boolean | not null, default false; at most one true row (partial unique index) |
| `created_at` | timestamptz | not null, default now |
| `disabled_at` | timestamptz | nullable |

The deployment operator is a normal `users` row with `is_operator = true`. It is not scoped to any
organisation and is created at startup only when no operator exists. A partial unique index
(`WHERE is_operator`) enforces at most one.

### `org_members`

| Column | Type | Constraints |
|--------|------|-------------|
| `org_id` | uuid | FK → organisations, PK part |
| `user_id` | uuid | FK → users, PK part |
| `role` | text | `org_admin` \| `org_member`, not null |
| `created_at` | timestamptz | not null |

### `projects`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `org_id` | uuid | FK → organisations, not null |
| `slug` | text | not null, unique per organisation |
| `name` | text | not null |
| `dsn_id` | text | not null, unique; numeric path segment used by the DSN |
| `retention_days` | integer | not null, default 30, check `1..=3650` |
| `model_endpoint` | text | nullable; must start `http://` or `https://` |
| `model_name` | text | nullable; non-empty when an endpoint is set |
| `model_api_key_ref` | text | nullable; reference to a secret, never the key itself |
| `confidence_threshold` | double precision | not null, default `0.5`, check `0..=1` |
| `created_at` | timestamptz | not null |

### `project_members`

| Column | Type | Constraints |
|--------|------|-------------|
| `project_id` | uuid | FK → projects, PK part |
| `user_id` | uuid | FK → users, PK part |
| `role` | text | `owner` \| `admin` \| `triager` \| `viewer`, not null |
| `created_at` | timestamptz | not null |

A project MUST retain at least one `owner`; demoting or removing the final owner is rejected.

### `invitations`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `org_id` | uuid | FK → organisations, not null |
| `kind` | text | `project_invite` \| `org_admin_setup`, not null, default `project_invite` |
| `project_id` | uuid | FK → projects, nullable; null for `org_admin_setup` |
| `email` | citext | not null |
| `role` | text | project role for `project_invite`; null for `org_admin_setup` |
| `token_hash` | text | not null, unique; SHA-256 of the one-time token |
| `created_by` | uuid | FK → users, not null |
| `expires_at` | timestamptz | not null, default now + 7 days |
| `accepted_at` | timestamptz | nullable |
| `accepted_user_id` | uuid | nullable; FK → users created or bound on acceptance |
| `revoked_at` | timestamptz | nullable |

The plaintext token is shown once at creation and never stored. Delivery is out of band; no email is
sent. An `org_admin_setup` token is returned to the operator when creating an organisation; redeeming
it creates (or binds a verified existing account) and grants `org_members.role = org_admin`.

### `ingest_keys`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `project_id` | uuid | FK → projects, not null |
| `label` | text | not null |
| `key_hash` | text | not null, unique; SHA-256 of the ingest key |
| `key_prefix` | text | not null; first 12 characters, for identification in the UI |
| `created_at` | timestamptz | not null |
| `revoked_at` | timestamptz | nullable |
| `last_used_at` | timestamptz | nullable |

The plaintext key is shown once at creation. Ingest authenticates by hashing the presented key.

### `sessions`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `user_id` | uuid | FK → users, not null |
| `token_hash` | text | not null, unique; SHA-256 of the opaque session token |
| `created_at` | timestamptz | not null |
| `expires_at` | timestamptz | not null, default now + 7 days |
| `last_seen_at` | timestamptz | nullable |

Logout deletes the row. Expired rows are swept periodically.

### `recovery_codes`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `user_id` | uuid | FK → users, not null |
| `code_hash` | text | not null, unique; SHA-256 of the single-use code |
| `created_at` | timestamptz | not null |
| `used_at` | timestamptz | nullable |
| `revoked_at` | timestamptz | nullable; set when the account reissues its codes |

A set of single-use codes is issued when a local account is created and shown once. Redemption sets a
new password and marks the code used; the remaining codes stay valid until used or replaced. An
authenticated account can reissue the set: prior unused codes are revoked and the new set is shown
once.

### `issues`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `project_id` | uuid | FK → projects, not null |
| `fingerprint` | text | not null; unique per project |
| `title` | text | not null |
| `culprit` | text | nullable |
| `level` | text | not null, default `error` |
| `first_seen` | timestamptz | not null |
| `last_seen` | timestamptz | not null |
| `count` | bigint | not null, default 1, check `>= 1` |
| `state` | text | `open` \| `resolved` \| `ignored`, not null, default `open` |
| `release` | text | nullable |
| `environment` | text | nullable |
| `tags` | jsonb | not null, default `{}` |
| `frames` | jsonb | not null, default `[]` |
| `breadcrumbs` | jsonb | not null, default `[]` |
| `suggestion_category` | text | nullable; `issue` \| `log` \| `raw_event` |
| `suggestion_confidence` | double precision | nullable, check `0..=1` |
| `suggestion_source` | text | nullable; model identifier |
| `suggestion_at` | timestamptz | nullable |
| `decision_category` | text | nullable; `issue` \| `log` \| `raw_event` |
| `decision_by` | uuid | nullable; FK → users |
| `decision_at` | timestamptz | nullable |
| `decision_overridden` | boolean | not null, default false |
| `first_presented_at` | timestamptz | nullable; set the first time the issue detail is opened by a member |
| `last_event_id` | uuid | nullable; FK → events (`ON DELETE SET NULL`) |

Unique: `(project_id, fingerprint)`. Indexes: `(project_id, state, last_seen desc)`,
`(project_id, decision_category, last_seen desc)`, `(project_id, count desc)`, GIN on `tags`.

### `issue_state_changes`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `issue_id` | uuid | FK → issues, not null |
| `state` | text | `open` \| `resolved` \| `ignored`, not null |
| `actor_user_id` | uuid | nullable; FK → users. Null means the `system` actor |
| `actor_label` | text | not null; email, or `system` for automated changes |
| `at` | timestamptz | not null |

Append-only. A state change and its history row are written in one transaction.

### `events`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `project_id` | uuid | FK → projects, not null |
| `issue_id` | uuid | nullable; FK → issues (`ON DELETE SET NULL`) until the worker groups it |
| `sentry_event_id` | text | not null |
| `received_at` | timestamptz | not null |
| `raw_object_key` | text | nullable; null after retention expires the payload |
| `raw_size` | bigint | not null |
| `raw_expired_at` | timestamptz | nullable |
| `attachment_count` | integer | not null, default 0 |

Unique: `(project_id, sentry_event_id)` — deduplicates retried deliveries (FR-006). Index:
`(project_id, received_at)`.

The event row survives retention so issue counts and triage history remain; only `raw_object_key`,
attachments, and the object itself are removed, and `raw_expired_at` is set.

### `attachments`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `event_id` | uuid | FK → events, not null |
| `filename` | text | not null |
| `content_type` | text | not null |
| `size` | bigint | not null, check `0..=10485760` |
| `object_key` | text | not null |
| `created_at` | timestamptz | not null |

### `log_lines`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `project_id` | uuid | FK → projects, not null |
| `timestamp` | timestamptz | not null |
| `level` | text | not null, default `info` |
| `message` | text | not null |
| `source` | text | not null, default `ingest` |
| `attributes` | jsonb | not null, default `{}` |
| `received_at` | timestamptz | not null |

Indexes: `(project_id, timestamp desc)`, GIN on `attributes`.

### `jobs`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `kind` | text | `group_event` \| `classify_event` \| `reconcile` |
| `project_id` | uuid | FK → projects, not null |
| `event_id` | uuid | nullable; FK → events |
| `state` | text | `pending` \| `running` \| `done` \| `failed`, not null |
| `attempts` | integer | not null, default 0 |
| `available_at` | timestamptz | not null, default now |
| `locked_at` | timestamptz | nullable |
| `last_error` | text | nullable |
| `created_at` | timestamptz | not null |

Index: `(state, available_at)` for claim queries. Workers use `FOR UPDATE SKIP LOCKED`.

### `audit_log`

| Column | Type | Constraints |
|--------|------|-------------|
| `id` | uuid | PK |
| `org_id` | uuid | FK → organisations, nullable; null for deployment-level actions (operator bootstrap) |
| `project_id` | uuid | nullable |
| `actor_user_id` | uuid | nullable |
| `action` | text | e.g. `role.granted`, `key.revoked`, `model.updated` |
| `target` | text | nullable |
| `metadata` | jsonb | not null, default `{}` |
| `at` | timestamptz | not null |

---

## Validation rules

- Emails are trimmed and lowercased before comparison; uniqueness is case-insensitive (`citext`).
- A local account requires `password_hash`; an OIDC-only account may omit it. Neither can have both
  disabled and an active session.
- `retention_days` is an integer in `1..=3650`.
- `confidence_threshold` is in `0..=1`, default `0.5`.
- A model endpoint must be `http://` or `https://` and `model_name` must be non-empty when connected.
- Ingest requests and individual attachments are limited to 10 MB; oversized input is rejected with
  `413` and nothing partial is persisted.
- `fingerprint` is opaque text but unique per project; it is recomputable from the stored payload.
- Raw object keys are write-once. No code path updates an existing object; retention only deletes.
- Every issue must have at least one `owner` in `project_members`.
- A member can hold one role per project; `project_members` is keyed by `(project_id, user_id)`.
- At most one `users.is_operator = true`. Startup seeding creates the operator only when none exists;
  later starts MUST NOT update the stored password.
- Every organisation MUST have either a pending `org_admin_setup` token or an `org_admin` member
  before project invitations are issued.
- `org_admin_setup` tokens are single-use, hashed at rest, and expire after 7 days; accepting one
  creates or binds the initial admin and grants `org_admin`.
- If an `org_admin_setup` token's email belongs to an existing account, acceptance requires that
  account to authenticate and consent; `accepted_user_id` is set and `password_hash` is unchanged.
- Reissuing recovery codes revokes all prior unused codes and returns the new set once; only hashes
  are stored.
- On startup, if no operator exists and either bootstrap value is missing or blank, startup fails and
  no partial operator row is created.
- Operator-created accounts are limited to the initial organisation admin; all other accounts are
  created by invitation.

## State transitions

| Entity | From | Event | To | Notes |
|--------|------|-------|----|-------|
| Issue | — | first occurrence grouped | `open` | History row with `system` |
| Issue | `open` | triager resolves | `resolved` | History row with actor |
| Issue | `resolved` | triager reopens | `open` | History row with actor |
| Issue | `open` | triager ignores | `ignored` | History row with actor |
| Issue | `ignored` | triager reopens | `open` | History row with actor |
| Invitation | — | admin creates | pending | Token shown once |
| Invitation | pending | invitee redeems | accepted | `accepted_at` set; membership created |
| Invitation | pending | admin revokes / expires | revoked / expired | Token no longer redeemable |
| Deployment operator | — | startup, no operator exists | active | Seeded from env; password hashed |
| Deployment operator | — | startup, no operator and vars missing | startup fails | Actionable error; no row created |
| Deployment operator | active | later startup with env set | unchanged | Password not reset; warning logged |
| Organisation | — | operator creates | active | `created_by` set; initial-admin setup token issued |
| Initial admin setup | pending | new account redeems | accepted | User created; password set; `org_admin` granted |
| Initial admin setup | pending | existing account authenticates and consents | accepted | `accepted_user_id` set; password unchanged |
| Recovery code | active | authenticated reissue | revoked | Prior unused codes invalidated; new set shown once |
| Ingest key | — | owner/admin creates | active | Plaintext shown once |
| Ingest key | active | owner/admin revokes | revoked | Terminal; already-stored events unaffected |
| Session | — | successful sign-in | active | Opaque token returned once |
| Session | active | logout / expiry | deleted / expired | Token no longer authenticates |
| Recovery code | active | password reset | used | Single use; other codes stay valid |
| Event | stored | retention cutoff | raw expired | Raw object + attachments removed; row kept |
| Job | pending | worker claim | running | `locked_at` set |
| Job | running | success / retryable failure | done / pending (with backoff) | `attempts` incremented on failure |

## Derived and denormalised fields

- `issues.count`, `first_seen`, and `last_seen` are maintained by the grouping worker inside the same
  transaction that links the event.
- `events.issue_id` is set by the grouping worker; an event belongs to exactly one issue.
- `raw_available` in API responses is `raw_object_key IS NOT NULL`.
- A suggestion is applied only while `first_presented_at IS NULL` and `decision_at IS NULL`; once a
  triager has seen the issue or decided, a late answer is discarded (FR-019).
- `open_issues`, `total_issues`, and `logs` counters on project summaries are computed per query in
  the prototype and may become maintained counters if profiling requires it.
- The deployment operator is a deployment-level principal: it can list and create organisations but
  cannot read a project's issues, logs, or raw payloads unless it is also a member of that project.
