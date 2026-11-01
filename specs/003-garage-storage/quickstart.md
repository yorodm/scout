# Quickstart: Garage Object Storage

Runnable validation for feature 003. Shapes and guarantees live in
[`contracts/`](./contracts/); entities in [`data-model.md`](./data-model.md).

## Prerequisites

- Docker with the compose plugin, and a checkout of this repository.
- Rust stable and `cargo` (for the automated checks and local migration runs).
- Ports `8080`, `5432`, `3900`, and `3903` free.
- No MinIO installation is required anywhere in this guide.

## 1. Start Garage and PostgreSQL

Generate the credentials and bucket name once, then start Garage. Compose runs
`/garage server --single-node --default-bucket`, so the layout, bucket, and access key are created
automatically:

```sh
export SCOUT_S3_ACCESS_KEY="GK$(openssl rand -hex 16)"
export SCOUT_S3_SECRET_KEY="$(openssl rand -hex 32)"
export SCOUT_S3_DEFAULT_BUCKET=scout-raw

docker compose up -d garage postgres
docker compose exec garage /garage status
docker compose exec garage /garage bucket list
```

Expected: `garage status` shows one healthy node with an assigned layout, and `bucket list` contains
`scout-raw`. Manual `layout assign` / `key create` / `bucket create` is only needed for Garage versions
before 2.3.0 or a custom layout.

## 2. Start Scout against Garage

```sh
export SCOUT_S3_ENDPOINT=http://127.0.0.1:3900
export SCOUT_S3_BUCKET="${SCOUT_S3_DEFAULT_BUCKET:-scout-raw}"
export AWS_ACCESS_KEY_ID="$SCOUT_S3_ACCESS_KEY"
export AWS_SECRET_ACCESS_KEY="$SCOUT_S3_SECRET_KEY"
export AWS_REGION=garage
export SCOUT_OPERATOR_EMAIL=operator@scout.local
export SCOUT_OPERATOR_PASSWORD=change-me-now
export SCOUT_DEMO_MODE=true
cargo run
```

Expected: startup succeeds (the sentinel write/delete probe passes) and `GET /api/health` returns
`200`. With a missing bucket or wrong credentials, startup fails immediately naming the endpoint and
bucket and the likely cause.

## 3. Ingest and verify stored bytes

Send an envelope with an attachment and an event id (see the feature 001 quickstart for the full
payload), then:

```sh
curl -s http://127.0.0.1:8080/api/health
# Sign in, list issues, and fetch the raw payload:
curl -s -H "authorization: Bearer <SESSION>" http://127.0.0.1:8080/api/issues/<issue-id>/raw -o /tmp/raw.bin
sha256sum /tmp/raw.bin
```

Expected: the stored bytes hash to the same value as the event body you sent, and the attachment
metadata resolves to a retrievable object in the Garage bucket.

## 4. Retention still deletes only objects

Set a project retention window of zero days (or use the demo settings screen), run the retention
sweep, and confirm the issue still lists with its metadata while the raw payload endpoint reports it is
no longer available. The Garage bucket no longer holds that key.

## 5. Misconfiguration fails fast

Repeat startup with each of these and confirm the process exits before binding `:8080`, within 60
seconds, prints no secret, and accepts no telemetry. Every message names the endpoint and bucket. A
closed port returns immediately; to exercise the bound, use an address that drops packets and confirm
failure within 60 seconds.

| Misconfiguration | Expected message shape |
| --- | --- |
| `SCOUT_S3_BUCKET=missing-bucket` | `object storage startup check failed: object storage write check failed for bucket missing-bucket at http://127.0.0.1:3900: … Create the bucket and grant the access key read, write, and delete permission.` |
| `AWS_SECRET_ACCESS_KEY=wrong` | Same prefix, ending `The object store rejected the credentials; check the access key and secret.` |
| `SCOUT_S3_ENDPOINT=http://127.0.0.1:3999` | Same prefix, ending `Check that the endpoint is reachable, that the object store is running, and that the bucket exists.` |

## 6. Migration between two buckets (the MinIO rehearsal)

The same steps apply when the source is MinIO; a second Garage bucket keeps the rehearsal local.

```sh
# Create a destination bucket in addition to the live one
docker compose exec garage /garage bucket create scout-raw-new
docker compose exec garage /garage bucket allow --read --write --owner scout-raw-new --key "$SCOUT_S3_ACCESS_KEY"
```

1. Run Scout with `SCOUT_S3_BUCKET=scout-raw` (the "old" store) and ingest two events.
2. Stop Scout.
3. Run the migration from the old bucket to the new one:

```sh
SCOUT_MIGRATE_SOURCE_ENDPOINT=http://127.0.0.1:3900 \
SCOUT_MIGRATE_SOURCE_BUCKET=scout-raw \
SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID=<key> \
SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY=<secret> \
SCOUT_MIGRATE_SOURCE_REGION=garage \
SCOUT_S3_ENDPOINT=http://127.0.0.1:3900 \
SCOUT_S3_BUCKET=scout-raw-new \
AWS_ACCESS_KEY_ID=<key> \
AWS_SECRET_ACCESS_KEY=<secret> \
AWS_REGION=garage \
cargo run -- migrate-objects
```

In the container: `docker compose run --rm scout migrate-objects`.

Expected summary: `found=4 copied=4 skipped=0 verified=4 mismatched=0 failed=0`, exit `0`
(two raw payloads plus two attachments).

4. Re-run the same command. Expected: `copied=0 skipped=4 verified=4`, exit `0`.
5. Corrupt one object in the destination (upload different bytes under the same key with a Garage
   client), re-run, and confirm it is overwritten and verified.
6. Start Scout with `SCOUT_S3_BUCKET=scout-raw-new` and confirm both issues return byte-identical raw
   payloads and attachments.

## 7. Automated checks

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo clippy --target wasm32-unknown-unknown --manifest-path ui/Cargo.toml -- -D warnings
(cd ui && trunk build --release)
```

The migration suite runs against two in-memory object stores and covers copy-all, skip-on-rerun,
overwrite-differing, unreadable-source failure, counts, and key preservation. These additional checks
cover the failure paths without Docker:

- `cargo test probe_failure_names_endpoint` — a closed endpoint fails the startup probe, naming the
  endpoint and bucket and never the secret.
- `cargo test verify_writable_times_out_on_blackholed_endpoint -- --ignored` — a listener that accepts
  but never responds fails the startup probe within 60 seconds, naming the endpoint and bucket and
  never the secret. It is ignored by default because it waits on real timeouts.
- `cargo test unreachable_object_store` — ingest returns `503`, stores nothing, and `/api/health`
  reports `degraded` while the object store is unreachable.
- `cargo test objects` and `cargo test migrate` cover the settings, list/head/overwrite helpers, and
  the copy/verify outcomes end to end.

## 8. Regression against another S3-compatible provider

Point `SCOUT_S3_ENDPOINT`, bucket, key, secret, and region at a non-Garage provider (MinIO included)
and repeat steps 3–4. Expected: identical behaviour, confirming the feature is a default and a
documentation change, not a Garage-only dependency.

## Environment gate

Steps 1–6 require Docker and a running Garage. The workspace used to build this feature has no Docker,
so those steps are a completion gate rather than an automated check; the in-memory suite and code
review cover the logic. When the gate is run, also run `docker compose config` and confirm it reports
no warnings, and record the verified migration counts. SC-001 has no time bound: the gate is that the
documented steps complete on a clean host with no MinIO and no undocumented manual step.
