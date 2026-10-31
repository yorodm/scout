# Phase 0 Research: Sentry Read-API Compatibility

All open questions from the specification and Technical Context are resolved here. Each decision is
what the implementation followed unless a later task explicitly revisits it.

## R1. Module layout and route isolation

- **Decision**: Put all compatibility code in `src/compat/` (`mod`, `auth`, `error`, `cursor`,
  `projection`, `query`, `handlers`, `tokens`). `compat::config` registers a single
  `web::scope("/api/0")`. The native token endpoints are registered from `api::config` because they
  live under `/api`.
- **Rationale**: The Sentry surface needs its own error envelope (`detail`), its own pagination, and
  its own authentication path. Isolating it keeps `ApiError`/`{"error": ...}` and the browser session
  extractor untouched, which SC-008 requires.
- **Alternatives considered**: (a) Extending `api.rs` — rejected: a 1,892-line file already, and the
  two error conventions would interleave. (b) A separate binary/workspace crate — rejected by
  constitution IV and V (single deployment unit, single binary crate).

## R2. Bearer token model and storage

- **Decision**: A `scout_pat_`-prefixed token built from `auth::new_token()` (32 random bytes,
  base64url unpadded). Store `ApiToken { id, user_id, label, token_hash, token_prefix, created_at,
  last_used_at, revoked_at }`. Hash with `auth::hash_token` (SHA-256 hex, same as sessions and ingest
  keys). Prefix = first 14 characters. `last_used_at` is written at most once per hour per token to
  avoid turning every read into extra write contention.
- **Rationale**: Reuses the project's existing credential primitives and the "hash only, show once"
  rule (constitution Security). A distinct prefix lets operators recognise Scout tokens in logs and
  makes accidental leakage identifiable without revealing the secret.
- **Alternatives considered**: (a) Reusing session tokens — rejected: sessions expire
  (`SCOUT_SESSION_TTL_DAYS`) and are browser-scoped. (b) Storing scopes per token — rejected by
  clarification 1 (live roles). (c) Argon2 for tokens — rejected: tokens are high-entropy, so a fast
  hash is appropriate and constant-time lookup by hash is required.

## R3. Scope derivation from live roles

- **Decision**: Resolve the project role (`Role`) and, when the project belongs to an organisation the
  user is a member of, the `OrgRole`, on every request. Map:
  - Viewer → `event:read`, `project:read`
  - Triager → viewer set + `event:write`
  - Admin/Owner → triager set + `project:write`, `project:admin`, `event:admin`, `project:releases`
  - Org member → `org:read`; org admin → `org:read`, `org:admin`, `org:integrations`
- **Rationale**: Matches FR-004/FR-005 and clarification 1: role changes take effect on the next
  request with no reissue. Computing a pod-local `HashSet<Scope>` per request is cheap.
- **Alternatives considered**: Persisting a scope list at token creation — rejected: drifts from
  roles and requires reissue. Deriving scopes from `Role` up front at app start — rejected: a role
  change would need a restart to be denied.

## R4. Organisation slug and address resolution

- **Decision**: Add `Organisation.slug`, derived at creation by lowercasing the name, replacing runs
  of non-alphanumerics with `-`, trimming `-`, truncating to 48 characters, and appending `-2`, `-3`,
  … until unique (case-insensitive). An organisation is addressable by slug or UUID string. A project
  is addressable by slug, UUID string, **or** `dsn_id`. Resolution walks
  `org → project`, then checks `deleted_at.is_none()`; any miss is `404`.
- **Rationale**: FR-006 requires Sentry-style paths without an extra operator step. Deriving at
  creation keeps the slug stable when the name is later edited. Slug-first lookup with UUID fallback
  keeps the native identifiers usable.
- **Alternatives considered**: (a) Using the org UUID as the slug — rejected: Sentry clients and docs
  use human-readable org slugs. (b) Recomputing the slug from the current name — rejected: breaks
  stored links and cursors. (c) Using the org name directly URL-encoded — rejected: ambiguous and
  case-sensitive.

