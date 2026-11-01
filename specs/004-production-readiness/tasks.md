---

description: "Task list for feature 004 — production readiness"
---

# Tasks: Production Readiness — Durable Metadata and Remaining Product Surfaces

**Input**: Design documents from `/specs/004-production-readiness/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Included because the project constitution (principle V) requires every functional
requirement to have an executable check. Existing tests in `src/tests.rs` use in-crate
`#[actix_web::test]` against the real Actix app; PostgreSQL-backed tests are `#[ignore]`d and run only
when `SCOUT_TEST_DATABASE_URL` is set. There is no `tests/` crate and no `lib.rs`.

**Organization**: Tasks are grouped by user story. US1 owns the storage seam and is the critical path;
US2 and US4 are mostly dashboard work that can proceed in parallel once the foundational phase lands;
US3 depends on US1's repository surface; US5 depends on US1–US4.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: `[US1]`…`[US5]`, mapping to the user stories in `spec.md`; omitted for Setup, Foundational, and Polish
- Every task names the exact file path it touches

## Path Conventions

- Backend: `src/` (single binary crate), migrations in `migrations/`, scripts in `scripts/`
- Frontend: `ui/src/` (Sycamore + Trunk, no Node.js)
- Feature artifacts: `specs/004-production-readiness/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Toolchain, configuration surface, and the directories the later phases write into.

- [x] T001 Confirm `sqlx::migrate!` compiles with the current dependency features and add `macros` to the `sqlx` feature list in `Cargo.toml` only if it does not (record the outcome in `specs/004-production-readiness/plan.md` Complexity Tracking)
- [x] T002 [P] Create `scripts/` and `specs/004-production-readiness/evidence/` with a `README.md` in `evidence/` that states the required header (date, host profile, commit, result) for committed evidence
- [x] T003 [P] Add `store_kind` (`SCOUT_STORE`, values `postgres|memory`, unset resolves from `DATABASE_URL`) and `retention_interval_secs` (`SCOUT_RETENTION_INTERVAL_SECONDS`, default `3600`, minimum `1`) to `src/config.rs`, with `#[cfg(test)]` coverage for every resolution case in research R7
- [x] T004 [P] Create `.env.example` documenting `SCOUT_OPERATOR_EMAIL`, `SCOUT_OPERATOR_PASSWORD`, `SCOUT_S3_ENDPOINT`, `SCOUT_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `SCOUT_STORE`, `SCOUT_DEMO_MODE`, and `SCOUT_RETENTION_INTERVAL_SECONDS`
- [x] T005 [P] Wire `SCOUT_STORE` and `SCOUT_RETENTION_INTERVAL_SECONDS` into the `scout` service in `docker-compose.yml`, keeping every existing variable and the `depends_on` health conditions unchanged

**Checkpoint**: `cargo test` still passes; no behaviour has changed.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Structure, schema, and model changes every story depends on. No user story work starts
until this phase compiles and the full existing suite passes.

**⚠️ CRITICAL**: T006 must land before any US1 task; T008–T010 unblock US2/US3 data work.

- [x] T006 Split `src/store.rs` into `src/store/mod.rs` (public surface, re-exports) and `src/store/memory.rs` (the existing maps, `Store::empty`, `seed`, `DEMO_PASSWORD`, and the current methods) with **no behaviour change**; update `src/main.rs` module wiring so the whole existing suite passes untouched
- [x] T007 [P] Add `migrations/0008_production_readiness.sql` exactly as specified in [data-model.md](./data-model.md) §2: `organisations.is_demo boolean NOT NULL DEFAULT false`, `audit_log.actor_label text NOT NULL DEFAULT ''`, `issues.decision_by_label text`, tables `oidc_connections` and `oidc_login_flows`, `CREATE EXTENSION IF NOT EXISTS pg_trgm`, and the two trigram indexes — every statement `IF NOT EXISTS`
- [x] T008 [P] Extend `src/models.rs`: add `is_demo: bool` (serde default) to `Organisation`, `actor_label: String` handling for `AuditEntry`, `decision_by_label` on `Issue`'s decision path, and new `OidcConnection` / `OidcLoginFlow` structs whose `client_secret`, `nonce`, and `pkce_verifier` are `#[serde(skip_serializing)]` and never `Debug`-printed
- [x] T009 [P] Update `src/audit.rs` so `record` / `record_with` also store the human-readable actor label required by `audit_log.actor_label`
- [x] T010 [P] Mark the seeded organisation as a demo organisation in `src/store/memory.rs` (`is_demo: true`) and add a `Store::demo_org_id()` helper used by the demo endpoints
- [x] T011 Update `src/tests.rs` harness constructors for the new fields and confirm `cargo test` is green before starting US1
- [x] T012 [P] Add `src/schema.rs` with the module skeleton and unit tests for version comparison (`stored > embedded`, `stored == embedded`, `stored < embedded`) using pure functions, without connecting to a database yet

