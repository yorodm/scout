# Phase 0 Research: Garage Object Storage

All unknowns from the specification and Technical Context are resolved here. Decisions apply to the
implementation unless a task says otherwise.

## R1. Migration is a mode of the same binary

- **Decision**: `scout migrate-objects` is detected from `std::env::args()` at the top of `main`
  before any Actix, worker, or retention wiring. The mode builds its own two storage clients, runs
  the copy, prints a summary, and returns an exit status. No argument parser dependency is added.
- **Rationale**: The spec requires a dedicated entry point that runs no part of the service
  (FR-016), and the constitution requires one deployment unit (Principle IV). A subcommand keeps a
  single artifact while making the entry point explicit in `docker run`/compose commands.
- **Alternatives considered**: (a) A separate `scout-migrate` binary — rejected: second artifact to
  ship and document. (b) A runtime flag on the server — rejected: it would start the service. (c) A
  separate crate/workspace member — rejected by Principle IV.

## R2. Configuration sources

- **Decision**: The destination uses the runtime settings already present in the environment:
  `SCOUT_S3_ENDPOINT`, `SCOUT_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
  `AWS_REGION`. The source uses `SCOUT_MIGRATE_SOURCE_ENDPOINT`, `_BUCKET`, `_ACCESS_KEY_ID`,
  `_SECRET_ACCESS_KEY`, `_REGION`.
- **Rationale**: Matches clarification 1. The operator already has destination settings in their
  deployment file; only the old store needs new values. Secrets stay out of argv and shell history.
- **Validation**: All five source values are required; a missing one aborts with a message naming the
  variable. The destination reuses the existing validation (`endpoint` and `bucket` required).
- **Alternatives considered**: (a) CLI flags — rejected: secrets in process lists and shell history.
  (b) A config file — rejected: a second configuration format for one command. (c) Both stores under
  new prefixed variables — rejected: duplicates the deployment's existing configuration.

## R3. Building the two S3 clients

- **Decision**: Add an explicit settings builder in `src/objects.rs` that constructs an
  `AmazonS3Builder` from `(endpoint, bucket, region, access key, secret key)` instead of relying on
  `AmazonS3Builder::from_env()`. Use `with_allow_http` when the endpoint starts with `http://`,
  `with_virtual_hosted_style_request(false)` so requests are path-style, and client options with
  bounded connect/request timeouts and a retry cap so an unreachable endpoint fails inside 30 seconds.
  `S3Settings::from_env()` reads the destination values (`SCOUT_S3_*`, `AWS_*`); the runtime store is
  built from `Config` through the same helper.
- **Rationale**: Garage's S3 gateway is reached over plain HTTP in the compose stack and does not need
  wildcard DNS, so path-style addressing works with `http://garage:3900`. Explicit credentials make
  the migration's source and destination independent of the process environment used by the server.
- **Alternatives considered**: (a) Two `from_env` builders with temporary environment mutation —
  rejected: unsafe and racy. (b) Virtual-hosted style — rejected: requires wildcard DNS and TLS setup
  that the single-node stack does not provide.

## R4. Listing the source

- **Decision**: Use `ObjectStore::list(None)` and consume the returned stream with
  `futures_util::StreamExt`. Skip keys under the `__scout/` prefix so a crashed startup probe's
  sentinel can never be copied. Add `futures-util` as a direct dependency (no default features beyond
  `std`).
- **Rationale**: Listing is the only way to discover keys; the stream adapter is already in the
  dependency graph through `object_store`, so a direct dependency adds no new transitive code.
- **Alternatives considered**: Manual `poll_next` plumbing — rejected: fragile and harder to read.
  Prefix-by-prefix listing — rejected: keys are already flat under `raw/` and `attachments/`.

## R5. Copy-and-verify algorithm

- **Decision**: For each listed source object:
  1. Read the source bytes once and compute their SHA-256.
  2. Read the destination object for the same key, if present.
  3. If the destination hash matches, count it skipped and verified.
  4. If it differs, write the source bytes with `PutMode::Overwrite`; if it is absent, write with
     `PutMode::Create`.
  5. Re-read the destination and compare hashes.
  6. Only a re-read that matches increments `copied` and `verified`; a copy whose re-read differs or
     fails increments `failed` (and `mismatched` when the bytes differ) and never `copied`.
- **Rationale**: Matches clarification 5 and the spec's `skipped`/`copied`/`verified`/`mismatched`
  counts. Re-reading after the write is the only way to prove the destination actually stored the
  bytes rather than trusting a successful response.
- **Alternatives considered**: (a) ETag comparison — rejected: multipart ETags are not content hashes.
  (b) Trusting `PUT` success without re-reading — rejected: SC-003 requires verified content.
  (c) Byte-for-byte in-memory comparison — equivalent but slower to report; hashing is already used by
  the project for credentials.

## R6. Concurrency

- **Decision**: Run up to 8 object transfers concurrently, coordinated with a `tokio::sync::Semaphore`
  and a `JoinSet`; keep the listing sequential so memory stays bounded.
- **Rationale**: Offline migrations of large stores should not take hours, and 8 is conservative for a
  single-node Garage and for typical object-store APIs.
- **Alternatives considered**: Fully sequential (simplest, but slow); unbounded fan-out (risks
  overwhelming the source or destination).

## R7. Failure semantics and exit codes

- **Decision**: Unreadable source or destination objects are logged with the key, counted as failures,
  and do not stop the run (so one bad object does not hide the rest). The final summary prints found,
  copied, skipped, verified, mismatched, and failed counts, then exits `1` if any failure occurred and
  `0` otherwise. Configuration errors exit `1` before any listing, naming the missing variable or
  setting.
