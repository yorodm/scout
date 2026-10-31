# Tasks: Scout — Self-Hosted Error Tracking & Log Ingestion

**Input**: Design documents from `/specs/001-error-tracking-log-ingestion/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Included. The plan requires in-crate integration coverage and the project explicitly asked for proper Rust integration tests. Per the project constraint there is no library crate and no `tests/` directory: cross-cutting tests live in `src/tests.rs` (`#[cfg(test)]`), unit tests live beside their modules.

**Organization**: Tasks are grouped by user story so each story is independently implementable and testable. The repository already contains a working prototype (Actix API, Sycamore UI, in-memory store); tasks describe the work to bring it to the plan, not a greenfield build.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: `[US1]`…`[US6]`, mapped to the spec's prioritized user stories
- Every task names an exact file path under the repository root

## Path Conventions

- Backend: `src/` (single binary crate; no `lib.rs`)
- Migrations: `migrations/`
- UI: `ui/src/` (workspace member `scout-ui`)
- Tests: `src/tests.rs` and `#[cfg(test)]` modules beside the code under test

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add the production dependencies and configuration surface the rest of the plan needs.

- [ ] T001 Add `sqlx`, `object_store`, `argon2`, `openidconnect`, `rand`, and `base64` dependencies to `Cargo.toml`; keep the existing Actix/Sycamore versions.
- [ ] T002 [P] Create `src/config.rs` with environment-backed settings and defaults: `DATABASE_URL`, `SCOUT_S3_*`, `SCOUT_RETENTION_DAYS` (30), `SCOUT_CONFIDENCE_THRESHOLD` (0.5), `SCOUT_INGEST_MAX_IN_FLIGHT`, `SCOUT_SESSION_TTL_DAYS` (7).
- [ ] T003 [P] Create the `migrations/` directory and register `sqlx::migrate!("./migrations")` in `src/main.rs`.
- [ ] T004 [P] Create `src/objects.rs` with an object-store abstraction (`put_create`, `get`, `delete`) over `object_store`.
- [ ] T005 [P] Create `src/auth.rs` with Argon2id hashing, opaque token generation, and SHA-256 token hashing helpers.
- [ ] T006 [P] Create `src/audit.rs` with an `audit(org_id, project_id, actor, action, target, metadata)` helper.
- [ ] T007 Update `docker-compose.yml` to add PostgreSQL and MinIO services with health checks and wire `DATABASE_URL`/`SCOUT_S3_*` into the app.
- [ ] T008 Update `Dockerfile` to build the Trunk UI and run the binary with migrations available in the image.
- [ ] T009 [P] Add or extend loading, empty, and error primitives in `ui/src/components/common.rs`.
- [ ] T010 Run the baseline checks for `Cargo.toml` and `ui/Trunk.toml`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`, and `cd ui && trunk build --release`; record the current pass state.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Persistence, object storage, authentication, job claiming, and shared error handling. **No user story starts until this phase is complete.**

### Migrations

- [ ] T011 Create `migrations/0001_orgs_users.sql`: `organisations`, `users`, `org_members` per data-model.md.
- [ ] T012 [P] Create `migrations/0002_projects_members.sql`: `projects` (`retention_days` check `1..=3650`, `confidence_threshold` check `0..=1` default `0.5`) and `project_members`.
- [ ] T013 [P] Create `migrations/0003_keys_sessions.sql`: `ingest_keys`, `sessions`, `recovery_codes` with hashed-token uniqueness.
- [ ] T014 [P] Create `migrations/0004_issues.sql`: `issues` (`state` `open|resolved|ignored`, unique `(project_id, fingerprint)`, `first_presented_at`) and `issue_state_changes`.
- [ ] T015 [P] Create `migrations/0005_events_logs.sql`: `events` (unique `(project_id, sentry_event_id)`, `raw_object_key` nullable, `raw_expired_at`), `attachments`, `log_lines`.
- [ ] T016 [P] Create `migrations/0006_jobs_audit.sql`: `jobs` and `audit_log`, with the `(state, available_at)` job index.

### Store and adapters

- [ ] T017 Define the `Store` trait in `src/store.rs` covering orgs, projects, users, memberships, keys, sessions, issues, events, logs, jobs, and retention.
- [ ] T018 Move the seeded in-memory implementation into `src/store_memory.rs`; keep `seed()` and add password hashes and recovery-code fixtures.
- [ ] T019 Implement the PostgreSQL adapter in `src/store_postgres.rs` using `sqlx::PgPool`, including all unique constraints and transactions from data-model.md.
- [ ] T020 Wire adapter selection (`SCOUT_STORE=memory|postgres`) in `src/state.rs` and `src/main.rs`; tests default to memory.
- [ ] T021 Implement ingest-key hashing, lookup, revocation, and `last_used_at` in `src/store_postgres.rs` and `src/ingest.rs`.
- [ ] T022 Implement Argon2id password verification, session issue/redeem/expiry, and `POST /api/login`, `POST /api/logout`, `GET /api/me` in `src/auth.rs` and `src/api.rs`.
- [ ] T023 Implement `src/objects.rs` write-once object semantics and wire raw object retrieval through it.
- [ ] T024 Implement job insertion and `SELECT … FOR UPDATE SKIP LOCKED` claiming in `src/store_postgres.rs` and `src/worker.rs`.
- [ ] T025 Add `PayloadTooLarge`, `TooManyRequests`, and `ServiceUnavailable` to `src/error.rs` with `413`, `429` + `Retry-After`, and `503` mappings.
- [ ] T026 Implement the configurable global in-flight ingest cap in `src/ingest.rs`, returning `429` with `Retry-After`.
- [ ] T027 Run migrations at startup under a PostgreSQL advisory lock in `src/main.rs`.
- [ ] T028 Extend `GET /api/health` in `src/api.rs` to report database and object-store reachability, returning `503` when either is down.
- [ ] T029 Add structured tracing fields (request id, project id, event id, job id) in `src/main.rs` and the ingest/worker paths.
- [ ] T030 Record admin actions through `src/audit.rs` for role, key, model, and retention changes.

### Shared models and test harness

- [ ] T031 Update `src/models.rs` enums: `IssueState` (`open`, `resolved`, `ignored`), `Role`, `Suggestion`, `CategorisationDecision`, and event raw/expiry fields.
- [ ] T032 Seed demo/triager/viewer accounts with known password hashes and project roles in `src/store_memory.rs`.
- [ ] T033 Update `src/api.rs` authorisation helpers to resolve the session user and project role from the `Store` trait without holding locks across awaits.
- [ ] T034 Update the harness in `src/tests.rs` to build the store via `src/store_memory.rs`, use a fake object store, and log in with email + password.
- [ ] T035 Add project/DSN resolution helpers (`dsn_id` and slug) to `src/store.rs` and use them in `src/ingest.rs`.
- [ ] T036 Verify every migration and store method against `data-model.md` validation rules and constraints; fix drift before user stories begin.

**Checkpoint**: Foundation ready — all six user stories can now be implemented in parallel.

---

## Phase 3: User Story 1 — Provision a project and ingest unmodified SDK errors into grouped issues (Priority: P1) 🎯 MVP

**Goal**: A project with an ingest key accepts Sentry traffic unmodified, stores the original payload, groups repeats into one issue with count/first/last-seen, and deduplicates retries.

**Independent Test**: On a fresh store, submit 1,000 identical events through the envelope endpoint; verify all are accepted, exactly one issue has count 1,000, first/last-seen are populated, a retried event id does not increment the count, and the raw payload is byte-identical.

### Implementation

- [ ] T037 [P] [US1] Extend `Event`, `Attachment`, `Issue`, and `EventMeta` in `src/models.rs` with `raw_object_key`, `raw_size`, `raw_expired_at`, `attachment_count`, and issue counters.
- [ ] T038 [P] [US1] Harden fingerprint derivation in `src/grouping.rs`: explicit fingerprint, exception-chain plus top five in-app frames, project salt, and normalised-message fallback.
- [ ] T039 [US1] Harden envelope parsing in `src/ingest.rs`: newline-delimited and length-prefixed items, `event`/`attachment`/`log`, unknown-item counting, truncated-length rejection, and gzip decoding.
- [ ] T040 [US1] Implement ingest authentication in `src/ingest.rs`: `X-Sentry-Auth`, `Authorization: Sentry …`, `?sentry_key=`, and `X-Scout-Key`; identical `401` for unknown, revoked, and cross-project keys.
- [ ] T041 [US1] Implement durability ordering in `src/ingest.rs`: write raw object(s), then insert event + grouping job in one transaction, then acknowledge; return `503` on failure.
- [ ] T042 [US1] Implement per-project deduplication by `sentry_event_id` in `src/store_postgres.rs` and `src/ingest.rs`; return the existing id without a new job.
- [ ] T043 [US1] Implement `POST /api/{project}/envelope/` (with and without trailing slash) in `src/ingest.rs` per `contracts/ingest.md`.
- [ ] T044 [US1] Implement `POST /api/{project}/store/` in `src/ingest.rs`, storing the request body as the raw payload.
- [ ] T045 [US1] Implement the grouping worker in `src/worker.rs`: claim job, derive fingerprint, upsert issue, increment count, update first/last-seen, link event, and never erase richer prior context.
- [ ] T046 [US1] Implement `GET /api/issues/{issue_id}/raw` in `src/api.rs` with `Content-Type: application/octet-stream` and `X-Scout-Event-Id`.
- [ ] T047 [US1] Enforce the 10 MB request and per-attachment limits in `src/ingest.rs`, returning `413` with nothing persisted.
- [ ] T048 [US1] Verify `429` + `Retry-After` in `src/ingest.rs` when the global in-flight cap is saturated, and that no accepted event is lost.
- [ ] T049 [US1] Keep route registration ordered in `src/main.rs` so `/api` and ingest routes are matched before the static-file catch-all.

### Tests for User Story 1

- [ ] T050 [P] [US1] Add grouping unit tests in `src/grouping.rs` for fingerprint stability, explicit fingerprints, frame ordering, and message normalisation.
- [ ] T051 [US1] Add the 3-event → one issue (count 3) integration test in `src/tests.rs`.
- [ ] T052 [US1] Add the 1,000-event → one issue (count 1,000) integration test in `src/tests.rs`.
- [ ] T053 [US1] Add the retried-event-id deduplication test in `src/tests.rs`.
- [ ] T054 [US1] Add unknown-key, revoked-key, and cross-project-key tests in `src/tests.rs`.
- [ ] T055 [US1] Add the byte-identical raw payload test (including the `X-Scout-Event-Id` header) in `src/tests.rs`.
- [ ] T056 [US1] Add the 10 MB rejection test in `src/tests.rs`.
- [ ] T057 [US1] Add the concurrency-cap `429` test in `src/tests.rs`.
- [ ] T058 [US1] Add attachment/breadcrumb/release preservation assertions to `src/tests.rs`.

### Story 1 completion

- [ ] T059 [US1] Update `README.md` ingest examples and DSN guidance for the production auth and size limits.
- [ ] T060 [US1] Run `quickstart.md` scenarios 1 and 2 and record the outcomes.

**Checkpoint**: User Story 1 is fully functional and independently testable — this is the MVP.

---

## Phase 4: User Story 2 — Triage grouped issues in the internal UI (Priority: P2)

**Goal**: Members list, filter, sort, search, open, and triage issues; detail shows stack trace, breadcrumbs, release, raw payload or an expired indication, and state history.

**Independent Test**: Seed a project with issues; as a member, filter/sort/search the list, open an issue, view the raw payload, and move it through open → resolved → ignored → open with recorded history.

### Implementation

- [ ] T061 [P] [US2] Implement issue list queries in `src/store_postgres.rs`: project scope, `state` (`open|resolved|ignored`), `level`, `q`, `category` (`issue|log|raw_event` from the human decision), and `sort` (`last_seen|count|first_seen`).
- [ ] T062 [US2] Implement `GET /api/projects/{project_id}/issues` in `src/api.rs` with the filters and sort from `contracts/api.md`.
- [ ] T063 [US2] Implement `GET /api/issues/{issue_id}` in `src/api.rs`, including event metadata, attachments, `raw_available`, and setting `first_presented_at` on the first read.
- [ ] T064 [US2] Implement `POST /api/issues/{issue_id}/state` in `src/api.rs` for `open`, `resolved`, and `ignored`, appending `issue_state_changes` with actor and time.
- [ ] T065 [US2] Return `404` from `src/api.rs` with an actionable message when the raw payload has expired, and keep issue metadata readable.
- [ ] T066 [P] [US2] Extend `IssuesView` in `ui/src/components/issues.rs` with state/level/category filters, sort controls, and search.
- [ ] T067 [P] [US2] Extend `IssueDetail` in `ui/src/components/issues.rs` with frames, breadcrumbs, release, raw payload view, and state-history timeline.
- [ ] T068 [US2] Add resolve/reopen/ignore controls with optimistic refresh in `ui/src/components/issues.rs`.
- [ ] T069 [US2] Extend the UI client with issue list/detail/state calls in `ui/src/api.rs`.
- [ ] T070 [US2] Use the loading/empty/error primitives from `ui/src/components/common.rs` in the issue views.
- [ ] T071 [US2] Wire issue navigation and the raw-expired state in `ui/src/components/app.rs`.

### Tests for User Story 2

- [ ] T072 [US2] Add issue list filter/sort/search integration tests in `src/tests.rs`.
- [ ] T073 [US2] Add open → resolved → ignored → open transition and history tests in `src/tests.rs`.
- [ ] T074 [US2] Add cross-project denial tests for list, detail, and raw in `src/tests.rs`.
- [ ] T075 [US2] Add the raw-expired (`raw_available: false`, `404` on raw) test in `src/tests.rs`.
- [ ] T076 [US2] Add the `first_presented_at` freeze test in `src/tests.rs`.
- [ ] T077 [US2] Verify `ui/Trunk.toml` builds: `(cd ui && cargo check --target wasm32-unknown-unknown)` and `trunk build --release`.

### Story 2 completion

- [ ] T078 [US2] Run `quickstart.md` scenarios 1 and 7 for the triage and access-control flows.
- [ ] T079 [US2] Document the issue filters and triage states in `README.md`.

**Checkpoint**: User Stories 1 and 2 work independently and together.

---

## Phase 5: User Story 3 — Ingest and search logs alongside issues (Priority: P3)

**Goal**: Structured logs are accepted under a project and searchable together with issues in one result set.

**Independent Test**: Submit JSON, NDJSON, and envelope log items; search for a term and verify matching logs and issues appear together, newest first, capped, and scoped to the project.

### Implementation

- [ ] T080 [P] [US3] Implement `LogLine` persistence and queries in `src/store.rs` and `src/store_postgres.rs`, including attributes JSONB and timestamp/level indexes.
- [ ] T081 [US3] Implement `POST /api/{project}/logs/` in `src/ingest.rs` for JSON objects, arrays, `logs`/`items` wrappers, and NDJSON.
- [ ] T082 [US3] Persist envelope `log` items in `src/ingest.rs` through the same log-line path.
- [ ] T083 [US3] Implement `GET /api/projects/{project_id}/logs` in `src/api.rs` with `q`, `level`, and `limit` (default 200, max 1000).
- [ ] T084 [US3] Implement `GET /api/projects/{project_id}/search` in `src/api.rs`: case-insensitive substring matching over the FR-012 fields, newest first, 50 per kind.
- [ ] T085 [P] [US3] Extend `LogsView` in `ui/src/components/other.rs` with level and text filters.
- [ ] T086 [P] [US3] Extend `SearchView` in `ui/src/components/other.rs` to render issues and logs in one result set.
- [ ] T087 [US3] Extend the UI client with log and search calls in `ui/src/api.rs`.

### Tests for User Story 3

- [ ] T088 [US3] Add log JSON/NDJSON/envelope ingest and retrieval tests in `src/tests.rs`.
- [ ] T089 [US3] Add the unified search test (issues + logs, newest first, capped) in `src/tests.rs`.
- [ ] T090 [US3] Add the cross-project log isolation test in `src/tests.rs`.
- [ ] T091 [US3] Add timestamp/level/attribute normalisation tests in `src/tests.rs`.
- [ ] T092 [US3] Run quickstart scenario 4 and verify the UI log/search views.

**Checkpoint**: User Stories 1–3 work independently; logs and issues are searchable together.

---

## Phase 6: User Story 4 — Get a categorisation suggestion and confirm or override it (Priority: P4)

**Goal**: A project owner connects a Jev-compatible endpoint; it suggests a category from a redacted, bounded summary; a triager confirms or overrides; failure never affects ingest.

**Independent Test**: Connect a controllable endpoint, submit an ambiguous item, verify a suggestion; make the endpoint unreachable, verify the item is stored with no suggestion; confirm and override, verify the decision and preserved suggestion.

### Implementation

- [ ] T093 [P] [US4] Implement project model settings (`endpoint`, `model`, `api_key_ref`, `confidence_threshold` default `0.5`) in `src/models.rs` and `src/store_postgres.rs`.
- [ ] T094 [US4] Implement `GET`/`PUT /api/projects/{project_id}/settings` in `src/api.rs` with endpoint, model-id, threshold, and retention validation; never return the API key.
- [ ] T095 [US4] Implement summary construction and redaction in `src/classifier.rs`: derived metadata plus a payload sample capped at 2,048 characters, with the sensitive-key list from `contracts/classifier.md` replaced by `[redacted]`.
- [ ] T096 [US4] Implement the Jev client in `src/classifier.rs`: `POST {endpoint}/v1/systemone`, optional bearer auth, 800 ms timeout, at most one retry, tolerant `choice`/`answer`/`category` parsing, and category normalisation.
- [ ] T097 [US4] Apply the per-project confidence threshold in `src/classifier.rs` and `src/worker.rs`; below it, record no suggestion.
- [ ] T098 [US4] Enforce the late-suggestion rule in `src/worker.rs`: apply only while `first_presented_at IS NULL` and `decision_at IS NULL`.
- [ ] T099 [US4] Implement `POST /api/issues/{issue_id}/categorisation` in `src/api.rs` for confirm/override with `issue|log|raw_event`, preserving the suggestion and never moving the stored item.
- [ ] T100 [US4] Update the built-in mock endpoint in `src/classifier.rs` to exercise redaction, thresholds, and outage paths.
- [ ] T101 [P] [US4] Add the suggestion/confidence card to `IssueDetail` in `ui/src/components/issues.rs`.
- [ ] T102 [P] [US4] Add the model connection, threshold, and retention form to `SettingsView` in `ui/src/components/other.rs`.
- [ ] T103 [US4] Extend the UI client with categorisation and settings calls in `ui/src/api.rs`.

### Tests for User Story 4

- [ ] T104 [US4] Add the suggestion-attached test in `src/tests.rs`.
- [ ] T105 [US4] Add the below-threshold discard test in `src/tests.rs`.
- [ ] T106 [US4] Add the unreachable/outage test asserting `200`, stored item, and `suggestion: null` in `src/tests.rs`.
- [ ] T107 [US4] Add malformed and unknown-category response tests in `src/tests.rs`.
- [ ] T108 [US4] Add the late-suggestion discard test using `first_presented_at` in `src/tests.rs`.
- [ ] T109 [US4] Add confirm/override tests asserting the decision is authoritative and the item/grouping/raw data are unchanged in `src/tests.rs`.
- [ ] T110 [US4] Run `quickstart.md` scenarios 5 and 6.

**Checkpoint**: User Stories 1–4 work independently; classifier failure is a normal state.

---

## Phase 7: User Story 5 — Administer organisations, members, roles, invites, and SSO (Priority: P5)

**Goal**: Organisation admins create projects and invitations; project owners manage keys and settings; roles are enforced server-side and take effect without a restart; recovery codes restore access; optional OIDC works.

**Independent Test**: Create an organisation and projects, invite two people with different roles, verify each sees only their scope, revoke one, and verify access disappears without redeploying; redeem a recovery code and sign in again.

### Implementation

- [ ] T111 [P] [US5] Implement invitation persistence, hashed one-time tokens, expiry, and preview/accept/revoke in `src/store_postgres.rs` and `src/auth.rs`.
- [ ] T112 [P] [US5] Implement single-use recovery codes at account creation and `POST /api/recover` in `src/auth.rs` and `src/api.rs`.
- [ ] T113 [US5] Implement `POST /api/orgs/{org_id}/invitations`, `GET /api/invitations/{token}`, `POST /api/invitations/{token}/accept`, and `DELETE /api/orgs/{org_id}/invitations/{id}` in `src/api.rs`.
- [ ] T114 [US5] Implement `POST /api/orgs/{org_id}/projects` in `src/api.rs`, creating the project and its first ingest key.
- [ ] T115 [US5] Implement member list/add/update/remove in `src/api.rs` with the owner/admin/triager/viewer matrix.
- [ ] T116 [US5] Enforce last-owner protection on demotion and removal in `src/api.rs`, returning `400`.
- [ ] T117 [US5] Implement ingest-key list/create/revoke in `src/api.rs`, returning plaintext once and never exposing hashes.
- [ ] T118 [US5] Implement optional OIDC configuration and the authorisation-code + PKCE flow in `src/auth.rs`, linking only on a verified matching email.
- [ ] T119 [US5] Record invitation, role, key, model, and retention changes through `src/audit.rs`.
- [ ] T120 [P] [US5] Add the password login, recovery, and invitation-accept forms to `ui/src/components/overlays.rs`.
- [ ] T121 [P] [US5] Extend `MembersView` in `ui/src/components/other.rs` with invites, role changes, removal, and last-owner messaging.
- [ ] T122 [P] [US5] Extend `SettingsView` in `ui/src/components/other.rs` with key create/revoke and SSO configuration.
- [ ] T123 [US5] Extend the UI client with auth, invite, member, key, and SSO calls in `ui/src/api.rs`.

### Tests for User Story 5

- [ ] T124 [US5] Add login success/failure, session expiry, and logout tests in `src/tests.rs`.
- [ ] T125 [US5] Add invitation create/preview/accept/revoke and single-use tests in `src/tests.rs`.
- [ ] T126 [US5] Add recovery-code success, reuse rejection, and unknown-code tests in `src/tests.rs`.
- [ ] T127 [US5] Add the role matrix and last-owner protection tests in `src/tests.rs`.
- [ ] T128 [US5] Add the live role-change-without-restart test in `src/tests.rs`.
- [ ] T129 [US5] Add the key rotation/revocation ingest test in `src/tests.rs`.
- [ ] T130 [US5] Add an OIDC mock-provider test covering verified-email linking and uninvited rejection in `src/tests.rs`.
- [ ] T131 [US5] Run `quickstart.md` scenarios 7 and 8.

**Checkpoint**: All administration and access-control flows are enforced server-side.

---

## Phase 8: User Story 6 — Deploy as one self-hosted unit with per-project retention (Priority: P6)

**Goal**: One compose file runs app + PostgreSQL + object storage; retention expires raw payloads and logs per project; stored payloads are never rewritten.

**Independent Test**: Start from empty services, reach a stored issue within 10 minutes, set a short retention, verify aged raw payloads/logs disappear while issue metadata remains, and verify an object cannot be overwritten.

### Implementation

- [ ] T132 [P] [US6] Finalize the `scout`/`postgres`/`minio` services, health checks, volumes, and environment in `docker-compose.yml`.
- [ ] T133 [P] [US6] Finalize the multi-stage `Dockerfile` (backend build, Trunk UI build, minimal runtime) and expose the UI bundle.
- [ ] T134 [US6] Implement the retention sweep job in `src/worker.rs` plus the per-request retention check in `src/ingest.rs`.
- [ ] T135 [US6] Delete expired attachments and raw objects, clear `events.raw_object_key`, set `raw_expired_at`, and delete expired `log_lines` in `src/store_postgres.rs` and `src/objects.rs`.
- [ ] T136 [US6] Enforce create-only object writes in `src/objects.rs` so no code path can overwrite an existing payload.
- [ ] T137 [US6] Ensure migrations in `migrations/` run idempotently on startup under the advisory lock in `src/main.rs` for multiple replicas.

### Tests for User Story 6

- [ ] T138 [US6] Add the retention test (expired raw/log removed, issue metadata kept, `raw_available: false`) in `src/tests.rs`.
- [ ] T139 [US6] Add the per-project retention isolation test in `src/tests.rs`.
- [ ] T140 [US6] Add the raw immutability test asserting no overwrite is possible in `src/tests.rs`.
- [ ] T141 [US6] Add a compose smoke script at `scripts/compose-smoke.sh` that starts the stack, waits for health, and reaches a first issue.
- [ ] T142 [US6] Run `quickstart.md` scenarios 1, 3, and 9 end to end.

**Checkpoint**: All six stories work in the single self-hosted deployment.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Observability, security, performance, and documentation across stories.

- [ ] T143 [P] Add ingest/worker counters and latency logs in `src/main.rs` and `src/worker.rs` (accepted/rejected, grouping latency, classifier outcomes, retention deletions).
- [ ] T144 [P] Review security: password/session/invite hashing, secret handling for model API keys, authorization on every route, and no raw payload in logs, in `src/auth.rs`, `src/api.rs`, and `src/classifier.rs`.
- [ ] T145 [P] Add an operator load script at `scripts/load-ingest.sh` for the 500 events/s target and document the measurement.
- [ ] T146 [P] Improve actionable error messages for `400`/`401`/`403`/`404`/`413`/`429`/`503` in `src/error.rs`.
- [ ] T147 [P] Update `README.md`, `architecture.md`, and `specs/001-error-tracking-log-ingestion/quickstart.md` to match the implemented routes and behavior.
- [ ] T148 Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` across `src/` and `ui/src/`; fix all failures.
- [ ] T149 Run the `ui/Trunk.toml` build: `(cd ui && cargo check --target wasm32-unknown-unknown)` and `(cd ui && trunk build --release)`.
- [ ] T150 Walk every scenario in `quickstart.md` and record pass/fail, including SC-001 through SC-010.

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (Phase 1)**: no dependencies.
- **Foundational (Phase 2)**: depends on Setup; blocks every user story.
- **User Stories (Phases 3–8)**: depend on Foundational. They may run in parallel by different developers, but the priority order is P1 → P2 → P3 → P4 → P5 → P6.
- **Polish (Phase 9)**: depends on the desired user stories being complete.