**Checkpoint**: Foundation ready — US1 can start, and US2/US4 UI work can start in parallel.

---

## Phase 3: User Story 1 — Keep every record across restarts and upgrades (Priority: P1) 🎯 MVP

**Goal**: PostgreSQL is the system of record; schema changes apply automatically, idempotently, and
safely under concurrent starts; a failed or future-versioned schema aborts startup; health reflects
durability; the in-memory store remains only for tests and explicit local development.

**Independent Test**: Start the stack against PostgreSQL, create an organisation, project, member, key,
issue, log line, and audit entry, restart the application, and confirm every record, count, and history
entry is unchanged while health reports durable storage. Then start three instances at once and confirm
exactly one applies pending schema changes.

### Tests for User Story 1 ⚠️

> Write these first; they fail until the implementation lands. PostgreSQL-backed tests use
> `SCOUT_TEST_DATABASE_URL` and `#[ignore]`.

- [ ] T013 [P] [US1] Add migration tests in `src/tests.rs`: fresh database applies `0001`–`0008`; a second run applies nothing; `_sqlx_migrations` has one row per file (`#[ignore]`)
- [ ] T014 [P] [US1] Add a three-concurrent-start test in `src/tests.rs` that spawns three migrator runs against one database and asserts no error, one applied set, and no partially created table (`#[ignore]`)
- [ ] T015 [P] [US1] Add a version-skew test in `src/tests.rs` that inserts a migration version above the embedded maximum and asserts the startup error names both versions and leaves no listener (`#[ignore]`)
- [ ] T016 [P] [US1] Add repository parity tests in `src/tests.rs` that replay the identity, project, telemetry, and audit fixtures through both `Store` variants and assert identical results (`#[ignore]` for the PostgreSQL arm)
- [ ] T017 [P] [US1] Add a retryable-failure test in `src/tests.rs`: with the store marked unavailable, `POST /api/1/envelope/` returns `503` with `Retry-After` and no event row is created

### Implementation for User Story 1

