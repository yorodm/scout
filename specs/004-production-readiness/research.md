# Phase 0 Research: Production Readiness

All decisions below were derived from the current code, the feature specification, the project
constitution (v1.1.0), and the schema already present in `migrations/0001`–`0007`. No decision adds a
product behaviour beyond what features 001 and 002 ratified.

---

## R1 — Persistence architecture

**Decision**: PostgreSQL is the system of record. `src/store.rs` becomes `src/store/` with:

```rust
pub enum Store {
    Memory(Arc<RwLock<MemoryStore>>), // tests and SCOUT_STORE=memory only
    Postgres(PgStore),                // sqlx::PgPool
}
```

with asynchronous repository methods (`find_user_by_email`, `create_project`, `insert_event`,
`claim_job`, …) implemented once per variant. `AppState.store` becomes a cloned `Store` instead of
`Arc<RwLock<Store>>`; handlers, ingest, the worker, `audit.rs`, and `compat/` call repository methods.

**Rationale**: Every alternative that keeps the in-memory maps as the read model makes correctness
depend on cache invalidation. Two behaviours the specification treats as immediate would break:

- role/key revocation while a session or API token is active (FR-008, FR-014, SC-006);
- acceptance-to-visibility within 10 seconds across instances that did not handle the ingest (SC-010).

A per-request refresh of authorization tables would recover the first and cost an extra query per
request; a full reload would recover the second and is impossible at 500 events/s. Direct reads cost
the same indexes PostgreSQL already has (`projects_by_dsn`, `issues_project_fingerprint`, …), so the
simpler model also wins on latency.

The `Store::Memory` variant is not dead code: it is how the 4,000-line in-crate test suite keeps
running without a database, and it is the documented `SCOUT_STORE=memory` development mode (FR-002).

**Alternatives considered**:

- *Write-through cache over the existing maps* — smallest diff, but stale reads on every other
  instance and a second source of truth to keep in sync. Rejected.
- *Keep `Arc<RwLock<Store>>` and persist a snapshot per mutation* — catastrophic write amplification
  and no cross-instance coherence. Rejected.
- *Full ORM / query builder* — a new dependency with no benefit over hand-written SQL against a schema
  that already exists. Rejected.

**Consequence**: the storage seam is the largest slice of this feature. It is executed domain by
domain (identity → projects/keys → telemetry → audit/jobs) so each slice compiles and passes the
suite before the next starts.

---

## R2 — Migration execution and version skew

**Decision**: `src/schema.rs` owns startup schema work:

1. Connect the pool, with bounded retry (≈30 s) so a starting database is tolerated.
2. If `_sqlx_migrations` exists, compare `MAX(version)` with the highest embedded migration version.
   If the stored version is greater, abort with:
   `database schema version N is newer than this build (M); deploy a matching build or restore a backup`
   (FR-004, SC-002).
3. Run the embedded migrator (`sqlx::migrate!("./migrations")`, or `Migrator::new(Path)` if the
   `macros` feature is required to be added). SQLx takes a PostgreSQL advisory lock, so concurrent
   starts serialize and exactly one applies each migration (FR-003, SC-003).
4. Map every migration error to an actionable startup error and exit non-zero; never continue serving
   (FR-004).

**Rationale**: The migration files already exist but have never been executed by the application; the
`_sqlx_migrations` table gives both the idempotency record and the version signal for downgrade
detection without inventing a parallel version table. Embedding keeps the single-binary promise; the
Dockerfile already copies `migrations/`, so both paths work, but embedding removes a runtime path
dependency.

**Alternatives considered**: a hand-rolled runner reading `migrations/` (reinvents advisory locking and
version bookkeeping); `migrate` subcommand only (violates FR-003 "no manual database step"); automatic
down-migrations on version skew (explicitly rejected by the clarification: rollback is operator-driven).

---

## R3 — Startup ordering and health

**Decision**: startup is a fixed pipeline, and the listener binds only after every step passes:

```text
config → validate storage → connect pool → schema check + migrate → bootstrap operator
       → object store verify_writable → bind :8080
```

`GET /api/health` reports `status`, `storage` (`ok|unavailable`), `object_store`
(`ok|unavailable|<backend>`), `store` (`postgres|memory`), and `durability` (`durable|ephemeral`).
It probes PostgreSQL with `SELECT 1` under a 2 s timeout and keeps the existing object-store probe.
Either dependency failing yields `503` and `status: "degraded"` (FR-005, SC-004).

**Rationale**: FR-005 wants one signal that covers both dependencies, and FR-002 wants an operator to
notice an ephemeral store immediately; `durability: "ephemeral"` plus the startup warning covers both
without new configuration. Binding late means a failed schema can never accept ingest.

**Alternatives considered**: separate `/api/health/live` + `/api/health/ready` (compose already checks
one endpoint; splitting adds surface without a consumer); a metrics-only health signal (compose's
healthcheck needs an HTTP status).

---

## R4 — Durable job queue

**Decision**: ingest inserts a `jobs` row (`kind = 'group_event'`, `state = 'pending'`,
`available_at = now()`) in the same transaction as the event. The worker loop claims with:

