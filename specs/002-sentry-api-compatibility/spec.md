# Feature Specification: Sentry Read-API Compatibility

**Feature Branch**: `spec/sentry_compatibility`

**Created**: 2026-11-01

**Status**: Blocked on constitution amendment (see Governance Note)

**Input**: User description: "Compatibility with sentry openapi spec https://raw.githubusercontent.com/getsentry/sentry/refs/heads/master/api-docs/openapi.json"

## Governance Note *(blocking)*

The ratified constitution (v1.0.0), under **Additional Constraints → Compatibility and Scope**,
states: *"Scout supports Sentry **ingestion** only. Sentry read-API compatibility, metrics, and
tracing are out of scope and MUST NOT be implied."*

This feature deliberately reverses that boundary. It MUST NOT move to `/speckit-plan` until the
constitution is formally amended (for example to v1.1.0) to replace that clause with the
compatibility boundary defined here. The governance amendment is an explicit, separate decision: it
is not implied by this specification.

## Purpose and scope

Existing Sentry API clients, scripts, dashboards, and integrations should be able to read and triage
Scout data by changing only their base URL and credentials. The compatibility contract is the Sentry
Public API document referenced above (OpenAPI 3.0.1, version `v0`, 11 paths across the Organizations,
Projects, Events, Releases, Integration, and Teams tags; the Teams and SCIM tags define no paths in
that document).

The compatibility surface is additive: Scout's existing `/api` dashboard API, `/api/{project}/*`
ingest endpoints, authentication, and UI remain unchanged. Sentry compatibility lives under
`/api/0/`.

Endpoints whose underlying Sentry feature has no Scout equivalent are **not** emulated with invented
data. They return documented, Sentry-shaped responses (empty collections where a read can be
honestly satisfied, otherwise an explicit unsupported response) and never mutate state silently.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read and triage a project's issues with a Sentry client (Priority: P1)

A developer points an existing Sentry API client or script at their Scout deployment, supplies a Scout
API token, and lists a project's issues using the Sentry paths. The returned objects look like
Sentry's issue objects closely enough that the client's existing parsing, filtering, and display keep
working. They then resolve and re-open issues through the same client.

**Why this priority**: Issue listing and status changes are the core read/triage loop. Without them,
"compatibility" delivers no practical value.

**Independent Test**: Configure a Sentry-shaped client with `base_url=https://scout.example` and a
Scout API token; list issues, filter by status/level/release, page through results, resolve one issue,
and confirm the change appears in Scout's UI and state history.

**Acceptance Scenarios**:

1. **Given** a project with 1,000 issues, **When** the client requests
   `GET /api/0/projects/{org}/{project}/issues/` without parameters, **Then** it receives the newest
   100 unresolved issues as Sentry-shaped objects, a `Link` header with a next cursor, and an
   `X-Hits` header with the total match count.
2. **Given** the same project, **When** the client requests `?query=is:resolved&sort=freq&limit=25`,
   **Then** only resolved issues are returned, ordered by event count, at most 25 of them.
3. **Given** two issues, **When** the client sends `PUT .../issues/?id=<a>&id=<b>` with
   `{"status": "resolved"}`, **Then** both are resolved in Scout, each gains a state-history entry
   naming the token's owner, and the response contains the updated issue objects.
4. **Given** an issue resolved through the Sentry API, **When** the user opens Scout's issue detail,
   **Then** the state and history match what the client submitted.

---

### User Story 2 - Browse a project's and an issue's tag values (Priority: P2)

A user lists the values of a tag key (for example `release`, `environment`, or a custom tag) for a
project or for a single issue, optionally filtering by a contains match.

**Why this priority**: Tag values drive the common "filter by tag" workflows in Sentry tooling and are
the only other read surface in the document that maps directly to stored Scout data.

**Independent Test**: Seed issues with tags, request `GET /api/0/projects/{org}/{project}/tags/release/values/`
and the equivalent issue-scoped path, and verify distinct values, contains filtering, and pagination.

**Acceptance Scenarios**:

1. **Given** a project whose issues carry `release` values `web@1.0.0` and `web@1.1.0`, **When** the
   client lists that tag's values, **Then** it receives `[{"name": "web@1.0.0"}, {"name": "web@1.1.0"}]`
   in a stable order.
2. **Given** the same data, **When** the client adds a contains filter for `1.1`, **Then** only
   `web@1.1.0` is returned.