- [ ] T018 [US1] Define `pub enum Store { Memory(Arc<RwLock<memory::MemoryStore>>), Postgres(postgres::PostgresStore) }` in `src/store/mod.rs` with the full async repository surface (lookups, mutations, list queries, job claim/complete, retention, health, `kind()`, `is_durable()`), delegating the `Memory` arm to the existing methods
- [ ] T019 [US1] Change `src/state.rs` so `AppState.store` is a cloned `Store` instead of `Arc<RwLock<Store>>`, and derive `ensure_storage()` from `Store::health()` with the existing `503` semantics
- [ ] T020 [US1] Implement adapter selection in `src/main.rs`/`src/config.rs` per research R7, including the loud warning `scout is using the in-memory metadata store: records will not survive a restart` and the `postgres` + missing `DATABASE_URL` startup error naming both variables
- [x] T021 [US1] Implement `src/schema.rs`: bounded pool connect retry (~30 s), `_sqlx_migrations` pre-check that aborts when the stored version exceeds the embedded maximum with `database schema version N is newer than this build (M); …`, embedded migrator run, and error mapping that always exits non-zero with an actionable message
- [ ] T022 [US1] Implement `src/store/postgres.rs` pool plumbing: `connect`, `health` (`SELECT 1`, 2 s timeout), session helpers, and `kind()`/`is_durable()`
- [ ] T023 [US1] Implement identity SQL in `src/store/postgres.rs`: users (password hash, `is_operator` unique), organisations (slug allocation, `is_demo`), `org_members`, sessions (hashed token, expiry sweep), recovery codes (revoke-then-issue in one transaction), invitations (hash lookup, pending/accept/revoke)
- [ ] T024 [US1] Implement project SQL in `src/store/postgres.rs`: projects (`retention_days` CHECK 1–3650, `confidence_threshold` CHECK 0–1, soft delete), `project_members` (roles `owner|admin|triager|viewer`), ingest keys (`revoked_at`, `last_used_at`), API tokens (hash lookup, revocation, `last_used_at`)
- [ ] T025 [US1] Implement telemetry SQL in `src/store/postgres.rs`: issues with `(project_id, fingerprint)` upsert and per-project `number` allocation, `issue_state_changes` append, events (`UNIQUE (project_id, sentry_event_id)`, raw key/size/expiry), attachments, log lines
- [ ] T026 [US1] Implement ops SQL in `src/store/postgres.rs`: audit inserts with `actor_label`, job insert/claim (`FOR UPDATE SKIP LOCKED` with backoff and attempt bound 5), job completion/failure, and retention statements returning the object keys to delete
- [ ] T027 [US1] Convert `src/auth.rs` to the repository API: `bootstrap_operator` (one operator only, never resets a password, actionable missing-variable error), `create_local_account`, `issue_recovery_codes`, `redeem_recovery_code`
- [ ] T028 [US1] Convert identity and administration handlers in `src/api.rs` (`login`, `logout`, `me`, `recover`, `me/recovery-codes`, orgs, invitations, acceptance) to repository calls with no lock held across an `await`
- [ ] T029 [US1] Convert project handlers in `src/api.rs` (`projects`, `project`, soft delete, keys, rotate, members, settings) to repository calls, preserving every existing status code and error message
- [ ] T030 [US1] Convert telemetry handlers in `src/api.rs` (`issues`, `issue`, `occurrences`, `raw`, `state`, `categorisation`, `logs`, `search`) to repository calls, keeping the documented filters, sorting, and result cap
- [ ] T031 [US1] Rewrite the ingest write path in `src/ingest.rs`: `put_create` the raw object, commit one transaction with the event row, attachment metadata, and `group_event` job, acknowledge only afterwards; on any failure return the existing retryable `503` and leave the orphan object to retention
- [ ] T032 [US1] Convert the Sentry compatibility layer (`src/compat/auth.rs`, `src/compat/handlers.rs`, `src/compat/tokens.rs`) to repository calls without changing any `/api/0/` response
- [ ] T033 [US1] Replace the in-process channel worker in `src/worker.rs` with a durable claim loop: load the raw payload from object storage by `raw_object_key`, group via SQL upsert, mark the job done/failed, and enqueue `classify_event` only after grouping commits
- [ ] T034 [US1] Apply the classification guard on the durable path in `src/worker.rs`: a late suggestion may not overwrite a decision or land after `first_presented_at` is set
- [ ] T035 [US1] Move the retention sweep to the durable store in `src/worker.rs` using `SCOUT_RETENTION_INTERVAL_SECONDS`: expire raw references and delete log lines past the project window, delete the referenced objects, keep issue rows, counters, and state history, and increment the counters added in US5
- [ ] T036 [US1] Rework the startup pipeline in `src/main.rs` to `config → validate → connect → schema → bootstrap operator → object-store verify → bind`, so no listener opens when any step fails
- [ ] T037 [US1] Extend `GET /api/health` in `src/api.rs` to the contract in [contracts/native-api.md](./contracts/native-api.md) (`storage`, `store`, `durability`, `object_store`), returning `503` and `degraded` when either dependency is unavailable
- [ ] T038 [US1] Map store failures to retryable `503` with actionable messages across `src/error.rs` and the converted handlers, without leaking driver internals or project existence
- [ ] T039 [US1] Keep `SCOUT_STORE=memory` fully functional for tests and local development and assert in `src/tests.rs` that `/api/health` then reports `durability: "ephemeral"`
- [ ] T040 [US1] Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` after the conversion in `src/` and fix every regression; the existing 001/002 suite must pass unchanged
- [ ] T041 [US1] On a Docker-capable host, run `cargo test -- --ignored` with `SCOUT_TEST_DATABASE_URL` set and record the result in `specs/004-production-readiness/evidence/`

**Checkpoint**: US1 is independently validatable with a restart against PostgreSQL. Stop here for the MVP.

---

## Phase 4: User Story 2 — Administer the deployment from the dashboard (Priority: P2)

**Goal**: The operator and administrators perform every production workflow from the dashboard: create
an organisation and initial admin, redeem the one-time token, invite members, change roles, manage keys,
and reissue recovery codes — with demo shortcuts isolated to the demo organisation.

**Independent Test**: With no prior data, sign in as the operator, create an organisation and initial
admin, accept the setup token as a new account, invite a second member, change their project role,
rotate an ingest key, and verify each change is visible immediately; then enable demo mode and confirm
a demo session cannot reach the real project while a demo reset preserves it.

### Tests for User Story 2 ⚠️

- [ ] T042 [P] [US2] Add SC-015 tests in `src/tests.rs`: the operator is denied every project-scoped route (issues, events, logs, raw, attachments, keys, settings) with `404`/`403` and no existence leak, while organisation creation and setup-token issuance still succeed
- [ ] T043 [P] [US2] Add SC-014 tests in `src/tests.rs`: with real and demo organisations present, a demo session lists only demo accounts, reads only demo projects, and receives `404` for the real project; `POST /api/demo/reset` leaves the real organisation, project, and operator intact
- [ ] T044 [P] [US2] Add initial-admin tests in `src/tests.rs`: a new-account token sets a password and returns recovery codes once; an existing account must sign in first, keeps its password, and gains the admin role; an expired/used/revoked token returns `410`
- [ ] T045 [P] [US2] Add recovery-code tests in `src/tests.rs`: reissuing invalidates prior unused codes, only the newest set works, and no plaintext is stored or returned twice

### Implementation for User Story 2

- [ ] T046 [US2] Enforce operator scope in `src/api.rs`: `auth_project`/`auth_org_admin` deny the operator on project routes while the operator keeps deployment-level routes; reuse the existing not-found/forbidden split
- [ ] T047 [US2] Scope the demo endpoints in `src/api.rs` and the store methods they use: accounts list only demo members; reset replaces demo-organisation rows only and never the operator or real organisations
- [ ] T048 [US2] Include `slug` and `is_demo` in `GET /api/orgs` in `src/api.rs` per the native API contract
- [ ] T049 [P] [US2] Complete real sign-in, recovery, invitation acceptance, and one-time recovery-code display in `ui/src/components/overlays.rs`, hiding demo shortcuts when `GET /api/demo/accounts` is unavailable
- [ ] T050 [P] [US2] Add the operator console to `ui/src/components/other.rs` (organisation list with slug and demo badge, creation form, one-time setup-token block with copy) and its navigation entry in `ui/src/components/app.rs`
- [ ] T051 [P] [US2] Add member and invitation screens to `ui/src/components/other.rs` (list, add, role change, removal with last-owner message, invite link shown once, revoke)
- [ ] T052 [P] [US2] Add ingest-key screens to `ui/src/components/other.rs` (list with prefixes, create, rotate, revoke, plaintext shown once)
- [ ] T053 [US2] Add the auth/org/invite/member/key calls and state to `ui/src/api.rs` and `ui/src/state.rs`
- [ ] T054 [US2] Make navigation role-aware in `ui/src/components/app.rs` (operator console only for the operator, organisation admin entry only for org admins, project settings only for owners/admins)
- [ ] T055 [US2] Apply the shared loading/empty/permission/error primitives from `ui/src/components/common.rs` to every admin screen added in this phase

**Checkpoint**: US1 and US2 both work independently; the dashboard has no demo-only production path.

---

## Phase 5: User Story 3 — Sign in with an organisation's identity provider (Priority: P3)

**Goal**: An organisation optionally connects an OIDC provider; a member signs in with a verified email
matching an existing membership and lands in the same roles, while local sign-in keeps working.

**Independent Test**: Configure a mock OIDC provider for an organisation, sign in with a verified-email
identity that matches a member, confirm the expected project access, confirm an uninvited and an
unverified identity are denied with no account created, then sign in locally with the same account.

### Tests for User Story 3 ⚠️

- [ ] T056 [P] [US3] Add OIDC flow tests with a local mock provider in `src/tests.rs`: verified-email match signs in, unverified email and unknown membership are denied with the fixed outcome codes, and no user row is created
- [ ] T057 [P] [US3] Add OIDC hardening tests in `src/tests.rs`: a replayed `state` fails, an expired flow fails, a removed connection fails the flow, and the client secret never appears in any response body or log line
- [ ] T058 [P] [US3] Add configuration tests in `src/tests.rs`: discovery failure returns `400` and changes nothing; `PUT`/`DELETE` require organisation admin; both actions are audited

### Implementation for User Story 3

- [ ] T059 [US3] Implement `src/oidc.rs`: discovery and metadata validation at save time, PKCE S256 start, state/nonce generation, token exchange, and ID-token validation (issuer, audience, expiry, nonce, signature) using the already-declared `openidconnect` crate
- [ ] T060 [US3] Add `oidc_connections` and `oidc_login_flows` repository methods to `src/store/mod.rs`, `src/store/memory.rs`, and `src/store/postgres.rs`, including the 10-minute flow TTL and sweep
- [ ] T061 [US3] Add the routes in `src/api.rs`/`src/main.rs`: `GET|PUT|DELETE /api/orgs/{org_id}/oidc` (organisation admin) and `GET /api/oidc/{org_slug}/start` + `GET /api/oidc/callback` (public), matching [contracts/oidc.md](./contracts/oidc.md) exactly
- [ ] T062 [US3] Implement the matching rule in `src/oidc.rs`: verified email only, existing membership or pending invitation in that organisation, no account creation, no role escalation, and identical denial for unverified and unknown emails
- [ ] T063 [US3] Audit `oidc.configured` and `oidc.removed` and redact the client secret, authorization code, PKCE verifier, nonce, and ID token from logs in `src/oidc.rs` and `src/audit.rs`
- [ ] T064 [P] [US3] Add the OIDC configuration section (issuer, client id, write-only secret, scopes, removal note) to `ui/src/components/other.rs`
- [ ] T065 [US3] Add the OIDC landing handler to `ui/src/components/app.rs`/`ui/src/state.rs` (read the `#oidc=<outcome>&token=` fragment, store the session, strip the fragment, show the failure code) and the calls in `ui/src/api.rs`

