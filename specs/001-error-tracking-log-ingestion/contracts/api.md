# Contract: Dashboard API

**Feature**: `001-error-tracking-log-ingestion` | **Plan**: [../plan.md](../plan.md)

All dashboard endpoints require `Authorization: Bearer <session token>`, except sign-in, invitation
preview, invitation acceptance, and health. Authorisation is checked server-side before data is read.

Error body for every failure:

```json
{ "error": "human-readable reason" }
```

Statuses used: `400` bad request, `401` unauthenticated, `403` authenticated but not permitted,
`404` not found or raw payload expired, `409` conflict, `413` too large, `503` dependency unavailable.

---

## Authentication

### Bootstrap (deployment operator)

On startup, if no user has `is_operator = true`, Scout creates the deployment operator from
`SCOUT_OPERATOR_EMAIL` and `SCOUT_OPERATOR_PASSWORD`. The password is hashed with Argon2id and never
logged. If no operator exists and either value is missing or blank, startup fails with an actionable
error naming the missing values. Later starts ignore the variables and do not reset the stored
password. The operator signs in through `POST /api/login`; its user object has `is_operator: true`.

### `POST /api/login`

```json
{ "email": "person@example.com", "password": "…" }
```

**200**

```json
{
  "token": "opaque-session-token",
  "user": { "id": "uuid", "email": "person@example.com", "name": "Person", "is_operator": false },
  "memberships": [
    { "project_id": "uuid", "project_name": "scout-web", "slug": "scout-web", "role": "owner" }
  ]
}
```

`401` for a wrong email or password (same body for both), `403` for a disabled account. There is no
public sign-up; accounts come from invitations or an operator.

### `POST /api/logout`

Deletes the session. **200** `{ "ok": true }`.

### `GET /api/me`

**200** `{ "user": {…, "is_operator": false}, "memberships": [ … ] }`.

### `POST /api/recover`

Unauthenticated password reset using a single-use recovery code issued at account creation.

```json
{ "email": "person@example.com", "recovery_code": "…", "new_password": "…" }
```

**200** `{ "ok": true }`. The code is consumed; other unused codes remain valid. An unknown email,
an unknown code, or a used code returns `401` with the same body. No email is sent.

When an account is created (by invitation acceptance, initial-admin setup, or operator bootstrap),
the creation response includes a `recovery_codes` array. The codes are shown once and stored only as
hashes. Every authenticated account can also reissue its codes through the endpoint below.

### `POST /api/me/recovery-codes`

Requires a bearer session. Generates a fresh set of single-use recovery codes for the caller, revokes
all prior unused codes, and returns the new set once.

**200**

```json
{ "recovery_codes": ["code-one", "code-two", "…"] }
```

Available to every account, including the deployment operator. No email is sent, and the codes are
never logged.

---

## Invitations

### `POST /api/orgs/{org_id}/invitations`

Requires organisation admin. Creates a one-time invitation token.

```json
{ "email": "new@example.com", "project_id": "uuid-or-null", "role": "triager-or-null" }
```

**200**

```json
{
  "invitation": {
    "id": "uuid",
    "kind": "project_invite",
    "email": "new@example.com",
    "project_id": "uuid",
    "role": "triager",
    "expires_at": "2026-11-07T12:00:00Z",
    "accepted_at": null
  },
  "token": "one-time-token-shown-only-here"
}
```

The plaintext `token` is returned once and never stored. Scout sends no email; the admin shares the
link out of band.

### `GET /api/invitations/{token}`

Unauthenticated preview. **200**

```json
{
  "email": "new@example.com",
  "organisation": "Acme",
  "project": "scout-web",
  "role": "triager",
  "expires_at": "2026-11-07T12:00:00Z"
}
```

`404` for an unknown token, `410` for an expired, revoked, or already-accepted one.

### `POST /api/invitations/{token}/accept`

```json
{ "name": "New Person", "password": "…" }
```

Creates a local account (or attaches the invitation to an existing account with the same email,
which must first authenticate), applies the project role, and returns a session in the same shape as
`POST /api/login`.

