# Contract: Garage Deployment

**Feature**: 003-garage-storage | **Status**: design contract

## Compose shape

| Aspect | Contract |
|---|---|
| Service name | `garage` in `docker-compose.yml` (replaces `minio` and `minio-init`). |
| Image | Pinned to `dxflrs/garage:v2.5.0` (published multi-arch image; verified on Docker Hub). The tag is recorded in the compose file and the README. |
| Command | `/garage server --single-node --default-bucket` (the image has no ENTRYPOINT, so the full argv is the compose `command`). |
| Ports | `3900:3900` (S3 API); `127.0.0.1:3903:3903` (admin API, local only). |
| Configuration | `deploy/garage.toml` mounted read-only at `/etc/garage.toml`. |
| Data | Named volume `garage-data:/var/lib/garage`, persisting both `metadata_dir` and `data_dir`. |
| Healthcheck | Runs Garage's own status command (`/garage status`); the `scout` service waits for it. |
| Scout dependency | `scout` depends on `postgres` (healthy) and `garage` (healthy). |
| Provisioning | `--single-node` creates the layout and `--default-bucket` creates the bucket and access key from `GARAGE_DEFAULT_*`. Manual CLI is the fallback for Garage < 2.3.0 or custom layouts. |

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
export SCOUT_S3_ACCESS_KEY="GK$(openssl rand -hex 16)"
export SCOUT_S3_SECRET_KEY="$(openssl rand -hex 32)"
export SCOUT_S3_DEFAULT_BUCKET=scout-raw

docker compose up -d garage postgres
docker compose exec garage /garage status
docker compose exec garage /garage bucket list
```

`--single-node` assigns the node layout and `--default-bucket` creates the bucket and key from the
`GARAGE_DEFAULT_*` values; no manual `layout assign`, `key create`, or `bucket create` step is required
on Garage 2.3.0 or later.

## Manual fallback

For Garage versions before 2.3.0, or a custom layout, run Garage without the flags and provision
manually:

```sh
docker compose exec garage /garage status
docker compose exec garage /garage layout assign -z dc1 -c 1G <node-id>
docker compose exec garage /garage layout apply --version 1
docker compose exec garage /garage key create scout-key
docker compose exec garage /garage bucket create scout-raw
docker compose exec garage /garage bucket allow --read --write --owner scout-raw --key scout-key
```

## Scout environment mapping

| Variable | Value in the shipped stack |
|---|---|
| `SCOUT_S3_ENDPOINT` | `http://garage:3900` |
| `SCOUT_S3_BUCKET` | `SCOUT_S3_DEFAULT_BUCKET` (default `scout-raw`) |
| `SCOUT_S3_DEFAULT_BUCKET` | `scout-raw`; also sets `GARAGE_DEFAULT_BUCKET` |
| `AWS_ACCESS_KEY_ID` | `SCOUT_S3_ACCESS_KEY` (also `GARAGE_DEFAULT_ACCESS_KEY`) |
| `AWS_SECRET_ACCESS_KEY` | `SCOUT_S3_SECRET_KEY` (also `GARAGE_DEFAULT_SECRET_KEY`) |
| `AWS_REGION` | `SCOUT_S3_REGION` (defaults to `garage`) |

Plain HTTP and path-style addressing are used; the shipped stack does not require TLS or wildcard DNS.

## Provider neutrality

The same environment variables configure any S3-compatible provider. Garage is the default and the
documented setup; MinIO and other providers remain supported by supplying their endpoint, bucket, key,
secret, and region. No endpoint is hard-coded in the application.

## What is deliberately not automated

- Production `rpc_secret` and admin token generation.
- Replication and placement policy beyond the single-node development default.
- Custom multi-node layouts and manual provisioning for Garage < 2.3.0.

These remain operator decisions, as recorded in the specification's clarification and assumptions.