**Checkpoint**: Local sign-in is unaffected before, during, and after provider configuration.

---

## Phase 6: User Story 4 — Finish the triage, search, and settings experience (Priority: P4)

**Goal**: An issue detail shows the occurrence timeline, stack trace, breadcrumbs, release, raw payload
or an explicit expired state, and the suggestion/decision pair; issues and logs are searchable in one
result set; owners manage model and retention settings from the dashboard.

**Independent Test**: Seed a project with several occurrences, a suggestion, an expired raw payload, and
log lines; verify the detail view, expired state, filters, unified search, suggestion confirmation, and
the model/retention form all work in the UI with loading, empty, and error states.

### Tests for User Story 4 ⚠️

- [ ] T066 [P] [US4] Add backend tests in `src/tests.rs`: the occurrences endpoint returns newest-first event metadata; `GET /api/issues/{id}/raw` returns the stored bytes and, after retention, an explicit expired state with `410` and no body
- [ ] T067 [P] [US4] Add backend tests in `src/tests.rs`: unified search returns issues and logs in one ordered set, honours `state`/`level`/`category`/`q`/`sort`, applies the documented result cap, and never returns another project's rows
- [ ] T068 [P] [US4] Add backend tests in `src/tests.rs`: confirming or overriding a suggestion updates the authoritative decision, preserves the original suggestion, and does not move the issue or its event

