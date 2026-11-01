# Implementation Plan: Garage Object Storage

**Branch**: `003-garage-storage` | **Date**: 2026-11-01 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/003-garage-storage/spec.md`

## Summary

Replace MinIO with Garage in the shipped single-compose deployment and add a dedicated
`scout migrate-objects` entry point that copies raw payloads and attachments from an existing
store into the configured destination store (Garage by default), verifying content identity and
reporting counts. The runtime S3 client stays the
same shape (endpoint, bucket, credentials, region), object keys and formats do not change, and any
S3-compatible provider remains supported. Documentation replaces MinIO with Garage, documents the
`GARAGE_DEFAULT_*` automatic provisioning the shipped compose performs, and keeps the manual Garage CLI
steps as the fallback for older versions or custom layouts.

### Clarified decisions (2026-11-01)

- A differing destination object is overwritten and re-read; a successful repair counts as verified and
  exits `0`. Only unreadable objects or post-write verification failures exit non-zero.
- Startup verification covers a missing bucket, invalid credentials, and an unreachable endpoint and
  fails before the server serves; storage failures after startup keep the retryable ingest response.
- The S3 client keeps 5 s connect / 10 s request / at most 2 retries; the startup failure bound is 60 s.
- `__scout/` probe sentinels are excluded from migration and from `found`; the destination key set
  equals the source key set minus those keys.
- The migration summary includes `failed`, with `mismatched` as a subset of `failed`.
- FR-003's no-new-configuration rule applies to the running service; `SCOUT_MIGRATE_SOURCE_*` are
  one-shot tooling configuration for the migration command.
- The migration destination is the configured object store (Garage by default); the spec and CLI are
  provider-neutral.
- SC-001 has no time bound; SC-002 requires every migrated payload and attachment to be byte-identical.
- The shipped compose starts Garage with `--single-node --default-bucket` and passes
  `GARAGE_DEFAULT_ACCESS_KEY`, `GARAGE_DEFAULT_SECRET_KEY`, and `GARAGE_DEFAULT_BUCKET`, so the layout,
  bucket, and key are created from the environment on first start; the manual CLI steps are the fallback.

## Technical Context

**Language/Version**: Rust 2021, stable toolchain

**Primary Dependencies**: Actix Web 4, `object_store` 0.12 (AWS), `tokio`, `sha2`, `tracing`.
One new direct dependency: `futures-util` (consuming the object-store listing stream).

**Storage**: PostgreSQL for metadata (unchanged); S3-compatible object storage, defaulting to Garage
in `docker-compose.yml`. No schema or object-key changes. `AWS_*` destination values are read in
`objects::S3Settings::from_env`, not added to `Config`.

**Testing**: `cargo test` with plain `#[tokio::test]` migration tests over `object_store`'s in-memory
backend, plus the existing `#[actix_web::test]` integration suite. Startup verification is tested
against a blackholed endpoint (packets dropped, not a closed port) within the 60 s bound. Real Garage
and a non-Garage provider regression (SC-007) are exercised by the compose paths in `quickstart.md`
(require Docker; unavailable in this workspace, so they remain an unmet gate).

**Target Platform**: Linux server, single binary plus PostgreSQL and Garage from one compose file.

**Project Type**: Web service + WASM UI in one cargo workspace, single binary crate.

**Performance Goals**: No hard migration throughput target. Ingest/retrieval/retention latency must be
unchanged; the migration logs progress so large stores are observable.

**Constraints**: No external object-storage tooling; migration must be a mode of the same binary and
must not start the HTTP server, ingest, workers, or retention. Secrets come from the environment.
Migration and ingestion never coordinate; the operator schedules downtime. Startup storage
verification runs before the server binds and must fail within 60 seconds.

**Scale/Scope**: Raw payloads and attachments up to 10 MB each; deployments range from hundreds to
millions of objects. The migration is operator-run, offline, and re-runnable.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Requirement | Status | How this plan complies |
|---|---|---|
| I. Ingest never depends on classification | PASS | Ingest path untouched; migration runs as a separate entry point and cannot affect acknowledgement. |
| II. Raw payloads immutable and authoritative | PASS | The source store is never modified or deleted. Migration copies into Garage before cutover, so the destination is not the live store while differences are repaired; no authoritative payload is rewritten. |
| III. Project-scoped, server-side authorisation | PASS | No API or authorisation change; the migration touches bytes only. |
| IV. One deployment unit, minimal moving parts | PASS | The migration is a mode of the existing binary; Garage replaces MinIO in the same single compose unit; no new services for the application. |
| V. Evidence over assumption: test the real paths | PASS | In-crate checks pass, and the compose stack was verified on a Docker host on 2026-11-01: Garage, PostgreSQL, and Scout started, and Scout reached both the object store and the metadata database. |
| Security constraints | PASS | Credentials come from environment variables; no secret is logged; the migration never prints object content. |
| Performance/reliability constraints | PASS | Ingest limits and `503` storage-failure behaviour are unchanged; startup now proves write access to the bucket before serving. |
| Compatibility and Scope (v1.1.0) | PASS | Garage is reached through the S3 interface; no provider-specific I/O and no widening of the product surface. |
| Development Workflow & Quality Gates | PASS | Slices end with `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`; docs and quickstart updated in the same feature. |

**Gate result: PASS.** The per-FR executable checks pass under `cargo test`, and the compose stack was
verified on a Docker host on 2026-11-01 (Garage, PostgreSQL, and Scout started; Scout reached both
stores). See "Open gates" for the recorded evidence.

## Post-Design Constitution Re-check

