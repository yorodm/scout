# Tasks: Sentry Read-API Compatibility

**Input**: Design documents from `/specs/002-sentry-api-compatibility/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Included. The feature specification does not ask for TDD, but constitution principle V
requires an executable check for every functional requirement, so each story has test tasks before its
implementation tasks.

**Organization**: Tasks are grouped by user story so each story is independently implementable and
testable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US4)
- Every task names the exact file path to change

## Path Conventions

Single cargo binary crate at the repository root (`src/`), WASM UI workspace member at `ui/`, SQL
under `migrations/`. Tests live in `src/tests.rs` and `#[cfg(test)]` modules; there is no `tests/`
crate (constitution V).

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the compatibility module tree, configuration switch, schema parity, and the two
primitives every story needs (error envelope, cursor codec).

- [ ] T001 Create `src/compat/{mod,auth,error,cursor,projection,query,handlers,tokens}.rs` with module declarations and declare `mod compat;` in `src/main.rs`
- [ ] T002 [P] Add `sentry_compat_enabled` read from `SCOUT_SENTRY_API_COMPAT_ENABLED` (default `false`) via the existing `env_flag` helper in `src/config.rs`, and wire `AppState.compat_enabled` in `src/state.rs` and `src/main.rs`
- [ ] T003 [P] Add the idempotent migration `migrations/0007_sentry_compat.sql` (organisations.slug, issues.number, api_tokens, unique indexes, backfills) exactly as specified in `data-model.md`
- [ ] T004 [P] Implement `CompatError` with the `{"detail": "<message>"}` envelope and statuses `400|401|403|404|409|501` in `src/compat/error.rs`, with unit tests for every variant
- [ ] T005 [P] Implement the opaque cursor codec in `src/compat/cursor.rs`: base64url-unpadded `{v,q,s,k,i}`, query fingerprint over normalised `(query, sort, ids, hashes, org, project)`, decode errors → `CompatError::BadRequest`, with unit tests for round-trip and mismatch

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Data model changes, store support, token authentication, scope derivation, and the
`/api/0` gate. No user story can be implemented before this phase completes.

**⚠️ CRITICAL**: Complete this phase before any user story work.

- [ ] T006 Add `ApiToken { id, user_id, label, token_hash, token_prefix, created_at, last_used_at, revoked_at }` to `src/models.rs`; `token_hash` is skipped from serialisation (invariants in `data-model.md`)
- [ ] T007 [P] Add `Organisation.slug` with `#[serde(default)]` and the slug derivation helper (lowercase, `[a-z0-9-]`, collapse runs, trim, ≤48 chars, `-2`/`-3` suffix on case-insensitive collision, empty name → `org`) in `src/models.rs`
- [ ] T008 [P] Add `Issue.number: u64` to `src/models.rs` and assign `max(number) + 1` per project at both issue-creation sites (`src/store.rs` grouping path and `src/worker.rs` classifier path)
- [ ] T009 Add token storage to `src/store.rs`: `api_tokens` collection with a `token_hash` index, `create_token`, `token_by_hash` (rejects revoked), `tokens_for_user`, `tokens_for_org`, `revoke_token`, and the ≤1-hour `last_used_at` write
- [ ] T010 Backfill slugs and issue numbers in `src/store.rs` (`Store::empty` and `store::seed`) and derive the slug when an organisation is created in `src/api.rs` (`create_org`)
- [ ] T011 Implement bearer authentication in `src/compat/auth.rs`: parse `Authorization: Bearer scout_pat_…`, SHA-256 via `auth::hash_token`, identical `401 {"detail":"Invalid token"}` for missing/malformed/unknown/revoked, disabled users rejected
- [ ] T012 Add project/org resolution in `src/compat/auth.rs`: org by slug or UUID, project by slug, UUID, or `dsn_id`, soft-deleted or unknown → `404`, no role or cross-project → `404`
- [ ] T013 Add `Scope` (event:read|write|admin, project:read|write|admin|releases, org:read|admin|integrations) and the live `Role`/`OrgRole` mapping table from `data-model.md` in `src/compat/auth.rs`, plus a `require(scope)` helper returning `403`, with unit tests for each role
- [ ] T014 Register the `web::scope("/api/0")` and the disabled-gate middleware (disabled → `404 {"detail":"Not Found"}` before auth) in `src/compat/mod.rs` and `.configure(crate::compat::config)` in `src/main.rs`
- [ ] T015 [P] Add an in-crate helper that issues a token and returns `(plaintext, ApiToken)` for tests in `src/tests.rs`, reusing `auth::new_token`/`auth::hash_token`