This endpoint also accepts an `org_admin_setup` token returned by `POST /api/orgs`. For a new
account it creates the account, sets the password, grants `org_admin`, and returns `recovery_codes`.
For an existing account, that person must authenticate first; the endpoint then grants `org_admin`
without changing the password. In both cases no project membership is created. The token is
single-use and expires after 7 days.

### `DELETE /api/orgs/{org_id}/invitations/{invitation_id}`

Requires organisation admin. Revokes a pending invitation. **200** `{ "ok": true }`.

---

## Organisations and projects

### `GET /api/orgs`

Requires the deployment operator. **200** `{ "organisations": [ { "id": "uuid", "name": "Acme", "created_at": "…" } ] }`.

### `POST /api/orgs`

Requires the deployment operator. Creates an organisation and its initial admin account.

```json
{ "name": "Acme", "initial_admin_email": "admin@example.com" }
```

**200**

```json
{
  "organisation": { "id": "uuid", "name": "Acme" },
  "initial_admin_setup_token": "one-time-token-shown-only-here"
}
```

If an account with `initial_admin_email` already exists, the setup token binds it only after that
person authenticates and consents; the token then grants `org_admin` and does **not** change their
password. If no account exists, redeeming the token creates it and sets a password. The operator
shares the setup token out of band; the admin redeems it with
`POST /api/invitations/{token}/accept`. The operator cannot create any other accounts.

### `GET /api/projects`

Returns only projects the caller is a member of.

**200**

```json
{
  "projects": [
    {
      "id": "uuid",
      "dsn_id": "1",
      "slug": "scout-web",
      "name": "scout-web",
      "org_name": "Acme",
      "role": "owner",
      "open_issues": 3,
      "total_issues": 4,
      "logs": 9,
      "model_configured": true,
      "retention_days": 30,
      "key": "scout_…-only-for-admin-and-above"
    }
  ]
}
```

### `POST /api/orgs/{org_id}/projects`

Requires organisation admin.

```json
{ "name": "billing-api", "slug": "billing-api" }
```

Creates the project and its first ingest key. The plaintext key is returned once.

### `GET /api/projects/{project_id}`

**200** `{ "project": {…}, "role": "owner", "key": "scout_…" }` (`key` only for admin and above).

### `GET /api/projects/{project_id}/settings`

Requires admin or owner. **200**

```json
{
  "project": { … },
  "role": "owner",
  "dsn": "http://scout_…@host:8080/1",
  "keys": [ { "id": "uuid", "label": "Default", "key_prefix": "scout_abc1", "revoked": false } ],
  "model": {
    "endpoint": "https://jev.example.com",
    "model": "jev-latest",
    "confidence_threshold": 0.5
  }
}
```

The plaintext `model.api_key` is never returned.

### `PUT /api/projects/{project_id}/settings`

Requires admin or owner.

```json
{
  "retention_days": 30,
  "model": {
    "endpoint": "https://jev.example.com",
    "api_key": "optional",
    "model": "jev-latest",
    "confidence_threshold": 0.5
  }
}
```

`model: null` disconnects the endpoint. Validation: retention `1..=3650`; endpoint `http(s)://`;
model id non-empty; threshold `0..=1`. **200** `{ "ok": true }`.

---

## Issues

### `GET /api/projects/{project_id}/issues`

Query parameters: `state` (`open` \| `resolved` \| `ignored`), `level`, `q`, `category`
(`issue` \| `log` \| `raw_event`, matching the human decision when present), and `sort` (`last_seen`
\| `count` \| `first_seen`). Any project member may call it.

**200** `{ "issues": [ Issue, … ] }`

```json
{
  "id": "uuid",
  "project_id": "uuid",
  "fingerprint": "sha256-hex",
  "title": "TypeError: boom",
  "culprit": "pay at src/checkout.ts:42",
  "level": "error",
  "first_seen": "…",
  "last_seen": "…",
  "count": 1284,
  "state": "open",
  "release": "web@2.4.1",
  "environment": "production",
  "tags": { "service": "checkout-web" },
  "frames": [ … ],
  "breadcrumbs": [ … ],
  "suggestion": { "category": "issue", "confidence": 0.91, "source": "jev-latest", "at": "…" },
  "decision": { "category": "issue", "by": "demo@example.com", "at": "…", "overridden": false },
  "state_history": [ { "state": "open", "by": "system", "at": "…" } ],
  "last_event_id": "uuid"
}
```