## R5. Sentry issue projection

- **Decision**: A `SentryIssue` struct built from a stored `Issue` plus its `Project`:
  - `id` = Scout issue UUID as a string; `shortId` = `{PROJECT_SLUG_UPPER}-{number}`.
  - `title` = issue title; `culprit` = stored culprit or `""`; `level` = stored level.
  - `status` = `unresolved|resolved|ignored`; `statusDetails` = `{}` for every issue (present, never
    null), plus `ignoreCount` when ignored.
  - `count` = event count as a JSON number; `userCount` = `0`; `annotations` = `[]`.
  - `firstSeen`/`lastSeen` = RFC 3339 UTC with `Z`; `metadata` = `{ "title": title, "value":
    culprit_or_empty }`.
  - `permalink` = `{self_url}/#/issues/{id}` (the existing UI detail route).
  - `project` = `{ id, name, slug, platform: null }`; `tags` = `[{ "key": k, "value": v }]` derived
    from the stored tag object (stringified, empty values omitted).
- **Rationale**: Sentry clients read these fields structurally; omitting any is a parse failure while
  plausible values are harmless. `userCount = 0` and empty annotations are the honest answers
  (spec Assumptions) rather than fabricated data.
- **Alternatives considered**: (a) Reusing Scout's native issue JSON — rejected: different field names
  and shapes. (b) Returning `count` as a string — rejected: this document's schema does not require a
  string, and numeric counts are what Scout stores; the projection isolates the choice to one field if
  a client insists otherwise. (c) Fabricating metadata/permalink placeholders — rejected by FR-024.

## R6. Search grammar and sort mapping

- **Decision**: Tokenise `query` on whitespace, honouring double quotes. A token `key:value` is
  interpreted only for `is`, `level`, `release`, `environment`, and any key present in an issue's
  `tags`. `is:` accepts `unresolved|resolved|ignored`; `is:unresolved` matches **open only** (FR-012).
  All other `key:value` tokens are ignored (no `400`). Bare tokens are a case-insensitive needle over
  `title`, `culprit`, and `fingerprint`. Multiple supported tokens are ANDed. Sort maps as FR-011:
  `date` → last seen desc, `new` → first seen desc, `freq` → count desc, `user` → count desc,
  `trends`/`recommended` → date. Every sort is tie-broken by issue UUID so cursors are deterministic.
- **Rationale**: FR-010 requires an honest, usable subset and forbids erroring on unknown operators.
  AND semantics match Sentry. A deterministic tiebreak is a prerequisite for R7.
- **Alternatives considered**: (a) Rejecting unsupported tokens with `400` — explicitly forbidden.
  (b) Full Sentry grammar including `assigned:`, `bookmarks:`, wildcards — the data does not exist;
  ignoring is the honest behaviour and is documented in the compatibility matrix.
- **`hashes` parameter**: Sentry's group hash is Scout's `fingerprint`. `hashes` filters issues whose
  `fingerprint` equals any supplied value. An unknown hash yields an empty page, never an error.

## R7. Cursor design and pagination headers

- **Decision**: A cursor is base64url (no padding) of
  `{"v":1,"q":"<16-hex query fingerprint>","s":"<sort>","k":<last sort key>,"i":"<issue uuid>"}`.
  The query fingerprint is the SHA-256 of the normalised `(query, sort, ids, hashes, org, project)`
  tuple, truncated to 16 hex characters. Following a cursor means an exclusive keyset comparison on
  `(sort key, uuid)` in the cursor's direction. A cursor whose `q` differs from the current request is
  `400`; a malformed or non-decodable cursor is `400`; a cursor past the end yields an empty page with
  `results="false"`. Page size = effective `limit` (default 100, clamped to 1..=100).
- **Rationale**: SC-005/FR-020 demand exactly-once traversal while new issues arrive; keyset
  pagination provides it, offsets do not. Binding the fingerprint prevents a cursor from silently
  producing a different result set.