**Checkpoint**: Foundation ready — the four story phases can proceed.

---

## Phase 3: User Story 1 — Read and triage a project's issues (Priority: P1) 🎯 MVP

**Goal**: A Sentry client lists, filters, sorts, pages, and status-mutates a project's issues through
`/api/0/projects/{org}/{project}/issues/` with Sentry-shaped objects and headers.

**Independent Test**: Point a Sentry-shaped client at the app with a Scout token, list issues, filter
`is:resolved` with `sort=freq&limit=25`, page through 1,000 issues, resolve two by id, and confirm the
state and history appear in the native API/UI.

### Tests for User Story 1 ⚠️ (write first, expect failure)

- [ ] T016 [P] [US1] Contract test for `GET /api/0/projects/{org}/{project}/issues/`: field set, `statusDetails`, `userCount`, `annotations`, `permalink`, `Link` and `X-Hits` headers in `src/tests.rs`
- [ ] T017 [P] [US1] Contract test for `PUT .../issues/`: multi-id update, cross-project ids skipped, state history names the token owner, missing `status` → `400`, `assignedTo` → `400`, unknown status → `400`, `DELETE` → `501` in `src/tests.rs`
- [ ] T018 [P] [US1] Pagination test: 1,000 issues traversed exactly once, `limit` clamps (0, negative, 101, non-numeric → 100/≤100), cursor replay against a different query → `400`, exhausted cursor → `[]` with `results="false"` in `src/tests.rs`
- [ ] T019 [P] [US1] Projection unit tests for status mapping in both directions, `shortId`, RFC 3339 `Z` timestamps, `metadata`/`tags`, and ignored `ignoreCount` in `src/compat/projection.rs`

### Implementation for User Story 1

