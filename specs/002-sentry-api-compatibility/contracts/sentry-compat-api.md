# Contract: Sentry-Compatible Read API (`/api/0/`)

**Feature**: 002-sentry-api-compatibility | **Status**: design contract for implementation

**Authority**: The 11 paths in the Sentry OpenAPI document `v0`
(`https://raw.githubusercontent.com/getsentry/sentry/refs/heads/master/api-docs/openapi.json`).
Where this document and that document disagree on a field name or path segment, the OpenAPI document
wins and this contract is corrected in the same change.

## Common behaviour

| Aspect | Contract |
|---|---|
| Base path | `/api/0/`. Collections accept a trailing slash and work without it. |
| Availability | Disabled by default. When `SCOUT_SENTRY_API_COMPAT_ENABLED` is not true, every path returns `404` with `{"detail": "Not Found"}` before authentication. |
| Authentication | `Authorization: Bearer <token>` where the token starts `scout_pat_`. Missing, malformed, unknown, or revoked → `401` with the identical body `{"detail": "Invalid token"}`. |
| Content type | `application/json` for all responses, including errors. |
| Timestamps | RFC 3339 UTC with a `Z` suffix. |
| Ids | Strings, including the numeric-looking project id. |
| Trailing slash | Treated as equivalent to no trailing slash. |

### Scopes

Requests are authorised from the token owner's live roles: `event:read`, `project:read` for viewers;
`event:write` for triagers; `project:write`, `project:admin`, `event:admin`, `project:releases` for
admins/owners; `org:read` for organisation members and additionally `org:admin`,
`org:integrations` for organisation admins. Insufficient scope → `403`; a resource the owner cannot
reach → `404` (never `403`, to avoid revealing existence).

| Endpoint | Required scope |
|---|---|
| Issue list / read | `event:read` |
| Issue status mutation | `event:write` |
| Tag values (project and issue) | `event:read` |
| Release commit files | `project:releases` |
| Service hooks, feedback, spike protection (read) | `project:read` |
| App installations / external issues | `org:integrations` |

### Error envelope

Every error is `{"detail": "<message>"}` with one of `400`, `401`, `403`, `404`, `409`, `501`.

## Pagination

List endpoints that paginate use cursor pagination.

```http
HTTP/1.1 200 OK
Content-Type: application/json
Link: <https://scout.example/api/0/projects/acme/web/issues/?cursor=eyJ2IjoxfQ>; rel="next"; results="true"; cursor="eyJ2IjoxfQ"
X-Hits: 412
```

- `limit`: default 100, clamped to `1..=100`. Non-numeric, zero, or negative values fall back to the
  default rather than erroring.
- `cursor`: opaque, bound to the query that produced it. A cursor replayed against a different
  `query`, `sort`, `id`, `hashes`, org, or project is `400`. Malformed cursors are `400`. A cursor past
  the end returns `[]`, `results="false"`, and no `cursor` parameter.
- `X-Hits` is the total match count for the current query, independent of pagination.

## 1. `GET /api/0/projects/{org}/{project}/issues/`

Reads a project's issues as Sentry issue objects.

| Parameter | Rule |
|---|---|
| `query` | Sentry search subset: `is:unresolved\|resolved\|ignored`, `level:<level>`, `release:<value>`, `environment:<value>`, `<tag-key>:<value>`, plus free text over title, culprit, fingerprint. Unsupported operators are ignored, never `400`. |
| `sort` | `date` (default), `new`, `freq`, `user`, `trends`, `recommended`. `trends`/`recommended` fall back to `date`. |
| `limit` | Default 100, max 100. |
| `cursor` | Opaque cursor from a previous page. |
| `id` | Repeatable; restricts to those issue ids within the project. |
| `hashes` | Repeatable; matches Scout `fingerprint`. Unknown hash → empty page. |
| `statsPeriod`, `shortIdLookup` | Accepted and ignored. |
| Unknown parameters | Ignored. |

```json
[
  {
    "id": "3f2b1c9e-8a5d-4a0e-9f1c-2b6d7e8f9a01",
    "shortId": "WEB-17",
    "title": "TypeError: cannot read property 'id' of undefined",
    "culprit": "app/checkout.js in renderCart",
    "level": "error",
    "status": "unresolved",
    "statusDetails": {},
    "count": 42,
    "userCount": 0,
    "firstSeen": "2026-10-30T09:12:44Z",
    "lastSeen": "2026-11-01T08:03:10Z",
    "metadata": { "title": "TypeError: cannot read property 'id' of undefined", "value": "app/checkout.js in renderCart" },
    "permalink": "https://scout.example/#/issues/3f2b1c9e-8a5d-4a0e-9f1c-2b6d7e8f9a01",
    "project": { "id": "9a7c2e11-2d3f-4c5b-8e6a-7b8c9d0e1f20", "name": "Web", "slug": "web", "platform": null },
    "tags": [{ "key": "environment", "value": "production" }],
    "annotations": []
  }
]
```

