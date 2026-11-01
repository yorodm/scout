# Implementation Plan: Garage Object Storage

**Branch**: `003-garage-storage` | **Date**: 2026-11-01 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/003-garage-storage/spec.md`

## Summary

Replace MinIO with Garage in the shipped single-compose deployment and add a dedicated
`scout migrate-objects` entry point that copies raw payloads and attachments from an existing
store into Garage, verifying content identity and reporting counts. The runtime S3 client stays the
same shape (endpoint, bucket, credentials, region), object keys and formats do not change, and any
S3-compatible provider remains supported. Documentation replaces MinIO with Garage and walks
through the Garage CLI steps the shipped compose no longer performs automatically.

## Technical Context

**Language/Version**: Rust 2021, stable toolchain

**Primary Dependencies**: Actix Web 4, `object_store` 0.12 (AWS), `tokio`, `sha2`, `tracing`.
One new direct dependency: `futures-util` (consuming the object-store listing stream).

**Storage**: PostgreSQL for metadata (unchanged); S3-compatible object storage, defaulting to Garage
in `docker-compose.yml`. No schema or object-key changes. `AWS_*` destination values are read in
`objects::S3Settings::from_env`, not added to `Config`.

**Testing**: `cargo test` with plain `#[tokio::test]` migration tests over `object_store`'s in-memory
backend, plus the existing `#[actix_web::test]` integration suite. Real Garage is exercised by the
compose smoke path in `quickstart.md` (requires Docker; unavailable in this workspace).

**Target Platform**: Linux server, single binary plus PostgreSQL and Garage from one compose file.

**Project Type**: Web service + WASM UI in one cargo workspace, single binary crate.

**Performance Goals**: No hard migration throughput target. Ingest/retrieval/retention latency must be
unchanged; the migration logs progress so large stores are observable.

**Constraints**: No external object-storage tooling; migration must be a mode of the same binary and
must not start the HTTP server, ingest, workers, or retention. Secrets come from the environment.
Migration and ingestion never coordinate; the operator schedules downtime.

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
| V. Evidence over assumption: test the real paths | PASS | Migration logic is covered in-crate against real `ObjectStore` implementations; the compose smoke path exercises Garage itself (env-gated, like previous deployment checks). |
| Security constraints | PASS | Credentials come from environment variables; no secret is logged; the migration never prints object content. |
| Performance/reliability constraints | PASS | Ingest limits and `503` storage-failure behaviour are unchanged; startup now proves write access to the bucket before serving. |
| Compatibility and Scope (v1.1.0) | PASS | Garage is reached through the S3 interface; no provider-specific I/O and no widening of the product surface. |
| Development Workflow & Quality Gates | PASS | Slices end with `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`; docs and quickstart updated in the same feature. |

**Gate result: PASS.** No violations; see Complexity Tracking for the overwrite rationale and the new
dependency.

## Post-Design Constitution Re-check

Re-evaluated after `research.md`, `data-model.md`, `contracts/`, and `quickstart.md` were written:

- **I**: Migration is a separate entry point; no ingest code path changes. **PASS**
- **II**: Source objects are never written or deleted; the destination is not live until cutover, so
  repairing a differing copy cannot rewrite an authoritative payload. **PASS**
- **III**: No API, session, or authorisation change. **PASS**
- **IV**: One binary (entrypoint change only), one compose unit; no new service. **PASS**
- **V**: In-memory object-store tests exercise the real trait; the Garage path is a documented compose
  gate (Docker unavailable here, recorded as an environment gate in quickstart). **PASS (with gate)**
- **Security**: Secrets only from the environment; never logged; the probe uses a unique sentinel and
  deletes it. **PASS**
- **Reliability**: Startup proves write access before serving; ingest keeps returning `503` on storage
  failure. **PASS**
- **Compatibility and Scope**: Provider-neutral S3 usage; Garage is the default and documentation, not
  a hard dependency. **PASS**

**Post-design result: PASS.** No new violations.

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