3. **Given** more values than one page, **When** the client follows the cursor, **Then** it receives
   every distinct value exactly once and a final page with `results="false"`.

---

### User Story 3 - Authenticate with Sentry-style tokens and scopes (Priority: P3)

An administrator creates a long-lived API token for a person or automation. Sentry clients present it
as `Authorization: Bearer <token>`. Requests are authorised by the token owner's current project roles,
and insufficient scope is rejected without revealing whether a resource exists.

**Why this priority**: Sentry clients expect a static bearer token rather than a short-lived browser
session. Scoping keeps the compatibility surface inside Scout's project-authorisation model.

**Independent Test**: Create a token for a viewer, call a read endpoint successfully, attempt a status
mutation and receive `403`, revoke the token, and confirm the next call returns `401`.

**Acceptance Scenarios**:

1. **Given** an authenticated user, **When** they create an API token, **Then** the plaintext token is
   shown once, only a hash is stored, and the token's metadata shows a prefix for identification.
2. **Given** a read token owned by a viewer, **When** it requests a project's issues, **Then** the
   request succeeds and a status mutation through the same token returns `403`.
3. **Given** a token whose owner is removed from the project, **When** the token is used for that
   project, **Then** the request is denied immediately without reissuing the token.
4. **Given** a revoked, unknown, or malformed token, **When** any `/api/0/` request is made, **Then**
   the response is `401` with the same body, and a project the owner cannot access is `404`.

---

### User Story 4 - Predictable behaviour for Sentry features Scout does not have (Priority: P4)

An existing client calls an endpoint for a Sentry subsystem Scout does not implement (releases' commit
files, legacy user reports, service hooks, spike protection, Sentry app installations, external
issues). The client receives a documented, Sentry-shaped answer — an empty collection where that is
honest, otherwise an explicit unsupported response — instead of a generic error page or fabricated
data.

**Why this priority**: Compatibility is judged by how tools behave across their whole workflow. A
deterministic, documented response prevents retries, crashes, and false assumptions, while preserving
the rule that Scout never invents data.

**Independent Test**: Call each path in the referenced document and confirm the documented full or
unsupported behaviour, with every error using Sentry's `detail` shape.

**Acceptance Scenarios**:

1. **Given** a known release, **When** the client requests its commit files, **Then** it receives an
   empty list; an unknown release returns `404`.
2. **Given** no integrations, **When** the client lists service hooks or app installations, **Then** it
   receives an empty list.
3. **Given** a request to create a service hook, submit legacy feedback, change spike protection, or
   create an external issue, **When** it is made, **Then** it receives `501` with a `detail` message
   naming the unsupported feature and no data changes.
4. **Given** a request to permanently delete issues, **When** it is made, **Then** it receives `501`
   documenting that Scout preserves issue history and raw payloads.

### Edge Cases

- Base paths and collection paths are requested with and without a trailing slash; both resolve.
- `limit` is absent (default 100), zero, negative, above 100, or non-numeric.
- A cursor is malformed, references a different query, or points past the last page.
- `query` contains unsupported Sentry search tokens (for example `assigned:me`, `bookmarks:me`, or
  wildcard searches); the request must remain usable and the limitation must be visible.
- `id` is repeated for issues in another project, or omits every id while mutating status.
- An organisation or project is addressed by an id, a slug, or a numeric DSN id; unknown and
  soft-deleted projects return `404` without leaking existence.
- The token owner's role changes or is revoked between requests.
- Tag values contain duplicates, empty strings, non-string values, or exceed one page (1,000 cap).
- A project has zero issues or a tag key has no values.
- Two clients page through the same project while new issues arrive.
- The compatibility layer is disabled, or `/api/0/` is called with a session token instead of an API
  token.

## Requirements *(mandatory)*

### Functional Requirements

**Surface and authentication**

- **FR-001**: Scout MUST expose a Sentry-compatible API under `/api/0/` alongside its existing `/api`
  surface, without changing existing endpoints, payloads, or the UI.
- **FR-002**: Every `/api/0/` request MUST authenticate with a Scout API token presented as
  `Authorization: Bearer <token>`; missing, malformed, unknown, and revoked tokens MUST all return the
  same `401` body.
- **FR-003**: A user MUST be able to create, list, and revoke long-lived API tokens; the plaintext
  token MUST be shown once, only a hash stored, and a stable prefix exposed for identification.
- **FR-004**: API tokens MUST inherit the owner's current project roles and organisation roles on every
  request, so role changes and removals take effect without reissuing or restarting.