Notes:

- `statusDetails` is always an object (never null). `ignoreCount` may be present when ignored.
- `count` is a JSON number. `userCount` is always `0` and `annotations` is always `[]` — Scout does
  not track distinct users or annotations.
- Ordering is deterministic: the requested sort, tie-broken by issue id.

**Errors**: `400` for a malformed/replayed cursor; `401`, `403`, `404` per the common rules.

## 2. `PUT /api/0/projects/{org}/{project}/issues/`

Applies `status` to issues.

```json
{ "status": "resolved" }
```

| Case | Result |
|---|---|
| `id` repeated | Each id **in this project** is updated; ids in other projects are skipped silently. |
| No `id` | Every issue matching the current filter is updated (all project issues when no filter is sent). |
| `status` present with `resolved` / `unresolved` / `ignored` | Applied; one state-history entry per change, actor = the token owner's email. |
| `status` absent, no other supported attribute | `400` naming the missing field. |
| Unsupported attribute (`assignedTo`, `isPublic`, `merge`, …) | `400` naming the unsupported field. Never silently ignored. |
| Unknown `status` value | `400`. |

Response: `200` with the updated Sentry issue objects. A status change is visible in the native API
and UI immediately (SC-006).

## 3. `DELETE /api/0/projects/{org}/{project}/issues/`

Always `501`:

```json
{ "detail": "Deleting issues is not supported: Scout preserves issue history and raw payloads." }
```

No issue, event, or payload is deleted.

## 4. `GET /api/0/projects/{org}/{project}/tags/{key}/values/`

Distinct values of one tag across the project.

| Parameter | Rule |
|---|---|
| `query` | Case-insensitive contains filter over values. |
| `limit` / `cursor` | Same pagination contract (cap 1,000 values total). |

```json
[{ "name": "web@1.0.0" }, { "name": "web@1.1.0" }]
```

Empty values are omitted, non-string scalars are stringified, duplicates collapse, and ordering is
ascending by value. A key with no values returns `[]` (never `404`).

## 5. `GET /api/0/organizations/{org}/issues/{issue}/tags/{key}/values/`

Same response and filter rules as (4), restricted to one issue. An issue outside the organisation or
inaccessible to the owner is `404`; an issue with no values for the key is `[]`.

## 6. `GET /api/0/organizations/{org}/releases/{version}/commitfiles/`

Always `[]` when the release version exists in an accessible project for this organisation; `404`
otherwise. Scout stores releases as issue attributes and has no commit data, so it never invents
entries.

## 7. Project service hooks

| Request | Response |
|---|---|
| `GET /api/0/projects/{org}/{project}/servicehooks/` | `[]` |
| `GET /api/0/projects/{org}/{project}/servicehooks/{hook}/` | `404` (no hook exists) |
| `POST /api/0/projects/{org}/{project}/servicehooks/` | `501` naming service hooks |
| `DELETE` on a hook | `501` naming service hooks |

## 8. Project user feedback

`GET /api/0/projects/{org}/{project}/users/` → `[]`. `POST` (legacy user report submission) → `501`
naming legacy user reports.

## 9. Spike protection

`GET` → `404` (no spike-protection state exists). `PUT`/`POST` → `501` naming spike protection.

## 10. Organisation app installations

`GET /api/0/organizations/{org}/sentry-app-installations/` → `[]`. Mutations → `501` naming Sentry
app installations.

## 11. External issues

`POST /api/0/organizations/{org}/sentry-app-installations/{installation}/external-issues/` and
`DELETE .../external-issues/{external_issue}/` → `501` naming external issues. No state changes.

## Compatibility matrix

| Path | Read | Mutate |
|---|---|---|
| Project issues | Full (list, filter, sort, page) | `PUT` status only; `DELETE` → `501` |
| Project tag values | Full | — |
| Issue tag values | Full | — |
| Release commit files | `[]` / `404` | — |
| Service hooks | `[]` / `404` | `501` |
| User feedback | `[]` | `501` |
| Spike protection | `404` | `501` |
| App installations | `[]` | `501` |
| External issues | — | `501` |

Anything not listed here is out of scope and must not be implemented or implied.
