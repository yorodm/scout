# Implementation Plan: Production Readiness — Durable Metadata and Remaining Product Surfaces

**Branch**: `004-production-readiness` | **Date**: 2026-11-01 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/004-production-readiness/spec.md`

## Summary

Make PostgreSQL the system of record for every metadata entity the product owns, keep the in-memory
adapter only for automated tests and explicit local development, and finish the surfaces that still
exist only on paper: the operator/administrator dashboard flows, optional per-organisation OIDC,
the remaining triage/search/settings UI, operational counters, and one repeatable release-verification
path.

The technical core is a storage seam. `src/store.rs` becomes `src/store/` with a `Store` enum whose
two variants — `Memory` (today's maps plus `seed()`) and `Postgres` (`sqlx::PgPool`) — expose the same
asynchronous repository methods. Handlers, ingest, the worker, the audit helper, and the Sentry
compatibility layer stop touching maps behind `Arc<RwLock<_>>` and call those methods instead, so the
durable and in-memory paths cannot drift in behaviour. A new `src/schema.rs` runs the embedded
migration set at startup before the service reports ready and aborts startup when the engine fails.
The product keeps its migrations, but the upgrade process — planning, sequencing instances, rollback —
belongs to the operator, and this feature claims no concurrency or version-skew guarantees around it.

Everything else is completion work behind that seam: OIDC login (`openidconnect`, already a declared
but unused dependency), the operator/administrator screens, the remaining issue/log/settings views,
a JSON operational-counter endpoint, a durable job queue claim, and two shell scripts that produce
committed release evidence. No product behaviour ratified by features 001 and 002 changes, and the
Sentry surface stays opt-in and limited to its existing contract.

The second clarification session adds five operational constraints that shape the design: every running
instance reads metadata from the shared store on each request with no application-level cache
(FR-031, SC-016); a store failure fails fast rather than retrying in-process (FR-009); invitations and
initial-admin tokens expire seven days after issue (FR-013); operational counters are operator-only
(FR-025); and the audit trail remains storage-only, with no read endpoint or screen (FR-007).

The third session settles credential and data lifecycle: the operator can change its own password and
receives recovery codes once on first sign-in (FR-012, SC-017); retention follows the server's receipt
time rather than client timestamps (FR-010, SC-018); backup and restore are documented operator
procedures with no shipped tooling (FR-027, SC-019); disabling an account or removing a member
invalidates its sessions and tokens on the next request (FR-014, SC-020); and startup migrations run
before the service reports ready (FR-003, SC-021).

The fourth session hardens access and query semantics: passwords are at least 12 characters with no
composition rules (FR-011, SC-022); five consecutive failed sign-ins lock an account until the operator
unlocks it, with generic rejections throughout (FR-012, FR-032, SC-023); sessions expire after a
seven-day maximum and 12 hours idle (FR-033, SC-024); unified search returns at most 200 newest
matches with a `has_more` indicator (FR-021, SC-025); and only the operator creates projects, naming an
existing organisation member as the initial owner and receiving **no** ingest key or DSN in the
response (FR-012, FR-034, SC-026).

## Technical Context

**Language/Version**: Rust 2021 edition on a stable toolchain; UI in Sycamore 0.9 compiled by Trunk
(no Node.js).

**Primary Dependencies**: Actix Web 4, `sqlx` 0.8 with `postgres`/`migrate`/`uuid`/`chrono`/`json`
(already declared, currently unused), `openidconnect` 4 (already declared, currently unused),
`object_store` 0.12, `argon2`, `sha2`, `uuid`, `chrono`, `serde`. **No new external services**, and
no new crate unless `sqlx`'s embedded `migrate!` macro needs the `macros` feature enabled.

**Storage**: PostgreSQL is the system of record for all metadata (organisations, projects, users,
memberships, roles, sessions, recovery codes, invitations, ingest keys, API tokens, issues, events and
attachment metadata, log lines, suggestions, decisions, audit entries, jobs, OIDC connections). An
S3-compatible object store (Garage by default) keeps write-once raw payloads. The in-memory store
stays for tests and `SCOUT_STORE=memory`.

**Testing**: `cargo test` with in-crate `#[actix_web::test]` integration tests through the real Actix
app (constitution V). The default suite uses the in-memory store; PostgreSQL-backed tests are `#[ignore]`d
and run when `SCOUT_TEST_DATABASE_URL` points at a disposable database. Migration, restart-durability,
and concurrency evidence comes from `scripts/compose-smoke.sh` plus the committed results in
`specs/004-production-readiness/evidence/`.