- **Alternatives considered**: (a) Offset cursors — rejected: inserts shift pages and duplicate/skip
  issues. (b) Opaque database cursors — no database in this slice. (c) Stateless HMAC-signed
  cursors — unnecessary; the fingerprint detects misuse without a secret.
- **Headers**: `Link: <{base}/api/0/projects/{org}/{project}/issues/?cursor={c}>; rel="next";
  results="true"; cursor="{c}"`, `X-Hits: {total}`. The last page still emits `results="false"` and
  omits `cursor` from the `Link` value.

## R8. Error envelope and status mapping

- **Decision**: A dedicated `CompatError` enum (`BadRequest`, `Unauthorized`, `Forbidden`,
  `NotFound`, `Conflict`, `Unsupported`) implementing `ResponseError`, always serialising
  `{"detail": "<message>"}`. Mapping:
  - missing/malformed/unknown/revoked token → `401` with the identical body `Invalid token`
  - valid token, insufficient scope → `403`
  - unknown org/project/issue/release, or a project the owner cannot access → `404`
  - unsupported mutation attribute or missing supported attribute → `400` naming the field
  - subsystems Scout does not implement → `501` with a message naming the feature
- **Rationale**: FR-021 and FR-002's uniform `401` avoid leaking token state; `403` vs `404` follows
  Sentry and constitution III.
- **Alternatives considered**: Reusing `ApiError` — rejected: it emits `{"error": ...}` and would
  change native behaviour if modified.

## R9. Opt-in configuration

- **Decision**: `Config.sentry_compat_enabled` from `SCOUT_SENTRY_API_COMPAT_ENABLED` via the
  existing `env_flag` helper (default `false`). `AppState.compat_enabled` carries it. A middleware
  wrapping the `/api/0` scope short-circuits every request with `404 {"detail": "Not Found"}` when
  disabled, before authentication.
- **Rationale**: Assumptions require opt-in and default-off; `404` keeps the surface invisible
  (FR-007), and gating before auth avoids timing/behaviour differences.
- **Alternatives considered**: Registering routes conditionally — works, but makes the disabled
  behaviour depend on router fallbacks and is harder to test consistently. A feature flag per route —
  duplicated logic.

## R10. Token management surface

- **Decision**: Native endpoints:
  - `GET /api/me/tokens` → metadata only (`id`, `label`, `token_prefix`, `created_at`,
    `last_used_at`, `revoked_at`).
  - `POST /api/me/tokens` `{ "label": "…" }` → `201` with `token` (plaintext, once) plus metadata.
  - `DELETE /api/me/tokens/{token_id}` → `204`, idempotent for already-revoked tokens, `404` for
    someone else's token.
  - `GET /api/orgs/{org_id}/tokens` and `DELETE /api/orgs/{org_id}/tokens/{token_id}` for org admins,
    listing members' tokens (metadata only) and revoking any of them.
  - Every create/revoke writes an audit row (`token.create`, `token.revoke`).
  The dashboard screen (`ui/src/components/tokens.rs`) lists the signed-in user's tokens, creates one
  with a label, shows the plaintext once with a copy action, and revokes with a confirmation.
- **Rationale**: Clarification 5 requires both native endpoints and a dashboard screen. Self-service
  creation matches "owned by a user" and live-role inheritance; org-admin listing/revocation provides
  the immediate containment promised in clarification 4 without inventing delegation semantics.
- **Alternatives considered**: (a) Admin-creates-for-user — adds target selection with no additional
  safety; the owner can create their own token. (b) Dashboard-only — rejected by clarification 5.

## R11. Tag values

- **Decision**: Derive from `Issue.tags` (a JSON object) across the project's issues, stringifying
  string/number/bool values and skipping null/empty. De-duplicate case-sensitively, sort ascending
  by value, apply `query` as a case-insensitive contains filter, cap at 1,000. The issue-scoped path
  applies the same derivation to one issue. Missing key → `[]`.