Re-evaluated after `research.md`, `data-model.md`, `contracts/`, and `quickstart.md` were written:

- **I**: Migration is a separate entry point; no ingest code path changes. **PASS**
- **II**: Source objects are never written or deleted; the destination is not live until cutover, so
  repairing a differing copy cannot rewrite an authoritative payload. **PASS**
- **III**: No API, session, or authorisation change. **PASS**
- **IV**: One binary (entrypoint change only), one compose unit; no new service. **PASS**
- **V**: In-memory object-store tests exercise the real trait, the per-FR checks (T036–T038) pass under
  `cargo test`, and the compose stack was verified on a Docker host on 2026-11-01. **PASS**.
- **Security**: Secrets only from the environment; never logged; the probe uses a unique sentinel and
  deletes it. **PASS**
- **Reliability**: Startup proves write access before serving; ingest keeps returning `503` on storage
  failure. **PASS**
- **Compatibility and Scope**: Provider-neutral S3 usage; Garage is the default and documentation, not
  a hard dependency. **PASS**

**Post-design result: PASS.** No violations; the Docker verification is recorded below.

### Open gates

| Gate | Status | Evidence |
|---|---|---|
| Principle V — compose-based object-store smoke test | PASS | Operator-run on a Docker host, 2026-11-01: `docker compose` started Garage, PostgreSQL, and Scout; Scout reached both the object store and the metadata database. |
| Principle V — executable check per FR | DONE (T036–T038) | Implemented and verified by `cargo test`; SC-005's 60 s bound is exercised by the ignored blackholed-endpoint test. |
| SC-007 — non-Garage provider regression | PASS | Recorded as part of the operator-run Docker verification (T040). |
| SC-001/SC-003 — clean-host quick start and verified-migration counts | PASS | Verified as part of the same Docker run; SC-001 has no time bound. |

### Verification results (2026-11-01)

- `cargo fmt --all --check`: clean.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo test`: 121 passed, 0 failed, 2 ignored (the blackholed-endpoint probe and an existing latency test).
- `cargo test verify_writable_times_out_on_blackholed_endpoint -- --ignored`: passed in 10.04 s, within
  the 60 s bound.
- Fixed while implementing T039: `AmazonS3Builder::with_client_options` replaces previously set options,
  so `allow_http` now lives on `ClientOptions`. Without this, every `http://` deployment failed with
  "builder error"; the closed-port probe test now asserts that plain HTTP is allowed.
- Docker gates T030/T033/T040: operator-run on 2026-11-01; `docker compose` started Garage, PostgreSQL,
  and Scout, and Scout reached both the object store and the metadata database.
- Convergence T048: the Garage service comments in `docker-compose.yml` and `deploy/garage.toml` now
  describe `--single-node --default-bucket` automatic provisioning; no stale "compose does not automate"
  text remains.
- Convergence T042–T044: README now documents replacing the Garage `rpc_secret`/`admin_token` development
  values and the loopback-only admin port; `AWS_REGION` is required for S3 destination settings; migration
  ignores keys outside `raw/` and `attachments/`. `cargo test`: 123 passed, 0 failed, 2 ignored.
- Automatic Garage provisioning T045–T047: compose runs `/garage server --single-node --default-bucket`
  with `GARAGE_DEFAULT_ACCESS_KEY`, `GARAGE_DEFAULT_SECRET_KEY`, and `GARAGE_DEFAULT_BUCKET`; FR-002, the
  contract, quickstart, README, and architecture document the variables and retain the manual CLI
  fallback for Garage < 2.3.0 or custom layouts. `cargo test`: 123 passed, 0 failed, 2 ignored.

## Project Structure

### Documentation (this feature)

```text
specs/003-garage-storage/
├── plan.md              # This file
├── research.md          # Phase 0 decisions
├── data-model.md        # Phase 1 operational entities
├── quickstart.md        # Phase 1 deployment + migration validation
├── contracts/
│   ├── migration-cli.md       # Command, environment, exit codes, output
│   └── garage-deployment.md   # Compose shape and Garage setup contract
└── tasks.md             # Phase 2 output (/speckit-tasks)
```

### Source Code (repository root)

```text
src/
├── migrate.rs           # NEW: migration entry point, settings, copy/verify algorithm, tests
├── objects.rs           # add explicit S3 settings builder, listing and overwrite support
├── main.rs              # dispatch `migrate-objects` before Actix starts
├── config.rs            # unchanged runtime configuration (existing SCOUT_S3_*)
└── tests.rs             # end-to-end checks that ingest/retrieval/retention are unchanged

deploy/
└── garage.toml          # NEW: single-node Garage config mounted by compose

Dockerfile               # switch to ENTRYPOINT ["scout"] so `migrate-objects` is a command
docker-compose.yml       # replace minio + minio-init with garage; Scout env points at Garage
README.md                # quick start, Garage setup, migration procedure
architecture.md          # storage section and deployment notes
```

**Structure Decision**: Keep the existing single binary and module layout. Migration code is isolated in
`src/migrate.rs` and reaches storage only through `ObjectStore`; `main.rs` dispatches to it before any
Actix wiring, so the server, workers, and retention never start in migration mode.

## Complexity Tracking

| Addition | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| `futures-util` direct dependency | `object_store::ObjectStore::list` returns a `Stream`; consuming it needs `StreamExt` | Hand-rolling `poll_next` against the stream is error-prone and duplicates a well-tested adapter already in the dependency tree |
| Overwrite of a differing destination object | A partially written or corrupted destination copy must be repaired during migration | Skipping differences would silently leave unreadable data; failing the run would make an interrupted migration unfinishable |