- [ ] T020 [US1] Implement the `SentryIssue` projection (all fields in `data-model.md`, `shortId = {PROJECT}-{number}`, `permalink = {self_url}/#/issues/{id}`, `count` numeric) in `src/compat/projection.rs`
- [ ] T021 [US1] Implement the search tokenizer and filters (`is:`, `level:`, `release:`, `environment:`, tag keys, free text over title/culprit/fingerprint, unsupported operators ignored) and the sort map in `src/compat/query.rs`, with unit tests
- [ ] T022 [US1] Implement the issue list handler (query, sort, `limit`, `cursor`, repeated `id`, `hashes` on `fingerprint`, ignore `statsPeriod`/`shortIdLookup`) with `Link`/`X-Hits` emission in `src/compat/handlers.rs`
- [ ] T023 [US1] Implement the status mutation handler (per-issue state change, one history entry each with the token owner's email, in-project ids only, unsupported attributes rejected) in `src/compat/handlers.rs`
- [ ] T024 [US1] Implement `DELETE .../issues/` → `501` citing preserved history and raw payloads in `src/compat/handlers.rs`
- [ ] T025 [US1] Register the three issues routes (with and without trailing slash) in `src/compat/mod.rs`

**Checkpoint**: US1 works standalone; this is the MVP.

---

## Phase 4: User Story 2 — Browse project and issue tag values (Priority: P2)

**Goal**: List distinct values for a tag key for a project or for one issue, with contains filtering
and pagination.

**Independent Test**: Seed issues with `release` values, request the project and issue tag-value
paths, verify distinctness, ordering, `query` filtering, pagination cap, and `[]` for a missing key.

### Tests for User Story 2 ⚠️

- [ ] T026 [P] [US2] Tag-value contract tests: distinct ascending values, contains filter, empty/object values omitted, non-string scalars stringified, 1,000 cap, missing key → `[]`, issue-scoped path, inaccessible issue → `404` in `src/tests.rs`

### Implementation for User Story 2

- [ ] T027 [US2] Implement tag extraction from `Issue.tags` (stringify scalars, skip null/empty/objects, de-duplicate, sort, cap 1,000) in `src/compat/projection.rs`
- [ ] T028 [US2] Implement the project tag-values and issue tag-values handlers, including `query` filtering and cursor/limit handling, in `src/compat/handlers.rs`
- [ ] T029 [US2] Register the tag-value routes in `src/compat/mod.rs`

**Checkpoint**: US1 and US2 both work independently.

---

## Phase 5: User Story 3 — Authenticate with tokens and scopes (Priority: P3)

**Goal**: Users can create, list, and revoke long-lived tokens through the native API and a dashboard
screen; scopes follow live roles and denial is uniform.

**Independent Test**: Create a token, call a read endpoint, mutate with a viewer token and get `403`,
revoke and get `401`; remove the owner from the project and get `404`; manage the whole lifecycle from
`#/tokens` in under a minute.

### Tests for User Story 3 ⚠️

- [ ] T030 [P] [US3] Native token tests: create returns plaintext once (prefix `scout_pat_`), list omits hash/plaintext, revoke is immediate and idempotent, another user's token → `404`, audit rows written in `src/tests.rs`
- [ ] T031 [P] [US3] Scope matrix tests: viewer read success + mutation `403`, triager mutation success, admin/owner full set, org member `org:read`, non-member `404`, live role removal between requests, org admin can list/revoke member tokens, plain member `403` in `src/tests.rs`

### Implementation for User Story 3

- [ ] T032 [US3] Implement native token endpoints `GET/POST /api/me/tokens` and `DELETE /api/me/tokens/{id}` with label validation (1–100 chars after trim), one-time plaintext, and `token.create`/`token.revoke` audit rows in `src/compat/tokens.rs`
- [ ] T033 [US3] Implement org-admin endpoints `GET /api/orgs/{org_id}/tokens` and `DELETE /api/orgs/{org_id}/tokens/{token_id}` (metadata only, member scope, `403` for non-admins) in `src/compat/tokens.rs` and register all token routes in `src/api.rs`
- [ ] T034 [US3] Build the API-token screen component (list, create form, one-time copy panel, revoke confirmation, empty/error states, keyboard accessible) in `ui/src/components/tokens.rs`
- [ ] T035 [US3] Add token API client calls and screen state/actions in `ui/src/api.rs` and `ui/src/state.rs`, export the module in `ui/src/components/mod.rs`, and add the `#/tokens` navigation entry in `ui/src/components/app.rs`

**Checkpoint**: US1–US3 work; token lifecycle is fully usable from the UI.

---

## Phase 6: User Story 4 — Predictable behaviour for unsupported features (Priority: P4)

**Goal**: Every remaining documented path answers honestly — empty collection, `404` detail, or `501`
naming the feature — with no fabricated data and no state change.

**Independent Test**: Call each unsupported path and confirm the status/body from the compatibility
matrix in `contracts/sentry-compat-api.md`, and that Scout data is unchanged.

### Tests for User Story 4 ⚠️

- [ ] T036 [P] [US4] Unsupported-surface tests: release commit files `[]`/`404`, service hooks `[]`/`404`, feedback `[]`, app installations `[]`, spike protection `404`, every mutation `501` with a naming `detail`, no data mutation, all errors use the `detail` envelope in `src/tests.rs`

### Implementation for User Story 4

- [ ] T037 [US4] Implement the release commit-files handler (known release → `[]`, unknown → `404`) in `src/compat/handlers.rs`
- [ ] T038 [US4] Implement the service-hook, feedback, spike-protection, app-installation, and external-issue handlers using a single documented exception table in `src/compat/handlers.rs`
- [ ] T039 [US4] Register the US4 routes (with trailing-slash variants) in `src/compat/mod.rs`

**Checkpoint**: All 11 documented paths now answer per contract.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, regression proof, performance evidence, and quality gates.

- [ ] T040 [P] Document `SCOUT_SENTRY_API_COMPAT_ENABLED`, the `/api/0` support matrix, and the token screen in `README.md` and `architecture.md`
- [ ] T041 [P] Performance test SC-007: 1,000-issue issue list and tag-value requests at p95 ≤500 ms, with `X-Hits` included in the measurement, in `src/tests.rs`
- [ ] T042 [P] Regression test SC-008: existing native endpoints and payloads unchanged, the token screen is the only new UI route, in `src/tests.rs`
- [ ] T043 Verify the disabled toggle across every path (`404 {"detail":"Not Found"}` before auth) and confirm no `/api/0/` response is HTML in `src/tests.rs`
- [ ] T044 Run `quickstart.md` end-to-end, including SC-006 (change visible within 1 s) and SC-009 (create/copy/revoke under a minute)
- [ ] T045 Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, UI `cargo check --target wasm32-unknown-unknown`, and `trunk build --release` from the repository root
- [ ] T046 Remove scaffolding `#[allow(dead_code)]`, stray TODOs, and unused helpers introduced during setup in `src/compat/`, `src/models.rs`, and `src/store.rs`, then re-run clippy

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately.
- **Foundational (Phase 2)**: Depends on Setup — **blocks all stories**.
- **User Stories (Phases 3–6)**: All depend on Foundational.
  - US1 has no story dependency.
  - US2 depends on T020 (projection helpers) and T025 (route registration pattern) but can be built
    in parallel with US1's T022–T024 by another developer, since tag values touch different handlers.
  - US3 depends on T009–T013 (token store and scopes); its UI tasks depend on T032/T033.
  - US4 depends only on T014 (scope registration); independent of US1–US3.
- **Polish (Phase 7)**: Depends on all shipped stories.

### Story Dependencies

- **US1 (P1)**: after Foundational → MVP.
- **US2 (P2)**: after Foundational; reuses projection and cursor helpers.
- **US3 (P3)**: after Foundational; the token model and scope map already exist, so this phase is the
  management surface and UI.
- **US4 (P4)**: after Foundational; no dependency on data from other stories.

### Within Each Story

Tests first (they must fail), then helpers, then handlers, then route registration. A story is done
only when its phase tests pass through the real Actix app.

### Parallel Opportunities

- Setup: T002, T003, T004, T005 are independent files.
- Foundational: T007/T008 (models) and T011–T013 (auth) touch different files and can be split; T009
  and T010 are sequential within `src/store.rs`.
- US1: T016–T019 are independent test files/modules; T020/T021 are independent helper modules.
- US2: T026 can be written while US1 handlers are in progress; T027–T029 must be sequential.
- US3: T030/T031 in parallel; T032 → T033; T034/T035 in parallel with T032 once the contract is fixed.
- US4: T036 in parallel with T037/T038.
- Polish: T040, T041, T042, T043 are independent.

---

## Parallel Example: User Story 1

```bash
# Tests first (all independent):
Task: "Contract test for GET issues in src/tests.rs"
Task: "Contract test for PUT issues in src/tests.rs"
Task: "Pagination test over 1,000 issues in src/tests.rs"
Task: "Projection unit tests in src/compat/projection.rs"

# Then the independent helpers:
Task: "Implement SentryIssue projection in src/compat/projection.rs"
Task: "Implement query tokenizer and sort map in src/compat/query.rs"
```

## Parallel Example: User Story 3

```bash
Task: "Native token tests in src/tests.rs"
Task: "Scope matrix tests in src/tests.rs"

# Once the token endpoints are fixed by contract:
Task: "Build the API-token screen in ui/src/components/tokens.rs"
Task: "Wire token client and nav in ui/src/api.rs and ui/src/components/app.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 only)

1. Phase 1 Setup → Phase 2 Foundational (both block everything).
2. Phase 3 US1: issues read + status triage with pagination.
3. **STOP and VALIDATE** using the US1 independent test. This is a useful Sentry-compatible MVP.

### Incremental Delivery

1. Foundation ready → US1 (MVP) → validate.
2. Add US2 (tag values) → validate → demo.
3. Add US3 (token management + dashboard screen) → validate → demo.
4. Add US4 (honest non-support) → validate → the 11-path contract is complete.
5. Polish: docs, performance, regression, quality gates → commit.

### Parallel Team Strategy

- Team completes Setup + Foundational together.
- Then: Developer A → US1, Developer B → US2 (after T020), Developer C → US3 (after T009–T013),
  Developer D → US4.
- US1 is the critical path for a demonstrable feature; US3's UI can start as soon as the native token
  contract is implemented.

---

## Notes

- `[P]` tasks touch different files and have no incomplete dependency.
- Every story remains independently testable; no story requires another story's UI.
- The compatibility surface must stay opt-in; tests assert both enabled and disabled behaviour.
- Never widen the surface beyond the 11 documented paths (constitution v1.1.0).
- Commit after a logical group and mark tasks `[X]` as they complete.
- Avoid: fabricating data for unsupported features, changing native endpoints, or adding a `tests/`
  crate.
