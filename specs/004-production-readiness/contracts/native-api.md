# Contract: Native API Changes

Base path `/api`. All responses are JSON. Errors keep the existing envelope
`{"error": "<actionable message>"}` and the existing status mapping in `src/error.rs`
(`400/401/403/404/409/410/413/429/503`). Authentication remains `Authorization: Bearer <session>`.
Every route below is either unchanged or explicitly listed; routes not listed here keep their feature
001/002 behaviour byte-for-byte.

## Changed responses

### `GET /api/health`

Now reports both durability dependencies. `200` only when both are reachable, otherwise `503`.

```json
{
  "status": "ok",
  "service": "scout",
  "version": "0.1.0",
  "storage": "ok",
  "store": "postgres",
  "durability": "durable",
  "object_store": "garage"
}
```

| Field | Values | Meaning |
|---|---|---|
| `status` | `ok`, `degraded` | Overall readiness |
| `storage` | `ok`, `unavailable` | Metadata probe (`SELECT 1`, 2 s timeout) |
| `store` | `postgres`, `memory` | Active adapter |
| `durability` | `durable`, `ephemeral` | `ephemeral` when the memory store is active |
| `object_store` | backend name, `unavailable` | Existing object-store probe |

### `GET /api/orgs`

Operator only. Each organisation now includes `slug` and `is_demo` so the dashboard can label the demo
organisation and link it to compatibility paths.

```json
{"organisations":[{"id":"…","name":"Acme","slug":"acme","is_demo":false,
                   "created_by":"…","created_at":"2026-11-01T10:00:00Z"}]}
```

### `POST /api/orgs`

Unchanged request (`name`, `initial_admin_email`) and unchanged response
(`organisation`, `initial_admin_setup_token`). The created organisation always has `is_demo: false`.

### `POST /api/orgs/{org_id}/projects` — operator only

Project creation moved to the deployment level (FR-034). The request now names the initial owner,
who must already belong to the organisation.

```json
{"name":"api","slug":"api","initial_owner_email":"owner@acme.test"}
```

| Case | Response |
|---|---|
| Operator, valid owner | `200` with `project` only — **no `key` and no `dsn`**, because the operator must never hold an ingest credential (FR-012) |
| Caller is not the operator | `403` (organisation admins and members cannot create projects) |
| Initial owner is not a member of the organisation | `400` with an actionable message; nothing is created |
| Slug already used in the organisation | `409` |

The named owner signs in and creates the first ingest key with `POST /api/projects/{project_id}/keys`,
which returns the plaintext key and the full DSN once. No other path may transfer that credential to the
operator.

### `GET /api/projects/{project_id}/search`

Returns at most 200 combined results, issues and logs interleaved newest-first (FR-021). The response
carries `has_more: true` when matches were truncated; there is no pagination in this feature.

### Demo endpoints (`SCOUT_DEMO_MODE` only)

| Route | Change |
|---|---|
| `GET /api/demo/accounts` | Lists only members of the demo organisation; excludes the operator and every real member |
| `POST /api/demo/reset` | Replaces demo-organisation data only; real organisations, their projects, and the operator are untouched |

Both routes stay unregistered when demo mode is off (`404`, as today). A demo session is authorised
only against demo-organisation projects; any other project resolves as `404`.

## Added routes

### `GET /api/metrics` — operator only

Returns process counters since start. `401` when unauthenticated, `403` when the caller is not the
operator.

```json
{
  "uptime_seconds": 3120,
  "store": "postgres",
  "counters": {
    "ingest_accepted": 1530042,
    "ingest_rejected": {"auth": 12, "malformed": 40, "oversized": 3, "saturated": 61, "storage": 0},
    "grouping_ok": 1529980,
    "grouping_failed": 2,
    "classifier_suggested": 810,
    "classifier_no_suggestion": 1529000,
    "classifier_failed": 170,
    "retention_objects_deleted": 4120,
    "retention_logs_deleted": 90300,
    "raw_expired": 4120
  }
}
```

### Operator account administration

| Route | Purpose |
|---|---|
| `GET /api/operator/users?locked=true` | Lists accounts, filtered to locked ones; returns `id`, `email`, `locked_at`, never password material |
| `POST /api/operator/users/{user_id}/unlock` | Clears the lock and resets the failure count; `200 {"ok":true}`, `404` unknown account, `403` non-operator |

Both are deployment-level routes, scoped to the operator exactly like `/api/orgs` and `/api/metrics`.
Unlocking is audited as `user.unlocked`. A locked account returns the same `401` body as a wrong
password on every sign-in attempt, including with the correct password (FR-032, SC-023).

### Organisation OIDC configuration — organisation admin only

| Route | Purpose |
|---|---|
| `GET /api/orgs/{org_id}/oidc` | Current connection: `issuer`, `client_id`, `scopes`, `configured_at`; **never** the secret |
| `PUT /api/orgs/{org_id}/oidc` | Create or replace: `{issuer, client_id, client_secret, scopes?}` |
| `DELETE /api/orgs/{org_id}/oidc` | Remove the connection; local sign-in continues to work |

`PUT` validates discovery synchronously and returns `400` with the provider's error summary when the
issuer is unreachable or the metadata is malformed. Every change is audited
(`oidc.configured`, `oidc.removed`). Flow details, including the public start/callback routes, are in
[oidc.md](./oidc.md).

## Unchanged but re-asserted semantics

| Behaviour | Contract |
|---|---|
| Ingest acknowledgement | `202` only after the raw object and metadata are committed; `503` with `Retry-After` on storage failure; nothing acknowledged that was not stored |
| Saturation | `429` with `Retry-After` when the global in-flight bound is reached |
| Oversized input | `413`, nothing partial persisted |
| Authorisation | Project-scoped routes resolve one project and one role per request; denied resources return `404` (not `403`) when existence would leak, `403` when the project is known and the role is insufficient |
| Sign-in lockout | Five **consecutive** failures for one account lock it, regardless of time between attempts; every later attempt returns the generic `401` `{"error":"invalid email or password"}` until the operator unlocks it; a successful sign-in resets the count |
| Password minimum | Every path that sets a password rejects fewer than 12 characters with a message naming the minimum |
| Session lifetime | Sessions expire after seven days or 12 idle hours; compatibility API tokens are unaffected |
| Operator authority | The operator may create organisations and issue initial-admin tokens; every project-scoped route denies it with `404`/`403` and never returns issues, events, logs, attachments, or keys |
| Sentry compatibility | `/api/0/` unchanged, still enabled only by `SCOUT_SENTRY_API_COMPAT_ENABLED=true` |
| Credential material | No response contains a password hash, token hash, recovery-code hash, ingest-key hash, OIDC client secret, or model API key |