- **FR-005**: Sentry scope names MUST be enforced from those roles: `event:read` and `project:read` for
  viewers, plus `event:write` for triagers, plus `project:write`, `project:admin`, `event:admin`, and
  `project:releases` for admins and owners. Organisation members MUST also receive `org:read`, and
  organisation admins MUST also receive `org:admin` and `org:integrations`. Requests above the granted
  scope MUST return `403`; resources the owner cannot access MUST return `404`.
- **FR-006**: Organisations MUST be addressable by identifier or slug; projects MUST be addressable by
  identifier, slug, or DSN id. Soft-deleted or unknown projects MUST return `404`.
- **FR-007**: When the compatibility surface is disabled, every `/api/0/` path MUST return `404`; it
  MUST be disabled by default and enabled by explicit configuration.

**Issues**

- **FR-008**: `GET /api/0/projects/{org}/{project}/issues/` MUST return issues as Sentry-shaped
  objects containing at least `id`, `shortId`, `title`, `culprit`, `status`, `statusDetails`,
  `level`, `count`, `userCount`, `firstSeen`, `lastSeen`, `metadata`, `permalink`, `project`, `tags`,
  and `annotations`.
- **FR-009**: Issue list MUST support `query`, `sort` (`date`, `new`, `freq`, `user`, `trends`,
  `recommended`), `limit` (default 100, maximum 100), `cursor`, repeated `id`, and `hashes`; it MUST
  accept and ignore `statsPeriod` and `shortIdLookup` without failing.
- **FR-010**: The supported query grammar MUST include `is:unresolved|resolved|ignored`, `level:<l>`,
  `release:<r>`, `environment:<e>`, `<tag>:<value>`, and free-text matching over title, culprit, and
  fingerprint. Unsupported operators MUST be ignored, with the limitation documented; they MUST NOT
  produce `400` or `500`.
- **FR-011**: `sort` values map as follows: `date` = last seen descending, `new` = first seen
  descending, `freq` = event count descending, `user` = event count descending (user counts are not
  tracked), while `trends` and `recommended` MUST fall back to `date` ordering.
- **FR-012**: Issue statuses MUST map as `unresolved` ↔ open, `resolved` ↔ resolved, `ignored` ↔
  ignored, in both directions; `statusDetails` MUST be present for every issue.
- **FR-013**: `PUT /api/0/projects/{org}/{project}/issues/` MUST apply `status` to every issue id that
  belongs to the project (and to every issue matching `status` when no id is supplied), recording one
  state-history entry per change with the token owner as actor. Ids outside the project MUST be skipped
  silently. Requests whose body contains no supported attribute MUST return `400`.
- **FR-014**: Supported mutation attributes are `status` only. Unsupported attributes (for example
  `assignedTo`, `isPublic`, `merge`) MUST be rejected with `400` naming the unsupported field; they
  MUST NOT be silently ignored.
- **FR-015**: `DELETE /api/0/projects/{org}/{project}/issues/` MUST return `501` explaining that issue
  history and raw payloads are preserved by Scout; it MUST NOT delete issues.

**Tag values**

- **FR-016**: `GET /api/0/projects/{org}/{project}/tags/{key}/values/` MUST return distinct values for
  that tag across the project's stored issues as `[{"name": "<value>"}]`.
- **FR-017**: `GET /api/0/organizations/{org}/issues/{issue}/tags/{key}/values/` MUST return the same
  shape restricted to that issue.
- **FR-018**: Tag-value responses MUST support `query` as a case-insensitive contains filter, MUST cap
  at 1,000 values, MUST omit empty values, MUST stringify non-string values consistently, and MUST
  return an empty list rather than `404` when the key has no values.

**Pagination, errors, and compatibility**

- **FR-019**: Issue collections MUST paginate with an opaque `cursor`, a `Link` header containing the
  next URL with `rel="next"`, `results="true|false"`, and `cursor="<next>"`, and an `X-Hits` header
  carrying the total match count. Default page size is 100; the maximum is 100.
- **FR-020**: Cursors MUST be stable for a fixed query and ordering: following all pages for a
  1,000-issue project MUST return every issue exactly once, and an exhausted cursor MUST return an
  empty page with `results="false"`.
- **FR-021**: Every `/api/0/` error MUST use Sentry's `{"detail": "<message>"}` envelope and appropriate
  statuses (`400`, `401`, `403`, `404`, `409`, `501`).