- **Rationale**: FR-014 requires a non-zero exit that names the object; continuing gives the operator
  a complete picture in one run. A single exit code keeps scripting simple.
- **Alternatives considered**: Abort on first failure — rejected: hides the scale of the problem. Two
  different non-zero codes — deferred; one code is enough for the documented procedure.

## R8. Startup storage verification

- **Decision**: In server mode, after building the store, call `Objects::verify_writable()` (in
  `src/objects.rs`), which performs a write/delete probe of a unique sentinel key
  (`__scout/health/<uuid>`) when the backend is S3. Failure aborts startup with a message naming the
  endpoint and bucket and the likely cause (missing bucket, bad credentials, unreachable host).
  Memory-backed runs skip the probe.
- **Rationale**: FR-005 requires failing before telemetry is accepted. A `HEAD` probe cannot
  distinguish a missing bucket from a missing object; a create/delete of a sentinel proves write
  access and bucket existence without touching payloads.
- **Alternatives considered**: `HEAD` on a known object — rejected: ambiguous. Relying on the first
  ingest to fail — rejected by the spec.

## R9. Compose and Garage configuration

- **Decision**: Replace the `minio` and `minio-init` services with a pinned `dxflrs/garage` image,
  a mounted `deploy/garage.toml`, ports `3900:3900` (S3) and `3903:3903` (admin), and a
  `garage-data` volume. The compose file does **not** configure layout, buckets, or keys; the README
  documents the `garage layout assign`, `garage layout apply`, `garage key create`,
  `garage bucket create`, and `garage bucket allow` steps (clarification 4). Scout points at
  `http://garage:3900`, bucket `scout-raw`, region `garage`, using the key/secret the operator
  created.
- **Rationale**: Matches the documented decision and Garage's own quick start. Keeping a single-node
  default in `garage.toml` makes the dev stack usable while leaving production layout to the operator.
- **Alternatives considered**: An init container calling the Garage CLI — explicitly rejected by the
  clarification. No Garage in compose — rejected: SC-001's clean-host flow.

## R10. Object keys and formats

- **Decision**: Reuse `raw/{project}/{event}` and `attachments/{project}/{event}/{index}` verbatim on
  both stores. The migration copies keys as-is; no renaming, re-encoding, or metadata rewriting.
- **Rationale**: FR-004 and the spec assumption that the feature moves bytes, not shapes. Keys are
  opaque to the migration.

## R11. Operational procedure (documentation contract)

- **Decision**: Document migration as two independent stages:
  1. **Copy stage (service can keep running or be stopped by the operator):** run
     `scout migrate-objects` with source variables set; it copies and verifies, and it may be re-run
     safely.
  2. **Cutover (operator-scheduled downtime):** stop the service, run the command once more to catch
     up, switch the destination settings to Garage, start the service.
  The tool never talks to the service, never pauses ingest itself, and never deletes source objects.
- **Rationale**: Clarifications 2–3 and FR-009/FR-017. The ordering gives the operator a verified copy
  before any irreplaceable data stops being written to the old store.
- **Alternatives considered**: Automatic quiescing or dual-write — rejected by the spec.

## R12. Testing strategy

- **Decision**:
  1. Migration unit/integration tests in `src/migrate.rs` using two `Objects::memory()` instances:
     copy-all, skip-identical on re-run, overwrite-differing, unreadable-source failure, count accuracy,
     and key preservation.
  2. Settings tests: missing source variables are rejected with the variable name; endpoint scheme
     selects HTTP; path-style is requested.
  3. Existing ingest/retrieval/retention tests continue to pass unchanged (regression for SC-007).
  4. Compose smoke path in `quickstart.md`: bring up Garage, run the documented CLI setup, ingest,
     fetch raw bytes, run retention, and run a migration between two buckets. Requires Docker and is
     an explicit environment gate in this workspace.
- **Rationale**: Constitution Principle V asks for real paths; in-memory `ObjectStore` is a real
  implementation of the same trait, and the compose path is documented for the parts only Garage can
  prove.
- **Alternatives considered**: Mocking `ObjectStore` — rejected: the in-memory backend already
  exercises the trait contract.

## R13. Container entrypoint

- **Decision**: Change the runtime image from `CMD ["scout"]` to `ENTRYPOINT ["scout"]` so a
  container argument becomes the Scout subcommand. `docker compose up` (no arguments) still starts the
  server; `docker compose run --rm scout migrate-objects` runs the migration mode.
- **Rationale**: Without this, `docker compose run <service> <args>` replaces `CMD` and would try to
  execute `migrate-objects` as a binary. The entrypoint form keeps one image and one documented
  invocation.
- **Alternatives considered**: (a) Keep `CMD` and document `docker run <image> scout migrate-objects` —
  rejected: awkward and inconsistent between local and container use. (b) A second image — rejected
  by Principle IV.

## Resolved specification gaps

| Gap | Resolution |
|---|---|
| Exact environment variable names | R2 (`SCOUT_MIGRATE_SOURCE_*`) |
| How two stores are authenticated | R3 (explicit settings; secrets from environment) |
| Cutover sequencing | R11 (copy stage, then operator-scheduled downtime and a final pass) |
| Destination conflict handling | R5 (skip identical, overwrite differing, fail on unreadable) |
| Migration throughput | R6 (8 concurrent transfers); no hard target in this slice |
| Startup verification of bucket existence | R8 (write/delete sentinel probe) |
