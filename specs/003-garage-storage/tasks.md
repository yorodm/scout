# Tasks: Garage Object Storage

**Input**: Design documents from `/specs/003-garage-storage/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Included. The specification does not request TDD, but constitution principle V requires an
executable check for every functional requirement, so each story has test tasks before its
implementation tasks where a test can run without Docker.

**Organization**: Tasks are grouped by user story so each story is independently implementable and
testable. The Garage compose scenarios need Docker; they are documented as a completion gate, not an
automated test in this workspace.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US4)
- Every task names the exact file path to change

## Path Conventions

Single cargo binary crate at the repository root (`src/`), deployment assets in `deploy/` and
`docker-compose.yml`, docs at the root. Tests live in `src/tests.rs` and `#[cfg(test)]` modules; there
is no `tests/` crate (constitution V).

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Dependency, deployment config, container entrypoint, and the migration module skeleton.

- [ ] T001 [P] Add `futures-util = { version = "0.3", default-features = false, features = ["std"] }` to `[dependencies]` in `Cargo.toml` (listing stream adapter per research R4)
- [ ] T002 [P] Create `deploy/garage.toml` with the contract's keys: `metadata_dir`, `data_dir`, `db_engine = "sqlite"`, `replication_mode = "1"`, `rpc_bind_addr`, a development `rpc_secret`, `s3_api.api_bind_addr = "[::]:3900"`, `s3_api.s3_region = "garage"`, `s3_api.root_domain`, `admin.api_bind_addr = "[::]:3903"`, and a development admin token
- [ ] T003 [P] Change the runtime stage in `Dockerfile` from `CMD ["scout"]` to `ENTRYPOINT ["scout"]` so `docker compose run --rm scout migrate-objects` works (research R13)
- [ ] T004 Create `src/migrate.rs` with module declarations and a `run_from_env` stub, declare `mod migrate;` in `src/main.rs`, and dispatch `migrate-objects` from `std::env::args()` before any Actix/worker wiring

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The storage abstraction both the running service and the migration mode depend on.

**⚠️ CRITICAL**: No user story can be completed before this phase.

- [ ] T005 Add `S3Settings { endpoint, bucket, region, access_key, secret_key }` to `src/objects.rs` with `S3Settings::from_env()` reading endpoint/bucket from `SCOUT_S3_*`, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, and `AWS_REGION` (no new `Config` fields); its builder constructs `AmazonS3Builder` from those explicit values, sets `with_allow_http` when the endpoint starts with `http://`, sets `with_virtual_hosted_style_request(false)` for path-style addressing, and sets `with_client_options(ClientOptions::new().with_connect_timeout(..).with_timeout(..).with_retry(RetryConfig::new().with_max_retries(2)))` so a dead endpoint fails inside 30 s; make `Objects::from_config` delegate to it
- [ ] T006 Add `Objects::list_objects()` returning `(key, size)` for every object using `ObjectStore::list(None)` and `futures_util::StreamExt`, plus `Objects::head_size(key)`, `Objects::put_overwrite(key, bytes)`, and `Objects::read(key)` reusing the existing error mapping in `src/objects.rs`
- [ ] T007 Add `Objects::verify_writable()` in `src/objects.rs` (write then delete `__scout/health/<uuid>`; skip for the in-memory backend) and call it from `src/main.rs` before serving, aborting startup with endpoint, bucket, and likely cause on failure
- [ ] T008 Unit tests in `src/objects.rs` for `S3Settings` validation (all fields required), `http://` enabling plain HTTP, path-style selection, and `Objects::from_config` still falling back to memory when endpoint/bucket are absent

**Checkpoint**: Foundation ready — deployment, migration, and failure-handling stories can proceed.

---

## Phase 3: User Story 1 — Deploy Scout with Garage from the shipped stack (Priority: P1) 🎯 MVP

**Goal**: The single compose stack runs Scout against Garage, with the operator performing the
documented Garage CLI setup, and ingest/retrieval/retention work unchanged.

**Independent Test**: On a clean host, follow the quick start, run the Garage CLI steps, start the
stack, ingest one event with an attachment, and fetch byte-identical raw bytes.

### Tests for User Story 1 ⚠️

- [ ] T009 [P] [US1] Assert the existing ingest, raw-payload, attachment, and retention tests still pass unchanged with the refactored `Objects` API in `src/tests.rs` (no behavioural change expected)

### Implementation for User Story 1