**Target Platform**: Linux x86_64 container (single binary), WASM UI served from `/app/ui/dist`.

**Project Type**: Web service plus WASM frontend in one cargo workspace; the backend is still one
binary crate with no `lib.rs` and no `tests/` crate.

**Performance Goals**: sustain 500 accepted events/s on a 4 vCPU deployment at p95 acknowledgement
≤150 ms for 10 KB events; 95% of accepted items visible in issue/log lists within 10 seconds.

**Constraints**: one compose deployment unit; the application applies pending migrations at startup
before reporting ready, while planning, sequencing, and rolling back upgrades belong to the operator
and carry no concurrency or version-skew guarantee; the core loop must not require a message broker,
search cluster, or IdP; Sentry compatibility stays opt-in and adds no endpoints; metadata is read per
request with no application-level cache; a store failure returns a retryable response immediately with
no internal retry or buffering; invitations and initial-admin tokens expire seven days after issue;
operational counters are operator-only; the audit trail has no read surface; retention follows the
server receipt time; passwords require at least 12 characters with no composition rules; five
consecutive failed sign-ins lock an account until an operator unlocks it, and locked accounts always
return the generic invalid-credentials response; sessions expire after seven days or 12 idle hours,
where activity is any authenticated request carrying the session; unified search caps at 200 newest
matches; and only the deployment operator creates projects, naming an existing organisation member as
the initial owner.