```sql
UPDATE jobs SET state = 'running', locked_at = now(), attempts = attempts + 1
WHERE id = (SELECT id FROM jobs
            WHERE state = 'pending' AND available_at <= now()
            ORDER BY available_at, created_at
            FOR UPDATE SKIP LOCKED
            LIMIT 1)
RETURNING id, kind, project_id, event_id;
```

Grouping then upserts the issue on `(project_id, fingerprint)` with counter and `last_event_id`
updates, links the event, and completes the job. Failures set `state = 'failed'`, `last_error`, and
`available_at = now() + backoff` until `attempts` reaches a small bound. The classification job is
enqueued only after grouping commits.

**Rationale**: FR-006 requires that a restart neither loses queued work nor processes an item twice.
`SKIP LOCKED` is exactly the multi-consumer primitive the schema was designed for
(`jobs_claim_idx`), and it needs no broker (constitution IV). Idempotency additionally rests on
`events (project_id, sentry_event_id)` uniqueness and the issue fingerprint upsert, so a retried job
converges instead of double-counting.

**Alternatives considered**: keeping the in-process `mpsc` channel (loses queued work on restart — the
current bug); a LISTEN/NOTIFY wake-up (a latency optimization, not required, and it complicates a
single-worker deployment).

---

## R5 — Ingest durability ordering

**Decision**: per event: (1) write the raw object with `put_create` (write-once), (2) commit one
PostgreSQL transaction containing the event row, attachment metadata rows, and the grouping job, (3)
acknowledge. Any failure before step 2 returns the existing retryable `503`; an object orphaned by a
failed transaction is left to object retention rather than deleted inline (a delete could race a
retry).

**Rationale**: FR-009 and the constitution require acknowledgement only after both raw bytes and
metadata are durable. Keeping the object write first means a retried event never references bytes that
do not exist; the `UNIQUE (project_id, sentry_event_id)` constraint makes the retry safe.

**Alternatives considered**: metadata first, object second (an acknowledged event could point at
missing bytes); writing the object inside the DB transaction (impossible across services).

---

## R6 — Demo isolation

**Decision**: migration `0008` adds `organisations.is_demo boolean NOT NULL DEFAULT false`. The seed
marks its organisation as demo. Behaviour:

- demo sign-in shortcuts resolve a session whose only membership is the demo organisation;
- `GET /api/demo/accounts` lists only demo-organisation members;
- `POST /api/demo/reset` replaces only demo-organisation rows (projects, issues, events, logs, keys,
  memberships, sessions of demo users) and never touches the operator or other organisations;
- every project-scoped route denies a demo session when the target project belongs to a non-demo
  organisation, returning `404` exactly as for an inaccessible project (FR-011, SC-014).

**Rationale**: today `reset_demo` replaces the whole store (`*state.store.write().await = fresh`), which
on a deployment with real accounts destroys every record. Confinement is enforced at the store layer
(project scope is already resolved per request), so the rule is structural rather than a UI check.

**Alternatives considered**: refusing demo mode once a real account exists (breaks evaluation on a
shared instance); a separate database for demo data (new moving part).

---

## R7 — Store selection and warnings

**Decision**: `Config.store_kind` is resolved as:

| `SCOUT_STORE` | `DATABASE_URL` | Result |
|---|---|---|
| `postgres` | set | PostgreSQL |
| `postgres` | unset | startup error naming both variables |
| `memory` | any | in-memory with a loud warning |
| unset | set | PostgreSQL (shipped compose path) |
| unset | unset | in-memory with a loud warning |

When the memory store is active, startup logs `WARN scout is using the in-memory metadata store:
records will not survive a restart` and `/api/health` reports `durability: "ephemeral"` (FR-002).

**Rationale**: the compose stack always sets `DATABASE_URL`, so the durable path needs no new variable
and cannot regress silently; an explicit `SCOUT_STORE=memory` is itself the acknowledgement that the
run is local and throwaway. Choosing the adapter from one variable keeps the rule "one documented
setting" from the clarification, and the warning plus the health field make an accidental ephemeral
deployment obvious without a second flag.

**Alternatives considered**: compiling the in-memory adapter out of release builds (two compilation
profiles and a test-harness fork); a boolean `SCOUT_PERSIST=1` (ambiguous with object-storage flags); a
second acknowledgement variable (contradicts "one documented setting").

---

## R8 — OIDC integration

**Decision**: `src/oidc.rs` implements optional per-organisation OIDC with the `openidconnect` crate:

- **Configuration** (organisation admin): issuer URL, client id, client secret, optional scopes
  (default `openid email profile`). Discovery document fetched and cached at save time so an invalid
  issuer fails fast. Stored in `oidc_connections` (migration `0008`).
- **Sign-in**: `GET /api/oidc/{org_slug}/start` → authorization-code + PKCE-S256 redirect with `state`
  and `nonce`; the flow row is persisted in `oidc_login_flows` (10-minute TTL, single use) so any
  instance can complete it. `GET /api/oidc/callback` validates `state`, exchanges the code, verifies
  the ID token signature, issuer, audience, expiry, and nonce.