- [ ] T010 [US1] Replace the `minio` and `minio-init` services in `docker-compose.yml` with a pinned `garage` service: image tag (verify and record the exact current stable tag in `contracts/garage-deployment.md`), `deploy/garage.toml` mounted read-only, ports `3900:3900`, admin port `3903:3903`, `garage-data` volume, and a `/garage status` healthcheck; update `scout.depends_on` to wait for it and drop the MinIO volume
- [ ] T011 [US1] Point the `scout` service environment in `docker-compose.yml` at Garage: `SCOUT_S3_ENDPOINT: http://garage:3900`, `SCOUT_S3_BUCKET: scout-raw`, `AWS_REGION: garage`, credentials mapped from `SCOUT_S3_ACCESS_KEY`/`SCOUT_S3_SECRET_KEY`, and depend on the `garage` healthcheck
- [ ] T012 [US1] Rewrite the quick start and deployment sections of `README.md`: remove MinIO, add the `garage layout assign`/`layout apply`/`key create`/`bucket create`/`bucket allow` sequence, the new environment values, the sentinel-probe failure meaning, and record the measured clean-host elapsed time in `quickstart.md` once verified
- [ ] T013 [US1] Update `architecture.md` storage and deployment sections to name Garage as the default S3-compatible store and state that provider choice remains configuration

**Checkpoint**: US1 works on a Docker host; this is the MVP.

---

## Phase 4: User Story 2 — Move an existing MinIO-backed deployment to Garage (Priority: P2)

**Goal**: `scout migrate-objects` copies raw payloads and attachments from a source store into Garage,
verifies content, skips identical copies, repairs differing ones, and reports counts.

**Independent Test**: Seed a source store with raw payloads and attachments, run the command twice, and
confirm `found=copied` then `copied=0/skipped=found`, verified counts, preserved keys, and byte-identical
reads after switching the destination.

### Tests for User Story 2 ⚠️

- [ ] T014 [P] [US2] Tests in `src/migrate.rs` over two `Objects::memory()` stores: every key is copied with identical bytes, the destination holds the same key set, counts satisfy `found = skipped + copied + failed` where `copied` is verified-only, and a copy that fails its re-read is counted only in `failed`/`mismatched`, never in `copied`
- [ ] T015 [P] [US2] Tests in `src/migrate.rs`: a second run reports `copied = 0` and `skipped = found`; a destination object with different bytes is overwritten and verified; an unreadable source object is counted and named without aborting the run, and the run requests exit status `1`
- [ ] T016 [P] [US2] Tests in `src/migrate.rs` for `MigrationSettings::from_env`: each missing `SCOUT_MIGRATE_SOURCE_*` variable (`ENDPOINT`, `BUCKET`, `ACCESS_KEY_ID`, `SECRET_ACCESS_KEY`, `REGION`) is rejected with a message naming it, and secrets never appear in the error text

### Implementation for User Story 2

- [ ] T017 [US2] Implement `MigrationSettings::from_env` in `src/migrate.rs`: source via `S3Settings::for_migration_source()` from `SCOUT_MIGRATE_SOURCE_ENDPOINT`, `SCOUT_MIGRATE_SOURCE_BUCKET`, `SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID`, `SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY`, `SCOUT_MIGRATE_SOURCE_REGION`; destination via `S3Settings::from_env()` (`SCOUT_S3_ENDPOINT`, `SCOUT_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`)
- [ ] T018 [US2] Implement the copy-and-verify algorithm in `src/migrate.rs` (research R5): list the source, **skipping keys under the `__scout/` prefix** so crashed probe sentinels are never copied; read source bytes once, SHA-256, read destination, skip when the hash matches, `PutMode::Create` when absent, `PutMode::Overwrite` when different, then re-read and compare; count `SkippedIdentical`/`CopiedNew`/`CopiedOverwrite`/`CopyFailed`/`Mismatch`/`Unreadable` per `data-model.md`
- [ ] T019 [US2] Add bounded concurrency of 8 with `tokio::sync::Semaphore` and `tokio::task::JoinSet` while listing sequentially, in `src/migrate.rs`
- [ ] T020 [US2] Emit the contract's log lines and final summary (`found`, `copied`, `skipped`, `verified`, `mismatched`, `failed`, elapsed), increment `copied` only after the re-read verifies, log each failed key (a failed copy logs `outcome=copy_failed`), and return exit status `0` only when `found == verified` and `failed == 0`, otherwise exit with `std::process::exit(1)` after printing the summary; wire the result through `src/main.rs`
- [ ] T021 [US2] Document the migration procedure in `README.md`: source variable table, copy stage then operator-scheduled downtime and final pass, local and `docker compose run --rm scout migrate-objects` invocations, and the re-run/corruption expectations from `contracts/migration-cli.md`

**Checkpoint**: US1 and US2 both work independently.

---

## Phase 5: User Story 3 — Understand and recover from storage misconfiguration (Priority: P3)

**Goal**: Missing bucket, bad credentials, and unreachable endpoints fail fast with actionable messages
and never acknowledge an unstored event.

**Independent Test**: Start with each misconfiguration and confirm failure within 30 seconds, a message
naming the setting, and no accepted telemetry.