**Scale/Scope**: a few dozen organisations, 1,000 issues per project, 1,000 tag values per key,
millions of events and log lines per project under retention, and three instances starting and serving
against one metadata store.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Requirement | Status | How this plan complies |
|---|---|---|
| I. Ingest never depends on classification (NON-NEGOTIABLE) | PASS | Classification stays a durable `classify_event` job claimed after ingest commits. Ingest still writes the raw object, commits the event row, enqueues, and acknowledges; the classifier call is bounded with one retry and no result on failure. No classifier work enters the request path. A metadata-store failure returns the retryable response immediately, with no in-process retry or buffer (FR-009). |
| II. Raw payloads immutable and authoritative | PASS | Object keys and byte layout do not change. PostgreSQL stores only the raw reference, size, attachment metadata, and expiry marker; retention clears the reference and deletes the object, never the event row or triage history. No endpoint writes payload bytes. |
| III. Project-scoped, server-side authorisation | PASS | Every project route resolves one project and one live role from the store per request (no cached snapshot). Scopes are checked server-side in `src/api.rs` and `src/compat/`. The deployment operator's authority is explicitly limited to deployment-level routes and is denied project data (FR-012, SC-015); project creation is such a deployment-level action and must name an existing organisation member as initial owner (FR-034, SC-026). Inaccessible resources return `404`/`403` without leaking existence. Because no instance caches metadata, a role change or key revocation is honoured by every instance on its next request (FR-031, SC-016). Operational counters and account unlocking are operator-only (FR-025, FR-032), and the audit trail has no read endpoint (FR-007). |
| IV. One deployment unit, minimal moving parts | PASS | No new service: PostgreSQL and Garage are already in `docker-compose.yml`. OIDC is optional and cannot block local sign-in or the core loop. The UI stays Sycamore/Trunk with no Node. |
| V. Evidence over assumption: test the real paths | PASS | Existing in-crate integration tests keep driving the real Actix app. New behaviour adds the same style of tests against the in-memory store, plus PostgreSQL-backed tests run through the same repository interface when a test database is available. Startup migrations are exercised by compose smoke (apply-before-ready and restart), while upgrade sequencing stays with the operator. |
| Security and Privacy constraints | PASS | Passwords, sessions, invitations, recovery codes, ingest keys, and API tokens remain SHA-256/Argon2id hashed and shown once. Every password-setting path enforces the 12-character minimum (FR-011, SC-022), five consecutive failed sign-ins lock the account with generic responses and operator-only unlocking (FR-032, SC-023), and sessions expire on a seven-day absolute limit or 12 hours of inactivity (FR-033, SC-024). OIDC client secrets must be replayed to the provider, so they are stored in a column excluded from every response and log, with the exception recorded in Complexity Tracking; verified-email matching never creates accounts. Classifier API keys are replayable in the same way and are stored in `projects.model_api_key_ref`, excluded from serialization and logged output, with the at-rest exposure recorded in Complexity Tracking. |
| Performance and Reliability constraints | PASS | Ingest keeps its 10 MB limits, `413`/`429`/`503` semantics, and acknowledgement-after-commit ordering. Operational counters are atomics on the hot path, not extra round trips. Health probes the database with a bounded timeout. |
| Compatibility and Scope constraints (v1.1.0) | PASS | The Sentry surface keeps its 11 documented paths and its opt-in gate. Migration `0007` keeps slug and per-project issue-number uniqueness; nothing under `/api/0/` gains a new endpoint. |
| Development Workflow & Quality Gates | PASS | Work is sliced so each slice passes `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, UI `cargo check --target wasm32-unknown-unknown`, and `trunk build --release`. |
| Migrations must be idempotent and concurrency-safe | PASS | The product ships its migrations and `src/schema.rs` runs the embedded set at startup before the service reports ready, so a deployment never serves on an unapplied schema. The migration engine records applied versions (re-running is a no-op) and takes PostgreSQL's advisory lock as an implementation detail; the specification deliberately claims no concurrency, ordering, or rollback guarantees, because that process belongs to the operator. `0008` is additive and `IF NOT EXISTS`-idempotent so it can also be applied by hand. |

**Gate result: PASS.** No unjustified violations. Three entries are recorded in Complexity Tracking:
the storage-seam refactor, the replayable OIDC client secret, and the replayable classifier key.

## Project Structure

### Documentation (this feature)

```text
specs/004-production-readiness/
├── plan.md              # This file
├── research.md          # Phase 0 decisions (R1–R13)
├── data-model.md        # Phase 1 durable mapping, invariants, migration 0008
├── quickstart.md        # Phase 1 runnable validation guide
├── contracts/
│   ├── native-api.md    # Changed and added /api routes, health fields, counters
│   ├── oidc.md          # Organisation OIDC configuration and sign-in flow
│   └── ui-surfaces.md   # Required dashboard views, states, and calls
├── evidence/            # Committed smoke and load-check results (Phase 3/4 output)
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 output (/speckit-tasks — not created here)
```

### Source Code (repository root)

```text
src/
├── store/                    # NEW: storage seam (was src/store.rs)
│   ├── mod.rs                # Store enum, async repository API, health, select(Config)
│   ├── memory.rs             # Current maps + seed(); tests and SCOUT_STORE=memory
│   └── postgres.rs           # PgPool adapter; every entity and query
├── schema.rs                 # NEW: embedded migrator, advisory lock, version-skew guard
├── metrics.rs                # NEW: atomic counters + snapshot for /api/metrics
├── oidc.rs                   # NEW: org OIDC config, discovery, PKCE flow, linking rules
├── models.rs (modify)        # + OidcConnection, is_demo, config fields
├── config.rs (modify)        # + SCOUT_STORE, SCOUT_RETENTION_INTERVAL_SECONDS, OIDC secrets
├── state.rs (modify)         # AppState holds Store; storage readiness from the store
├── main.rs (modify)          # connect → schema → bootstrap → object check → serve
├── auth.rs (modify)          # repository-based bootstrap, accounts, recovery codes
├── api.rs (modify)           # all handlers through the store API; + admin/OIDC/metrics routes,
│                             #   operator account list/unlock, lockout checks, project creation
├── ingest.rs (modify)        # transaction: raw object → event row → job → acknowledge
├── worker.rs (modify)        # durable job claim (SKIP LOCKED), SQL grouping + suggestion write
├── audit.rs (modify)         # async durable audit writes
├── compat/ (modify)          # auth/handlers/tokens read through the store API
├── error.rs (modify)         # actionable messages and retryable storage failures
├── ui.rs (unchanged)         # static bundle serving
└── tests.rs (modify)         # new coverage; PG tests gated on SCOUT_TEST_DATABASE_URL

