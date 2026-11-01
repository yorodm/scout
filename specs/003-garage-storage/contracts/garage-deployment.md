# Contract: Garage Deployment

**Feature**: 003-garage-storage | **Status**: design contract

## Compose shape

| Aspect | Contract |
|---|---|
| Service name | `garage` in `docker-compose.yml` (replaces `minio` and `minio-init`). |
| Image | Pinned to `dxflrs/garage:v2.5.0` (published multi-arch image; verified on Docker Hub). The tag is recorded in the compose file and the README. |
| Ports | `3900:3900` (S3 API); `127.0.0.1:3903:3903` (admin API, local only). |
| Configuration | `deploy/garage.toml` mounted read-only at `/etc/garage.toml`. |
| Data | Named volume `garage-data:/var/lib/garage`, persisting both `metadata_dir` and `data_dir`. |
| Healthcheck | Runs Garage's own status command (`/garage status`); the `scout` service waits for it. |
| Scout dependency | `scout` depends on `postgres` (healthy) and `garage` (healthy). |
| Provisioning | The compose file does **not** create layout, buckets, or keys. The README documents those steps. |

## `deploy/garage.toml`

Required keys and their development defaults:

| Key | Value |
|---|---|
| `metadata_dir` | `/var/lib/garage/meta` |
| `data_dir` | `/var/lib/garage/data` |
| `db_engine` | `sqlite` (single-node default) |
| `replication_factor` | `1` |
| `rpc_bind_addr` | `[::]:3901` |
| `rpc_secret` | A development value; the README instructs operators to replace it. |
| `s3_api.api_bind_addr` | `[::]:3900` |
| `s3_api.s3_region` | `garage` |
| `s3_api.root_domain` | `.s3.garage.localhost` (unused with path-style addressing) |
| `admin.api_bind_addr` | `[::]:3903` |
| `admin.admin_token` | A development value; replace outside a local stack. |

## Documented setup sequence

The README must contain these steps verbatim enough to follow without leaving the quick start:

```sh
docker compose up -d garage postgres

# Node id from the status output
docker compose exec garage /garage status
docker compose exec garage /garage layout assign -z dc1 -c 1G <node-id>
docker compose exec garage /garage layout apply --version 1

# Credentials for Scout
docker compose exec garage /garage key create scout-key
docker compose exec garage /garage bucket create scout-raw
docker compose exec garage /garage bucket allow --read --write --owner scout-raw --key scout-key

# Then start Scout with the key id and secret in the environment
docker compose up -d scout
```

The operator copies the key id and secret printed by `garage key create` into
`SCOUT_S3_ACCESS_KEY` / `SCOUT_S3_SECRET_KEY` (the compose file maps them to `AWS_ACCESS_KEY_ID` /
`AWS_SECRET_ACCESS_KEY`).

## Scout environment mapping

| Variable | Value in the shipped stack |
|---|---|
| `SCOUT_S3_ENDPOINT` | `http://garage:3900` |
| `SCOUT_S3_BUCKET` | `scout-raw` |
| `AWS_ACCESS_KEY_ID` | Operator-created Garage key id |
| `AWS_SECRET_ACCESS_KEY` | Operator-created Garage secret |
| `AWS_REGION` | `SCOUT_S3_REGION` (defaults to `garage`) |

Plain HTTP and path-style addressing are used; the shipped stack does not require TLS or wildcard DNS.

## Provider neutrality

The same environment variables configure any S3-compatible provider. Garage is the default and the
documented setup; MinIO and other providers remain supported by supplying their endpoint, bucket, key,
secret, and region. No endpoint is hard-coded in the application.

## What is deliberately not automated

- Cluster layout assignment and application.
- Bucket creation and access-key creation.
- Production `rpc_secret` and admin token generation.
- Replication and placement policy beyond the single-node development default.

These remain operator decisions, as recorded in the specification's clarification and assumptions.
