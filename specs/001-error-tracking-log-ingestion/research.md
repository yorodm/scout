# Phase 0 Research: Scout — Self-Hosted Error Tracking & Log Ingestion

**Feature**: `001-error-tracking-log-ingestion` | **Date**: 2026-10-31 | **Plan**: [plan.md](./plan.md)

All items below resolve the Technical Context. There are no remaining `NEEDS CLARIFICATION` markers.

---

## R1. Sentry-compatible ingest surface

**Decision**: Support the Sentry envelope endpoint (`POST /api/{project}/envelope/`) and the legacy
event endpoint (`POST /api/{project}/store/`), plus a small structured-log endpoint
(`POST /api/{project}/logs/`). Accept envelope items of type `event`, `attachment`, and `log`; count
other item types as ignored and continue processing the rest. Accept both newline-delimited and
length-prefixed envelope items, `gzip` content encoding, and all three Sentry key presentations:
`X-Sentry-Auth`, `Authorization: Sentry …`, and `?sentry_key=`.

**Rationale**: These are the payload shapes unmodified Sentry SDKs already emit; accepting them is the
product's core promise (FR-001, FR-002). The log endpoint is the documented fallback from the spec's
assumption, and it also accepts Sentry `log` items through the envelope route.

**Alternatives considered**: Sentry read-API compatibility (explicitly a non-goal); a bespoke SDK
(rejected: it would require client changes); accepting only envelopes (rejected: older SDKs and simple
shippers still use `/store/`).

**Resolution of unknowns**: Envelope framing, item types, compression, and auth presentation are
defined in [contracts/ingest.md](./contracts/ingest.md).

## R2. Metadata store: PostgreSQL with sqlx

**Decision**: Use PostgreSQL 16 with `sqlx` migrations and runtime-checked queries. Keep the existing
`Store` API as a trait with two adapters: PostgreSQL for the running service and an in-memory adapter
for the in-crate test suite. Migrations run at startup under a PostgreSQL advisory lock.

**Rationale**: The spec requires durable metadata, transactional durability before acknowledgement,
and concurrent-safe startup. `sqlx` keeps SQL explicit and supports migrations without an ORM. A
trait boundary lets the existing `actix_web::test` suite run without Docker while the production
adapter is exercised by the compose smoke test.

**Alternatives considered**: Diesel (more machinery and compile-time schema coupling); an embedded
database (does not satisfy the self-hosted multi-process expectation or the spec's metadata-store
requirement); keeping the in-memory store in production (violates FR-008 durability).

## R3. Raw payload storage: S3-compatible, write-once

**Decision**: Use the `object_store` crate with the S3 backend, configured by endpoint, bucket,
region, and credentials, so AWS S3 and MinIO share one adapter. Store each received event as one
immutable object at `projects/{project_id}/events/{event_id}` and each attachment at
`projects/{project_id}/events/{event_id}/attachments/{attachment_id}`. Create objects with a
create-only mode; never overwrite. Retention deletes objects; nothing else deletes or rewrites them.

**Rationale**: Satisfies FR-002, FR-034, and the "payload volume out of the database" constraint.
Create-only writes make immutability a storage-level guarantee rather than a code convention.

**Alternatives considered**: PostgreSQL large objects (bloats the metadata store and complicates
backup/retention); local filesystem (breaks the single-deployment object-store contract); storing the
whole envelope per event (duplicates attachments and inflates retention cost).

**Byte-identity rule**: The object body is exactly the decompressed bytes received after transport
framing is removed. For `/store/` it is the request body; for envelopes it is the event item payload.
`GET /api/issues/{id}/raw` streams the object unchanged.

## R4. Durability ordering and job queue

**Decision**: On ingest, write the raw object first, then insert the event row and a `group_event` job
in one PostgreSQL transaction, then acknowledge. If the object write fails or the transaction fails,
return `503` with a retryable error and do not acknowledge. The worker claims jobs with
`SELECT … FOR UPDATE SKIP LOCKED`, groups the event, and only then enqueues classification.

**Rationale**: FR-008 requires acknowledgement only after durable storage, and FR-009/FR-018 require
ingest never to wait on the classifier. The raw object is the source of truth, so writing it before
the row means a crash leaves an orphaned object at worst, never an acknowledged event without bytes.