migrations/
├── 0001…0007 (unchanged)     # exercised by the migrator for the first time
└── 0008_production_readiness.sql   # demo marker, OIDC tables, supporting indexes

ui/src/
├── api.rs (modify)           # org, invite, OIDC, demo and metrics calls
├── state.rs (modify)         # session/role-aware state for the new screens
└── components/
    ├── app.rs (modify)       # operator/admin navigation, project creation entry
    ├── overlays.rs (modify)  # real sign-in, recovery, invitation acceptance, setup token
    ├── other.rs (modify)     # members, keys, SSO, model/retention, operator organisation setup
    ├── issues.rs (modify)    # timeline, raw expired state, suggestion/decision card, filters
    └── common.rs (modify)    # shared loading/empty/error primitives used everywhere

scripts/
├── compose-smoke.sh          # NEW: clean-host smoke path (SC-009, SC-011)
└── load-ingest.sh            # NEW: sustained ingest + latency evidence (SC-010)

deploy/ (unchanged)           # garage.toml
docker-compose.yml (modify)   # SCOUT_STORE=postgres, retention interval, metrics-friendly logs
README.md, architecture.md (modify)  # match the shipped stack and routes
```

**Structure Decision**: The existing single-binary crate layout is preserved. The only structural
change is turning `src/store.rs` into a `src/store/` module so the two adapters live beside a single
repository surface; `src/schema.rs` is deliberately separate from `src/migrate.rs`, which is the
object-store migration subcommand from feature 003 and keeps its name and behaviour.

## Phase 0: Outline & Research

Decisions recorded in [research.md](./research.md):

| ID | Question | Decision |
|---|---|---|
| R1 | Persistence architecture | PostgreSQL authoritative behind a `Store` enum; no read cache; memory variant only for tests/explicit dev |
| R2 | Migration execution | The embedded migrator runs at startup before the service reports ready and aborts startup on failure; upgrade planning, sequencing, and rollback belong to the operator, with no concurrency or version-skew guarantees |
| R3 | Startup ordering and health | connect → migrate → bootstrap operator → verify objects → serve; `/api/health` probes both dependencies |
| R4 | Durable job queue | `jobs` row in the ingest transaction; claim with `FOR UPDATE SKIP LOCKED`; idempotent grouping |
| R5 | Ingest durability ordering | raw objects first, metadata transaction second, acknowledge last; orphans are retention's responsibility |
| R6 | Demo isolation | `organisations.is_demo`; demo sessions resolve only to the demo organisation; reset replaces demo rows only |
| R7 | Store selection and warnings | `SCOUT_STORE=postgres|memory`; postgres is the default when `DATABASE_URL` is set; loud warning otherwise |
| R8 | OIDC integration | `openidconnect` crate, per-organisation connection, authorization code + PKCE, verified-email match only |
| R9 | Operational counters | In-process atomics exposed as JSON at `/api/metrics` for the operator |
| R10 | Retention verification | Configurable sweep interval; smoke backdates rows through the shipped database |
| R11 | UI completion approach | Extend existing Sycamore components; no router or new crate; shared loading/empty/error primitives |
| R12 | Release evidence | Two committed scripts plus dated result files under `specs/004-production-readiness/evidence/` |
| R13 | Multi-instance reads, outage behaviour, token lifetime, counters, and audit surface | Per-request reads from the shared store (no application-level cache); store failures fail fast with no internal retry; invitations and setup tokens expire in seven days and report expiry distinctly; counters are operator-only; the audit trail has no read surface |
| R14 | Access and session hardening | 12-character minimum passwords with no composition rules; five consecutive failures lock an account until the operator unlocks it, with no time window; seven-day absolute and 12-hour idle session limits; 200-result search cap with `has_more`; operator-only project creation with a named initial owner and no credential in the response |

## Phase 1: Design & Contracts

- [data-model.md](./data-model.md) — table-to-model mapping, invariants, job state machine, and
  migration `0008`.
- [contracts/native-api.md](./contracts/native-api.md) — changed responses and the added routes
  (health fields, operator metrics, OIDC configuration, demo guards).
- [contracts/oidc.md](./contracts/oidc.md) — configuration shape, sign-in flow, matching rules,
  failure modes.
- [contracts/ui-surfaces.md](./contracts/ui-surfaces.md) — the views and states the dashboard must
  provide for US2, US3, and US4.
- [quickstart.md](./quickstart.md) — clean-host validation path for every success criterion.

## Post-Design Constitution Re-check

Re-evaluated after `research.md`, `data-model.md`, `contracts/`, and `quickstart.md` were written:

- **I**: The ingest transaction commits the event and its job before acknowledgement; classification
  is claimed separately and never blocks ingest. A store failure returns the retryable response
  immediately, releasing the ingest slot, with no retry or buffering (FR-009). **PASS**
- **II**: Retention clears `raw_object_key` and deletes objects; `events` rows, issue counters, and
  state history remain. No design element rewrites a stored payload. **PASS**
- **III**: Repository methods are per-request, project-scoped queries with no application-level cache,
  so three serving instances agree on every mutation (FR-031, SC-016); the operator guard is enforced
  server-side and covered by SC-015 tests; denied resources are indistinguishable from missing ones;
  counters are operator-only (FR-025) and the audit trail exposes no read route (FR-007). **PASS**
- **IV**: No new service or dependency on an IdP; OIDC is optional per organisation and local
  accounts remain always available. UI stays Sycamore/Trunk. **PASS**
- **V**: Each acceptance scenario maps to a memory-store integration test, and durability claims map
  to PostgreSQL-backed tests plus compose evidence; migrations run for the first time under the real
  advisory lock. **PASS**
- **Security**: Hashed credentials unchanged; the OIDC client secret and the classifier API key are the
  only replayable secrets, both excluded from serialization, responses, and logs, and both recorded in
  Complexity Tracking. **PASS**
- **Migrations**: `0008` is additive and `IF NOT EXISTS`-idempotent; startup applies pending migrations before serving and never reverts applied ones. **PASS**
- **Access hardening**: The lock state, failure counter, session limits, and initial-owner requirement are
  all store-level data with server-side checks; none depends on the UI. Generic responses keep the lock
  from becoming an account-enumeration oracle. Project creation returns project metadata only, so the
  deployment-level account never receives an ingest credential. **PASS**

**Post-design result: PASS.** No new violations.

**Environment gate**: this workspace has no Docker and no running PostgreSQL, so compose-based tasks
and the `SCOUT_TEST_DATABASE_URL` suite cannot be executed here — including T096 (three serving
instances) and T099 (token expiry against the store). They are scheduled and documented but must run on
a Docker-capable host before the feature is reported complete.

## Complexity Tracking

| Addition | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| `Store` enum with two adapters and async repository methods (replaces `Arc<RwLock<Store>>` in `AppState`) | The durable path and the test path must expose identical behaviour, and read-your-write staleness is unacceptable for role revocation and ingest visibility | A write-through cache over the existing maps would keep handlers unchanged, but it cannot give immediate revocation or cross-instance visibility within the 10 s target without a per-request refresh that erases its own benefit |
| Replayable OIDC client secret stored in `oidc_connections.client_secret` | The provider requires the secret on every token exchange, so it cannot be hashed | Hashing is impossible for a value that must be replayed; keeping it only in the environment would break per-organisation configuration without a redeploy (US3/AC4) |
| Replayable classifier key stored in `projects.model_api_key_ref` | The classifier call must present it to the project's decision endpoint, so it cannot be hashed | Hashing is impossible for a value that must be replayed, and keeping it in memory only is what this feature removes. It is excluded from every response and log; database access implies access to it, which the security review (T090) examines explicitly |
| `sqlx` `macros` feature (added) | `sqlx::migrate!` is not re-exported by the `migrate` feature alone, so embedding the migration set required `macros` | Loading migrations from disk works but makes the image's `migrations/` directory load-bearing at runtime; embedding keeps the single-binary promise. Verified: without `macros`, `cargo check` fails with "cannot find `migrate` in `sqlx`" |