### Implementation for User Story 4

- [ ] T069 [US4] Complete the issue list view in `ui/src/components/issues.rs`: state, level, category, text, and sort controls with an active-filter summary
- [ ] T070 [US4] Complete the issue detail header and timeline in `ui/src/components/issues.rs`: title, culprit, level, state, counts, release, environment, and the occurrence timeline
- [ ] T071 [US4] Add stack frames (in-app highlighted), breadcrumbs, and the read-only raw payload panel with the explicit expired state to `ui/src/components/issues.rs`
- [ ] T072 [US4] Add the suggestion-vs-decision card with confirm and override actions to `ui/src/components/issues.rs`, refreshing without moving the issue
- [ ] T073 [US4] Add resolve, reopen, and ignore controls to `ui/src/components/issues.rs`
- [ ] T074 [US4] Complete the logs view and the unified search view in `ui/src/components/other.rs`, distinguishing "no data yet" from "no matches"
- [ ] T075 [US4] Complete the model endpoint, confidence threshold, and retention form in `ui/src/components/other.rs` with inline validation errors
- [ ] T076 [US4] Extend `ui/src/api.rs` and `ui/src/state.rs` with the issue, occurrence, raw, search, log, and settings calls used by these views
- [ ] T077 [US4] Apply the shared loading, empty, and actionable error primitives from `ui/src/components/common.rs` to every view in this phase