**Alternatives considered**: Acknowledge then enqueue asynchronously (risks loss); a message broker
(the architecture explicitly avoids deploying one); writing the row first (a crash would leave a row
pointing at no payload). Orphaned objects are cleaned by a periodic reconciliation job.

**Prototype delta**: The current worker uses an in-memory channel. It remains the test-only path; the
PostgreSQL job table becomes the production path.

## R5. Grouping identity

**Decision**: Use the sender's `fingerprint` when present. Otherwise compute SHA-256 over the
exception type/value chain plus up to five non-`in_app: false` stack frames (filename, function,
lineno), salted with the project id. If no exception or frames exist, hash the normalised message
(lowercased, digit runs collapsed, whitespace collapsed). Store the fingerprint on the event and the
issue so grouping can be recomputed from stored payloads.

**Rationale**: Deterministic and re-derivable (FR-004); normalising numbers prevents
`timeout after 3000ms` and `timeout after 8080ms` from becoming separate issues. The project salt
prevents cross-project fingerprint collisions from being meaningful.

**Alternatives considered**: Stack-trace-only hashing (misses message-only errors); exact message
hashing (over-groups); machine-learning grouping (non-deterministic, untestable, and contrary to the
"re-derivable" architecture driver).

## R6. Classifier boundary, redaction, and confidence

**Decision**: Send one `POST {endpoint}/v1/systemone` request with `{model, state, questions}`. The
`state` contains derived metadata (kind, message, title, exception type, level, release, environment,
receiving route, and a payload sample of at most 2 KB) after redacting keys whose names match a
case-insensitive sensitive list (`authorization`, `cookie`, `set-cookie`, `password`, `passwd`,
`secret`, `token`, `api_key`, `apikey`, `access_token`, `refresh_token`, `ssn`, `credit_card`,
`email`). Reject answers below the project's `confidence_threshold` (default 0.5). Use an 800 ms
timeout with at most one retry; any non-2xx, timeout, parse failure, unknown category, or late answer
leaves the item with no suggestion and never fails ingest.

**Rationale**: Implements FR-013 through FR-019 and the clarified privacy boundary. Redaction happens
before serialisation so sensitive values never enter the request. The confidence threshold is
per-project so an owner can tune precision without a redeploy.

**Alternatives considered**: Sending the raw payload (violates FR-014); sending metadata only (lower
classification quality); global threshold (cannot suit every project); unbounded retries (would extend
the incident window); storing a late suggestion (violates FR-019).

**Mock**: The prototype's built-in `/mock-jev` endpoint stays for demos and tests; it returns the same
`{choice, confidence}` shape.

## R7. Local authentication and invitations

**Decision**: Local accounts authenticate with email and password. Store Argon2id password hashes.
Issue opaque session tokens (256-bit random), store only their SHA-256 hash with an expiry (default 7
days) and rotate on sign-in. There is no public sign-up: an account is created by redeeming a
one-time invitation token, or as an organisation's initial admin created by the deployment operator
(R15). Invitation tokens are 256-bit random, stored hashed,
scoped to an organisation plus an optional project/role, and expire after 7 days. Admin-generated
links are shared out of band; Scout sends no email. At account creation, issue a set of single-use
recovery codes, stored hashed; redeeming one sets a new password without email.

**Rationale**: Implements the clarified FR-028/FR-030 and keeps the single deployment free of SMTP.
Hashing credentials and session/invite tokens means a database read does not yield usable secrets.

**Alternatives considered**: Email magic links (requires SMTP, rejected in clarification); storing
plaintext tokens (the current prototype does this; rejected for production); public self-sign-up
(conflicts with project-scoped access control and the operator-owned tenancy model); email-based
password reset (requires SMTP); operator-only password reset (leaves no self-service path).

**OIDC linking**: When an organisation has OIDC configured, authenticate via the authorisation-code
flow with PKCE and link to an existing account only when the provider's `email_verified` claim is true
and the email matches. If no account exists and invitations are required, sign-in fails with an
actionable "invitation required" message.

## R8. Retention and search