- **Matching**: only a verified email matching an existing membership in that organisation signs in
  (FR-018, SC-007). No user is created, no role is granted, and an unverified or unknown email returns
  the same denial message. Local sign-in is untouched.
- **Removal**: deleting the connection leaves local accounts unchanged and immediately stops
  provider sign-in (US3/AC4).
- **Secrets**: the client secret must be replayed to the provider, so it is stored in
  `oidc_connections.client_secret`, never serialized into a response, and redacted in logs. The
  exception is recorded in the plan's Complexity Tracking.

**Rationale**: `openidconnect` is already a declared but unused dependency, so the protocol work is
validation and flow plumbing rather than crypto. Persisting flows enables multi-instance completion
and keeps PKCE verifiers out of cookies.

**Alternatives considered**: signed-cookie-only flows (state spread across instances breaks
verification); matching on provider subject (the specification and constitution mandate verified email);
creating accounts on first SSO login (explicitly forbidden by US3/AC2).

---

## R9 — Operational counters

**Decision**: `src/metrics.rs` holds `AtomicU64` counters: `ingest_accepted`, `ingest_rejected`
(reason-labelled), `grouping_ok`, `grouping_failed`, `classifier_suggested`,
`classifier_no_suggestion`, `classifier_failed`, `retention_objects_deleted`, `retention_logs_deleted`,
`raw_expired`. `GET /api/metrics` returns JSON to an authenticated operator (FR-025). Counters are
incremented in-process on the existing code paths and are also emitted as periodic `INFO` log lines so
a log-only deployment sees them.

**Rationale**: counters on the hot path must not add a round trip, and the deployment already ships no
monitoring backend, so a JSON document is the smallest useful signal. Operator-only avoids publishing
tenant volume to anonymous callers.

**Alternatives considered**: Prometheus text format (no scraper in the stack, and the format adds a
dependency-free but unnecessary contract); a `metrics` crate (new dependency for atomics).

---

## R10 — Retention verification

**Decision**: retention keeps its hourly default and gains `SCOUT_RETENTION_INTERVAL_SECONDS`
(default `3600`). `scripts/compose-smoke.sh` ages data honestly by running SQL inside the shipped
database container (`UPDATE events SET received_at = now() - interval '40 days' WHERE …` and the same
for `log_lines`), restarts Scout with a 2-second interval, then verifies through the API that the raw
payload reports the expired state while the issue and its counters remain.

**Rationale**: SC-009 requires the smoke path to exercise retention, but retention only deletes data
older than the project window, and ingest always timestamps rows with `now()`. Backdating is a test
fixture, not a product feature, and it avoids adding an "expire now" administrative endpoint that could
be misused. The interval knob is a legitimate operator setting in its own right.

**Alternatives considered**: an operator-only "run retention now" endpoint (new destructive surface);
a fake clock in production code (untestable complexity for one scenario).

---

## R11 — UI completion approach

**Decision**: extend the existing Sycamore components; no router crate and no new dependency. Views are
selected in `ui/src/components/app.rs` as today, with role-aware navigation. New screens reuse
`common.rs` primitives for loading, empty, and error states (FR-020):

- real email/password sign-in, recovery-code reset, invitation acceptance, initial-admin setup token
  redemption;
- operator: organisation creation with one-time setup token display;
- organisation admin: member invitations and role changes;
- project owner: key create/rotate/revoke, model endpoint + threshold + retention form, OIDC
  configuration;
- triage: occurrence timeline, raw-expired state, suggestion vs. decision card, issue filters and
  unified search.

**Rationale**: the components already exist and already call most of the endpoints; the gaps are
screens and states, which the feature's US2–US4 acceptance scenarios describe directly. Adding a
router would churn `app.rs` without helping verification.

**Alternatives considered**: introducing `sycamore-router` (new dependency, larger diff); server-side
templates (abandons the ratified UI stack).

---

## R12 — Release evidence

**Decision**: two POSIX shell scripts using `curl` and `awk` (no new runtime dependency):

- `scripts/compose-smoke.sh` — `docker compose up -d --build --force-recreate`, wait for health,
  operator sign-in, organisation + initial admin, setup-token redemption, project creation, event and
  log ingest, raw retrieval, retention, restart, re-read, compatibility path check, demo-confinement
  check; writes a dated Markdown result file.
- `scripts/load-ingest.sh` — generates 10 KB envelopes at a target rate, records accepted rate, p95
  acknowledgement latency, and time-to-visibility, and compares them with SC-010.

Both write into `specs/004-production-readiness/evidence/`, which is committed. Neither runs as a
per-change CI gate (FR-024).

**Rationale**: the specification demands repeatable, committed evidence rather than a CI gate, and the
workspace has no build infrastructure for performance hardware. Shell keeps the scripts readable for a
self-hosting operator.

**Alternatives considered**: a Rust load harness (another binary target, against "one binary"); a
hosted CI job (no runner with the reference profile).