- **Rationale**: FR-016–FR-018 and the Assumptions note that no separate tag index exists in this
  slice.
- **Alternatives considered**: Maintaining a tag index — rejected as scope creep; 1,000-value cap
  documented as accepted.

## R12. Unsupported subsystems

- **Decision**: A single table in `handlers.rs` describes each non-Scout subsystem:
  releases commit files (`[]` for a known release, `404` otherwise), service hooks (list `[]`, detail
  `404`), user feedback (`[]`), app installations (`[]`), spike protection (GET `404`/configured
  object? → `501` on mutation), external issues (mutation `501`). Any mutating call returns `501` with
  a message naming the feature; `DELETE issues/` always returns `501` citing payload preservation.
- **Rationale**: FR-022–FR-024: answer within the documented shapes, never fabricate, never mutate.
- **Alternatives considered**: Returning `404` for everything unimplemented — hides the difference
  between "not found" and "not supported" and breaks clients that switch on status.

## R13. PostgreSQL parity

- **Decision**: Migration `0007_sentry_compat.sql` (idempotent) adds:
  `organisations.slug TEXT` with a case-insensitive unique index and a backfill `UPDATE`;
  `issues.number BIGINT` with `NOT NULL` default 0 plus a per-project unique index and a backfill by
  `first_seen`; and `api_tokens (id, user_id, label, token_hash, token_prefix, created_at,
  last_used_at, revoked_at)` with a unique index on `token_hash` and an index on `user_id`.
  The in-memory `Store` implements the same invariants and is backfilled on construction.
- **Rationale**: Constitution requires idempotent migrations and forbids drift between the two
  representations; feature 001 already ships migrations without a wired adapter.
- **Alternatives considered**: Deferring migrations until the PostgreSQL adapter lands — rejected:
  schema decisions made twice risk divergence.

## R14. Testing strategy

- **Decision**: In-crate tests, all through `#[actix_web::test]` with a helper that builds the app
  with `compat_enabled = true`:
  1. Projection: status mapping, `shortId`, timestamps, `metadata`, `statusDetails`, `permalink`.
  2. Auth: uniform `401` for missing/malformed/unknown/revoked; `403` for scope; `404` for
     cross-project; live role removal between requests.
  3. List: default page of 100, `limit` clamps, all six sort values, every supported query operator,
     ignored unsupported operators, `hashes` on fingerprint.
  4. Pagination: 1,000 issues traversed exactly once; query-bound mismatch is `400`; exhausted cursor
     empty with `results="false"`; `X-Hits` totals.
  5. Mutation: status applied to ids in-project; out-of-project ids skipped; unsupported attribute
     `400`; empty body `400`; state history names the token owner.
  6. Tag values: distinct values, contains filter, empty values omitted, cap, issue-scoped path.
  7. Unsupported: empty lists, `404` detail, `501` mutations, `DELETE issues` `501`.
  8. Toggle: every path `404` when disabled.
  9. Native tokens: create returns plaintext once, list omits hashes, revoke is immediate, org admin
     revocation works, audit rows written.
- **Rationale**: Constitution V and SC-003 require the real request path; the list above maps
  one-to-one onto acceptance scenarios and edge cases so nothing is verified only in a unit test.
- **Alternatives considered**: Testing `projection`/`query` as pure functions only — kept as
  supplementary `#[cfg(test)]` modules, but never as the sole evidence.

## Resolved specification gaps

| Gap | Resolution |
|---|---|
| `hashes` parameter semantics | Filters on Scout `fingerprint` (R6). |
| `shortId` / `permalink` format | `{PROJECT}-{number}` and `{self_url}/#/issues/{id}` (R5). |
| `is:unresolved` scope | Open only, per FR-012 (R6). |
| Exact `count` JSON type | Number; isolated to the projection (R5). |
| Rate limiting | None in this slice; documented absence (clarification 4). |