**Decision**: Each project has `retention_days` (1–3650, default 30). A periodic sweep (and the
per-request retention check already present in the prototype) deletes log lines, attachment rows,
and raw objects older than the cutoff, and clears each event's raw pointer (`raw_expired_at`) while
leaving event and issue metadata intact. List endpoints report `raw_available = false` once the payload
is gone. Unified search uses PostgreSQL: case-insensitive substring matching over issue
title/culprit/fingerprint and log message/source/level/attributes, ordered newest first and capped at
50 results per kind. JSONB `attributes` and `tags` carry GIN indexes for containment queries; results
are always filtered by `project_id` and the caller's membership.

**Rationale**: Satisfies FR-024, FR-033, and SC-007 without adding a search cluster. PostgreSQL keeps
the deployment at three services.

**Alternatives considered**: Elasticsearch/Loki (explicitly the split the product exists to remove);
keeping expired payloads accessible (violates retention); hard-deleting issues (loses triage history);
full-text ranking (deferred; can replace the matcher without changing the API).

## R9. UI delivery

**Decision**: Sycamore 0.9 client-side rendering, Tailwind CSS compiled by Trunk's built-in
`tailwind-css` asset pipeline, built to `ui/dist` and served by `actix-files` from the same binary.

**Rationale**: One Rust toolchain, no Node.js, and no second deployment, per the project constraints
and the clarify decision not to add a standalone Tailwind binary.

**Alternatives considered**: A JS framework (breaks the one-toolchain constraint); server-rendered
Askama/Leptos (viable but a larger rewrite of the existing UI); a separate static host (violates the
single-deployment unit).

## R10. Test strategy without a library split

**Decision**: Keep all backend tests inside the binary crate. Unit tests live beside their modules;
cross-cutting integration tests live in `src/tests.rs` behind `#[cfg(test)]`, using
`actix_web::test::init_service` and the in-memory store/object-store fakes. Add a compose-based
smoke script for the PostgreSQL + MinIO wiring. Do not add a `tests/` crate or a `lib.rs`.

**Rationale**: The project explicitly forbids refactoring the backend into a library. In-crate tests
can reach `crate::` internals while still exercising the real router, auth, ingest, grouping, and
classification paths. The compose smoke script covers the adapters that the in-memory suite cannot.

**Alternatives considered**: `tests/*.rs` integration crates (impossible without a library target);
requiring Docker for every test (slow and unavailable in some environments); testing handlers in
isolation (misses routing and authorisation).

## R11. Performance sizing

**Decision**: Plan for ≥500 events/s sustained and 1,000 events/s peak per 4 vCPU deployment, with p95
ingest acknowledgement ≤150 ms for 10 KB events and 95% list visibility ≤10 s.

**Rationale**: The spec defines visibility (SC-005) but not throughput. This target is consistent with
"a team's telemetry on its own infrastructure", fits a single Actix worker plus PostgreSQL/MinIO, and
gives load tests a concrete pass/fail line. It is a planning assumption, not a new requirement.

**Alternatives considered**: 10,000 events/s (would demand a broker, batched writes, and a separate
worker tier); best-effort with no target (makes SC-005 unverifiable under load).

## R12. Observability

**Decision**: Structured `tracing` logs with request id, project id, event id, and job id; counters for
accepted/rejected events, grouping latency, classifier outcomes (suggested/skipped/failed), job
attempts, and retention deletions; a `/api/health` endpoint that reports database and object-store
reachability. Admin actions (role, key, model, retention changes) are recorded in an `audit_log` table.

**Rationale**: The spec does not mandate metrics, but operational readiness and the deferred
observability item require at least a health signal and an audit trail for privileged changes.

**Alternatives considered**: OpenTelemetry/Prometheus exporters (deferred; can be added without
changing the logging shape); no audit trail (insufficient for team access administration).

## R13. Ingest backpressure

**Decision**: Apply one global in-flight concurrency cap to ingest requests. Requests that would
exceed the cap are rejected immediately with `429` and a `Retry-After` header rather than waiting or
queuing without bound. The cap is configured by the operator with a generous default (sized so normal
traffic at the R11 target does not trip it) and is not a per-project quota.