**Checkpoint**: The triage loop is usable end to end without the API documentation open.

---

## Phase 7: User Story 5 — Prove the deployment is release-ready (Priority: P5)

**Goal**: One documented smoke path and one repeatable load check produce committed evidence that
durability, visibility, retention, and the compatibility surface behave as specified.

**Independent Test**: On a clean host, run the smoke path and load check; confirm the stack becomes
healthy, data survives a restart, retention expires raw data while triage history survives, the
compatibility paths answer, and the quality gates pass.

### Tests for User Story 5 ⚠️

- [ ] T078 [P] [US5] Add counter tests in `src/tests.rs`: accepted/rejected ingest, grouping outcomes, classifier outcomes, and retention deletions increment exactly once per event and surface through `GET /api/metrics`
- [ ] T079 [P] [US5] Add an operator-only access test for `GET /api/metrics` in `src/tests.rs`: `401` anonymous, `403` non-operator, `200` operator

### Implementation for User Story 5

- [ ] T080 [US5] Implement `src/metrics.rs` (atomics plus snapshot) and increment the counters in `src/ingest.rs`, `src/worker.rs`, and the retention sweep without adding round trips to the hot path
- [ ] T081 [US5] Expose `GET /api/metrics` in `src/api.rs` per [contracts/native-api.md](./contracts/native-api.md), operator-only, and emit the same numbers as periodic `INFO` lines
- [ ] T082 [US5] Write `scripts/compose-smoke.sh`: `docker compose up -d --build --force-recreate`, wait for health, operator sign-in, organisation + initial admin + token redemption, project creation, event and log ingest, raw retrieval, retention via the documented backdating step, restart, re-read, compatibility path check (`--compat-only`), and demo-confinement check; write a dated evidence file
- [ ] T083 [US5] Write `scripts/load-ingest.sh`: 10 KB envelopes at a target rate, accepted rate, p95 acknowledgement latency, and 95% time-to-visibility, compared against SC-010 and written to a dated evidence file
- [ ] T084 [US5] Verify feature 002's outstanding migration proof inside `scripts/compose-smoke.sh`: apply `migrations/0007_sentry_compat.sql` on the shipped database, re-apply it, assert organisation slug and per-project issue-number uniqueness with seeded duplicates, and assert it is a no-op the second time (closes 002 T047/T050)
- [ ] T085 [US5] Run `scripts/compose-smoke.sh` on the clean host and commit the result under `specs/004-production-readiness/evidence/`
- [ ] T086 [US5] Run `scripts/load-ingest.sh` on the documented 4 vCPU reference deployment and commit the result, recording any accepted shortfall with its justification
- [ ] T087 [US5] Update `README.md`, `architecture.md`, and `docker-compose.yml` to describe PostgreSQL as the metadata store, the in-memory mode, the bootstrap variables, the new routes, and every verification step in `quickstart.md`
- [ ] T088 [US5] Walk every section of `specs/004-production-readiness/quickstart.md` on a clean host and record pass/fail per scenario, including SC-001 through SC-015

**Checkpoint**: Evidence exists for every success criterion, including the previously unproven 002 migration.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Consistency, security review, and the final gate.