### Tests for User Story 3 ⚠️

- [ ] T022 [P] [US3] Test `Objects::verify_writable()` in `src/objects.rs`: an unreachable endpoint (a closed loopback port) returns an actionable error naming the endpoint and bucket without printing the secret, and the failure returns within the configured bound (≤30 s)
- [ ] T023 [P] [US3] Test in `src/tests.rs` that the existing `503` storage-failure path still returns a retryable response and acknowledges nothing when the metadata/object store is unavailable

### Implementation for User Story 3

- [ ] T024 [US3] Refine `Objects::verify_writable()` failure messages in `src/objects.rs` to distinguish missing bucket, invalid credentials, and unreachable host, following `contracts/garage-deployment.md`
- [ ] T025 [US3] Add the misconfiguration section to `quickstart.md` (missing bucket, wrong secret, closed port) with the expected message shape and the 30-second bound

**Checkpoint**: All storage failure modes are explicit and safe.

---

## Phase 6: User Story 4 — Keep working with any S3-compatible provider (Priority: P4)

**Goal**: Provider choice remains configuration; nothing depends on Garage-specific behaviour.

### Tests for User Story 4

- [ ] T026 [P] [US4] Test in `src/objects.rs` that `S3Settings` for a non-Garage endpoint (different region, HTTPS) builds successfully and that no code path inspects the provider name

### Implementation for User Story 4

- [ ] T027 [US4] State the provider-neutral contract in `README.md` (Garage is the default; MinIO and other S3-compatible stores remain supported with their own endpoint, bucket, key, secret, and region) and keep endpoint/region out of code

---

## Phase 7: Polish & Cross-Cutting Concerns

- [ ] T028 [P] Search the repository for remaining MinIO references in `README.md`, `architecture.md`, `docker-compose.yml`, and `src/` and confirm each remaining mention is an explicit "still supported" statement or a migration input
- [ ] T029 [P] Update `quickstart.md` with the automated-check commands and the Docker/Garage environment gate so the unverified part is explicit
- [ ] T030 Run `docker compose config` to validate the compose file without starting containers, and record any warning in `quickstart.md`
- [ ] T031 From the repository root, run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, UI `cargo check --target wasm32-unknown-unknown`, and `trunk build --release`
- [ ] T032 Remove dead helpers or scaffolding introduced during the slices in `src/migrate.rs` and `src/objects.rs`, then re-run clippy

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately.
- **Foundational (Phase 2)**: Depends on Setup — **blocks every story**.
- **US1 (Phase 3)**: After Foundational; independent of US2–US4.
- **US2 (Phase 4)**: After Foundational; independent of US1, but its README section follows T012.
- **US3 (Phase 5)**: After Foundational (probe in T007); independent of US1/US2.
- **US4 (Phase 6)**: After Foundational; documentation-only overlap with T012/T021.
- **Polish (Phase 7)**: After the stories you intend to ship.

### Within Each Story

Tests first; then settings, algorithm, and reporting for US2; compose before documentation for US1.
A story is done only when its tests pass through the real code paths.

### Parallel Opportunities

- Setup: T001, T002, T003 are independent files.
- Foundational: T005 and T006 touch `src/objects.rs` sequentially; T007/T008 are independent.
- US2: T014, T015, T016 can be written together; T017–T019 are sequential in `src/migrate.rs`; T021 is docs.
- US3: T022 and T023 are independent.
- Polish: T028, T029 are independent; T030–T032 are gates.

---

## Parallel Example: User Story 2

```bash
Task: "Tests for copy-all and key preservation in src/migrate.rs"
Task: "Tests for re-run skip, overwrite, and unreadable-source failure in src/migrate.rs"
Task: "Tests for missing source variables in src/migrate.rs"
```

## Parallel Example: User Story 3

```bash
Task: "Probe failure message test in src/objects.rs"
Task: "503 storage-failure regression test in src/tests.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1)

1. Phase 1 Setup → Phase 2 Foundational.
2. Phase 3 US1: compose and documentation for Garage.
3. **STOP and VALIDATE** on a Docker host with the quickstart.

### Incremental Delivery

1. Foundation ready.
2. US1 → clean-host Garage deployment.
3. US2 → migration command and procedure.
4. US3 → fail-fast misconfiguration.
5. US4 → provider-neutrality documentation and regression.
6. Polish → gates and environment record.

## Notes

- [P] tasks touch different files with no incomplete dependency.
- Secrets are never logged; environment validation names the variable, not the value.
- Object keys are copied verbatim; the source store is never modified.
- Migration and ingestion never coordinate; the operator schedules downtime.
- The Garage compose path needs Docker, which this workspace lacks; that gate is recorded in
  `quickstart.md` and must not be reported as verified here.