**Rationale**: A single cap protects PostgreSQL and object storage from a runaway SDK or a burst
that would otherwise exhaust connections and memory, while keeping the first slice simple. `429` with
`Retry-After` is the retry-safe signal SDKs already understand, and it never counts as an accepted
event (FR-035, SC-010). A per-project quota would add policy, billing-like state, and UI that the
spec does not require.

**Alternatives considered**: No limit and rely on a reverse proxy (leaves the default deployment
exposed); global queueing with `503` (hides saturation and risks unbounded latency); per-project rate
limits (more policy than the first slice needs); per-project daily quotas (not a rate control and
creates a hard data-loss boundary).

---

## R14. Triage states and categorisation decisions

**Decision**: Support `open`, `resolved`, and `ignored` issue states. Every change appends an
`issue_state_changes` row with the actor and time. A confirm/override decision is authoritative
*metadata*: it determines the item's displayed category and the `category` list filter, but it never
moves, regroups, or deletes the stored event, issue, count, or raw payload. A suggestion is applied
only before the issue is first presented and before any human decision.

**Rationale**: `ignored` lets a triager silence noise without claiming a fix, matching common
error-tracker practice and the prototype. Keeping decisions as metadata preserves raw immutability
(FR-034), avoids destructive re-filing, and keeps counts and history trustworthy while still making
the human decision authoritative (FR-015, FR-017, FR-023).

**Alternatives considered**: Only open/resolved (loses a useful triage action); re-filing the stored
item between issue/log/raw storage (destructive, rewrites grouping and counts, and contradicts the
immutable-payload principle); treating the suggestion as the outcome (violates FR-015).

---

## R15. Operator provisioning and first organisation

**Decision**: At startup, read `SCOUT_OPERATOR_EMAIL` and `SCOUT_OPERATOR_PASSWORD`. If no user has
`is_operator = true`, create one with an Argon2id password hash and the flag. If an operator already
exists, ignore the environment values and do not reset the password; log a warning that the bootstrap
variables were ignored. The operator authenticates through the normal local login and is not scoped
to any organisation. It can create an organisation with `POST /api/orgs`, which also creates (or
reuses by email) the organisation's initial admin account and returns a one-time setup token using
the invitation token mechanics. The initial admin redeems the token to set a password and receive
recovery codes, then holds the `org_admin` role. The operator cannot create any other accounts;
everyone else joins by invitation. Passwords and tokens are never logged.

**Rationale**: Implements FR-025, FR-028, FR-030, and SC-011 without SMTP or a CLI. Idempotent
seeding avoids surprising password resets; returning a setup token avoids the operator choosing or
knowing the admin's permanent password; least privilege keeps the operator out of tenant data unless
explicitly added to a project.

**Alternatives considered**: First-run setup page (more UI state and a bootstrap-exposure window);
operator CLI (extra surface and container exec); seeded default credentials (unsafe); resetting the
password from the environment on every start (can lock out an operator who changed it); letting the
operator create arbitrary accounts (more privilege than needed).

**Security notes**: Supply the bootstrap values as deployment secrets. The password is hashed
immediately and never logged. Setup tokens are single-use, hashed at rest, and expire after 7 days.
Operator actions (organisation creation, initial-admin setup) are written to `audit_log`.

---

## Resolved `NEEDS CLARIFICATION` checklist

| Topic | Resolution |
|-------|------------|
| Throughput / scale target | R11: 500 events/s sustained, 1,000 peak |
| Exact classification request shape | R6 + contracts/classifier.md |
| Object-store backend and immutability | R3: `object_store`, create-only writes |
| Session/invitation token storage | R7: hashed, expiring |
| Search implementation | R8: PostgreSQL `ILIKE` + JSONB GIN |
| Test placement without a library | R10: in-crate `src/tests.rs` |
| Observability floor | R12: structured logs, health, audit log |
| Ingest backpressure | R13: global in-flight cap, `429` + `Retry-After` |
| Triage states and decision semantics | R14: open/resolved/ignored; decisions are metadata-only |
| Search matching rules | R8: case-insensitive substring, newest first, 50 per kind |
| Operator provisioning and first organisation | R15: idempotent env-seeded operator; setup token for the initial admin |