### User story dependencies

- **US1 (P1)**: independent after Foundational. MVP.
- **US2 (P2)**: independent after Foundational; consumes issues created by US1 but can be tested with seeded issues.
- **US3 (P3)**: independent after Foundational; shares project scoping and auth with US1/US2.
- **US4 (P4)**: independent after Foundational; consumes an event/issue but can be tested with seeded items.
- **US5 (P5)**: independent after Foundational; its role model is used by US2–US4 tests.
- **US6 (P6)**: depends on US1 (raw storage) and US3 (log retention); packaging can start earlier.

### Within each story

- Models → store queries → endpoints → UI → integration tests.
- Tests are written before the corresponding implementation where practical; all tests must pass before the story checkpoint.
- No story may depend on an unfinished task in another story.

### Parallel opportunities

- All Setup tasks marked `[P]` can run together.
- All migration tasks (T011–T016) are different files and can run in parallel after T001–T003.
- In US1, models, grouping, and unit tests (T037, T038, T050) can run in parallel; the endpoints and worker are sequential.
- In US2–US5, backend and UI tasks marked `[P]` can be split across developers once the store layer exists.
- US1–US5 can be staffed in parallel after Foundational, provided each developer works on disjoint files.

---

## Parallel Example: User Story 1

```text
Task T037: extend Event/Attachment/Issue models in src/models.rs
Task T038: harden fingerprint derivation in src/grouping.rs
Task T050: add grouping unit tests in src/grouping.rs
```