- [ ] T089 [P] Review `src/error.rs` and every handler for actionable `400/401/403/404/410/413/429/503` messages that never leak project existence, driver internals, or secrets (FR-026)
- [ ] T090 [P] Security review against the constitution: hashing of every credential, OIDC client-secret redaction, operator scope, demo confinement, no raw payload or bootstrap password in logs; record findings in `specs/004-production-readiness/evidence/security-review.md`
- [ ] T091 [P] Normalise terminology across `README.md`, `architecture.md`, `specs/004-production-readiness/*`, and the contracts: "raw payload", "durable metadata store", "in-memory store", "operator"
- [ ] T092 [P] Verify no response exposes a password hash, token hash, recovery-code hash, ingest-key hash, OIDC client secret, or model API key, by scanning router output fixtures in `src/tests.rs`
- [ ] T093 Run the full gate from the repository root (`Cargo.toml`, `src/`, `ui/`): `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `(cd ui && cargo check --target wasm32-unknown-unknown)`, and `(cd ui && trunk build --release)`, fixing every failure (SC-013)
- [ ] T094 Confirm every behaviour and automated check from features 001 and 002 still passes unchanged in `src/tests.rs` and that the ignored PostgreSQL suite runs green with `SCOUT_TEST_DATABASE_URL` (FR-029, SC-012)
- [ ] T095 Re-run `/speckit-converge` to verify the implementation against `spec.md` and append any remaining work to `specs/004-production-readiness/tasks.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately.
- **Foundational (Phase 2)**: Depends on Setup; BLOCKS every user story. T006 (store split) must land before any US1 task and before US2/US3 touch store methods.
- **US1 (Phase 3)**: Depends on Foundational. Critical path; US3 depends on its repository surface, and US5 depends on its durable store.
- **US2 (Phase 4)**: Depends on Foundational. Its UI tasks (T049–T055) are independent of US1; its backend tasks assume the store API from T018 being in place.
- **US3 (Phase 5)**: Depends on Foundational for the schema and on T018/T023–T026 for persistence.
- **US4 (Phase 6)**: Depends on Foundational. Backend tests use the memory store; UI work is independent of US1.
- **US5 (Phase 7)**: Depends on US1 (durable store, retention) and benefits from US2–US4 being complete for the smoke path.
- **Polish (Phase 8)**: Depends on all desired stories.

### Within Each Story

- Tests before implementation; the PostgreSQL tests are `#[ignore]`d and run with a live database.
- Repository methods before handlers; handlers before UI wiring.
- Each phase ends with the quality gate for the files it touched.

### Parallel Opportunities

- Setup: T002–T005 are independent files.
- Foundational: T007–T010 and T012 touch different files and can run together; T006 and T011 are sequential.
- US1: T013–T017 (tests) are independent of each other; T023–T026 are different SQL domains in one file and must be sequenced; T028–T030 are sequential edits to `src/api.rs`.
- US2: T049, T050, T051, T052 touch different files (T050–T052 share `other.rs`, so run them in order); T042–T045 are independent test files/sections.
- US3: T064 (`other.rs`) and T065 (`app.rs`, `state.rs`) can run together.
- US4: T069–T073 share `issues.rs` and must be ordered; T074–T075 share `other.rs` and must be ordered.
- US5: T078–T079 are independent; T082/T083 are independent scripts.

### Parallel Example: User Story 1

```text
# Tests (independent sections of src/tests.rs)
T013 migration tests · T014 concurrent starts · T015 version skew · T016 repository parity · T017 retryable 503

# Different SQL domains in src/store/postgres.rs (sequence them in one branch)
T023 identity · T024 projects · T025 telemetry · T026 ops/jobs/retention
```

### Parallel Example: User Story 2

```text
# UI files that do not overlap
T049 overlays.rs · T050–T052 other.rs (sequential) · T053 api.rs + state.rs · T054 app.rs
```

---

## Implementation Strategy

### MVP First (User Story 1 only)

1. Phase 1 Setup → Phase 2 Foundational (compiles, suite green, migrations written).
2. Phase 3 User Story 1 → restart-durability evidence against PostgreSQL.
3. **STOP and VALIDATE** with `quickstart.md` sections 1, 2, 7, 8, and 12.
4. This alone removes the product's largest credibility gap: metadata no longer disappears on restart.

### Incremental Delivery

1. Setup + Foundational → foundation ready.
2. US1 → durable core → validate → deploy (MVP).
3. US2 → dashboard administration and demo confinement → validate.
4. US3 → optional OIDC → validate.
5. US4 → triage/search/settings UI → validate.
6. US5 → smoke and load evidence → release.
7. Polish → quality gate and security review.

### Notes

- US2 and US4 are UI-heavy and can run alongside US1 as long as the store API in T018 lands first.
- PostgreSQL and compose tasks (T013–T017, T021, T031–T035, T041, T082–T088) cannot be verified in this
  workspace (no Docker, no PostgreSQL); they must run on a Docker-capable host before completion is
  claimed.
- Commit after each task or logical group, as in features 001–003.
- Do not modify custom reviewer checklists (`checklists/*`) on the reviewer's behalf.