- **FR-022**: `GET /api/0/organizations/{org}/releases/{version}/commitfiles/` MUST return `[]` when the
  release is present in an accessible project and `404` otherwise.
- **FR-023**: `GET` on project service hooks, a service-hook detail, project user-feedback, and
  organisation app installations MUST return an empty list, or `404` for a specific hook; `POST` and
  `PUT`/`DELETE` on those surfaces, spike-protection mutations, and Sentry-app external-issue
  mutations MUST return `501` with a `detail` naming the unsupported feature.
- **FR-024**: The compatibility layer MUST NOT fabricate metrics, feedback, releases' commit data,
  hooks, integrations, or spike-protection state, and MUST NOT mutate Scout data outside the
  issue-status mapping in FR-013.
- **FR-025**: All `/api/0/` responses MUST be JSON with ISO-8601 UTC timestamps; ids MUST be strings.

### Key Entities *(include if feature involves data)*

- **API Token**: A long-lived credential owned by a user, stored as a hash with a display prefix, used
  as a Sentry bearer token; derives scopes from the owner's live roles and can be revoked.
- **Sentry Issue Projection**: The Sentry-shaped view of a stored issue, including synthetic `shortId`,
  `permalink`, `statusDetails`, `metadata`, `project`, and `userCount` (reported as 0 because Scout does
  not track distinct users).
- **Tag Value**: A distinct string value of a tag key, derived from stored issue tags, filtered and
  paginated for the Sentry tag-values endpoints.
- **Pagination Cursor**: An opaque, query-bound position used by `Link`/`cursor` pagination.
- **Scope**: A Sentry permission name derived from the token owner's project or organisation role at
  request time.
- **Compatibility Exception**: A documented endpoint category Scout intentionally does not implement,
  with its honest empty or `501` behaviour.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A representative Sentry API client (a script using the documented issue list, status
  mutation, and tag-value calls) completes its workflow against Scout with no code changes beyond the
  base URL and token.
- **SC-002**: 100% of the 11 documented paths return a documented response — full support, an honest
  empty collection, or an explicit `501` — with no unhandled `5xx` or HTML error page.
- **SC-003**: Every issue list, mutation, and tag-value response conforms to the documented field and
  error shapes; a compatibility test suite covering the document's examples passes 100%.
- **SC-004**: 100% of requests by tokens without the required scope are denied with `403`, and 100% of
  cross-project or inaccessible-resource requests are denied without revealing existence.
- **SC-005**: Following pagination over a 1,000-issue project returns each issue exactly once; no page
  ever exceeds 100 items.
- **SC-006**: Status changes made through the compatibility API are visible in Scout's issue list and
  state history within 1 second.
- **SC-007**: 95% of issue-list and tag-value requests for a 1,000-issue project complete within
  500 ms on the reference self-hosted deployment.
- **SC-008**: The existing native dashboard API, ingest endpoints, UI, and test suite continue to pass
  unchanged, and ingest performance is unaffected.

## Assumptions

- The referenced OpenAPI document (`v0`, 11 paths) is the full compatibility contract for this
  feature; the wider Sentry API, metrics, tracing, SCIM, and Teams endpoints are out of scope.
- The compatibility surface is an opt-in deployment setting, disabled by default, because it exposes
  project data to long-lived tokens and expands the attack surface.
- Scout API tokens are long-lived, hashed at rest, and inherit permissions from the owner rather than
  carrying an independent scope list; this favours live role enforcement over Sentry's per-token scope
  editor.
- Issue ids remain Scout's opaque identifiers; `shortId` is synthesised from the project and a stable
  issue number, and `permalink` points at the Scout UI detail route.
- `userCount` is always `0`, `annotations` is empty, and assignment/merge/bookmark features are not
  emulated; the values are present so clients can parse the objects.
- Tag values are derived from the tags stored on issues; Scout does not maintain a separate tag index
  in this slice, and a 1,000-value cap is accepted.
- Permanently deleting issues is intentionally unsupported because it conflicts with the
  raw-payload and history guarantees; this is documented in the compatibility matrix.
- Legacy user reports, service hooks, spike protection, Sentry apps, and external issues are
  subsystems Scout does not have; their endpoints return documented empty or `501` responses.
- The compatibility layer reuses Scout's existing organisation, project, issue, and role data; no
  parallel data model is created.
- The constitution amendment described in the Governance Note is a prerequisite, and this spec does
  not itself authorise changing the constitution.