Then:

```text
Task T039: harden envelope parsing in src/ingest.rs
Task T041: implement durability ordering in src/ingest.rs
Task T043: implement the envelope endpoint in src/ingest.rs
Task T045: implement the grouping worker in src/worker.rs
```

---

## Implementation Strategy

### MVP first (User Story 1 only)

1. Complete Phase 1 (Setup).
2. Complete Phase 2 (Foundational) — this is the critical path.
3. Complete Phase 3 (US1).
4. **Stop and validate**: 1,000 events → one issue, raw byte identity, key scoping, dedupe.
5. Deploy/demo the ingest + grouping slice.

### Incremental delivery

1. Foundational → US1 → validate (MVP).
2. US2 → validate triage UI and state history.
3. US3 → validate logs and unified search.
4. US4 → validate classifier suggestion/confirmation and outage behavior.
5. US5 → validate invitations, roles, recovery, and SSO.
6. US6 → validate compose deployment and retention.
7. Polish → observability, security, performance, documentation.

### Parallel team strategy

- Team completes Setup + Foundational together.
- After Foundational:
  - Developer A: US1 (ingest/grouping).
  - Developer B: US2 (triage API/UI).
  - Developer C: US3 (logs/search).
  - Developer D: US4 (classifier).
  - Developer E: US5 (administration/auth).
  - Developer F: US6 (deployment/retention).
- Keep file ownership disjoint: `src/ingest.rs` (US1/US3), `src/api.rs` (US2–US5, coordinate), `src/worker.rs` (US1/US4/US6, coordinate), `ui/src/components/*` (split by component).

---

## Notes

- Every task includes an exact path; no task relies on a `tests/` directory or a `lib.rs`.
- `[P]` tasks touch different files and have no dependency on an incomplete task.
- Commit after each task or logical group; do not commit generated `ui/dist` artifacts unless the release process requires them.
- Stop at each checkpoint and validate the story independently before moving on.
- The prototype's existing in-crate tests in `src/tests.rs` should be extended, not replaced.