### `GET /api/issues/{issue_id}`

Any member with access. Sets `first_presented_at` on the first read, which freezes suggestions
(FR-019).

**200**

```json
{
  "issue": { … },
  "event": { "id": "uuid", "sentry_event_id": "…", "received_at": "…", "size": 2048, "attachment_count": 1 },
  "event_json": { … },
  "attachments": [ { "filename": "note.txt", "content_type": "text/plain", "size": 20 } ],
  "raw_available": true
}
```

`raw_available` is false after retention; the UI must show the payload as expired rather than
returning an error.

### `GET /api/issues/{issue_id}/raw`

Any member with access. Streams the immutable object with
`Content-Type: application/octet-stream` and `X-Scout-Event-Id: <sentry event id>`. `404` when the
payload has expired under retention.

### `POST /api/issues/{issue_id}/state`

Requires triager or above.

```json
{ "state": "resolved" }
```

Allowed values: `open`, `resolved`, `ignored`. Appends an `issue_state_changes` row with the actor's
email. **200** `{ "issue": { … } }`.

### `POST /api/issues/{issue_id}/categorisation`

Requires triager or above.

```json
{ "action": "confirm" }
```

or

```json
{ "action": "override", "category": "log" }
```

`category` must be `issue`, `log`, or `raw_event`. Confirm fails with `400` when no suggestion exists;
override fails with `400` when `category` is missing or invalid. The human decision is authoritative
and the suggestion remains on the issue for audit. **200** `{ "issue": { … } }`.

---

## Logs and unified search

### `GET /api/projects/{project_id}/logs`

Query: `q`, `level`, `limit` (default 200, maximum 1000). **200** `{ "logs": [ LogLine, … ] }`.

```json
{
  "id": "uuid",
  "project_id": "uuid",
  "timestamp": "…",
  "level": "warning",
  "message": "pool saturation observed",
  "source": "orders-api",
  "attributes": { "pool": "primary", "waiting": 48 },
  "received_at": "…"
}
```

### `GET /api/projects/{project_id}/search`

Query: `q` (required; blank returns empty results). **200** `{ "issues": [ … ], "logs": [ … ] }`,
each capped at 50 results and scoped to the project.

---

## Members and roles

### `GET /api/projects/{project_id}/members`

Any project member. **200**

```json
{
  "members": [
    { "user_id": "uuid", "name": "Dana Demo", "email": "demo@example.com", "role": "owner" }
  ]
}
```

### `POST /api/projects/{project_id}/members`

Requires admin or owner. Only an owner may grant `owner`.

```json
{ "email": "person@example.com", "role": "triager" }
```

### `PUT /api/projects/{project_id}/members/{user_id}`

Requires admin or owner. Only an owner may change an owner role. Demoting the final owner returns
`400`.

```json
{ "role": "viewer" }
```

### `DELETE /api/projects/{project_id}/members/{user_id}`

Requires admin or owner. Removing the final owner returns `400`.

### Role matrix

| Action | Viewer | Triager | Admin | Owner |
|--------|--------|---------|-------|-------|
| Read issues, logs, search, raw | yes | yes | yes | yes |
| Resolve/reopen, confirm/override | no | yes | yes | yes |
| Settings, keys, add/remove non-owner members | no | no | yes | yes |
| Grant or change the owner role | no | no | no | yes |

---

## Ingest keys

### `GET /api/projects/{project_id}/keys`

Requires admin or owner. Returns key metadata only (`id`, `label`, `key_prefix`, `created_at`,
`revoked_at`); never the plaintext key or its hash.

### `POST /api/projects/{project_id}/keys`

Requires admin or owner.

```json
{ "label": "ci" }
```

**200** returns the key object **including the plaintext `key` once**. It is never retrievable again.

### `DELETE /api/projects/{project_id}/keys/{key_id}`

Requires admin or owner. Revokes the key. Already-stored events are unaffected. **200**
`{ "ok": true }`.

---

## Health

### `GET /api/health`

Unauthenticated. **200**

```json
{ "status": "ok", "service": "scout", "version": "0.1.0", "database": "ok", "object_store": "ok" }
```

`503` when the database or object store is unreachable.
