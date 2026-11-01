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
| Operator authority | The operator may create organisations and issue initial-admin tokens; every project-scoped route denies it with `404`/`403` and never returns issues, events, logs, attachments, or keys |
| Sentry compatibility | `/api/0/` unchanged, still enabled only by `SCOUT_SENTRY_API_COMPAT_ENABLED=true` |
| Credential material | No response contains a password hash, token hash, recovery-code hash, ingest-key hash, OIDC client secret, or model API key |
