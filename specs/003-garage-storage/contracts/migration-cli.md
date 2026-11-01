# Contract: `scout migrate-objects`

**Feature**: 003-garage-storage | **Status**: design contract

## Invocation

```sh
scout migrate-objects
```

The command is a mode of the Scout binary. It does not start the HTTP server, ingest, workers, or
retention, and it exits when the run finishes. Positional arguments are not accepted; all input comes
from the environment.

## Environment

| Variable | Required | Meaning |
|---|---|---|
| `SCOUT_MIGRATE_SOURCE_ENDPOINT` | yes | Source endpoint, including scheme (`http://` or `https://`). |
| `SCOUT_MIGRATE_SOURCE_BUCKET` | yes | Source bucket. |
| `SCOUT_MIGRATE_SOURCE_ACCESS_KEY_ID` | yes | Source access key. |
| `SCOUT_MIGRATE_SOURCE_SECRET_ACCESS_KEY` | yes | Source secret key. |
| `SCOUT_MIGRATE_SOURCE_REGION` | yes | Source region. |
| `SCOUT_S3_ENDPOINT` | yes | Destination object store (Garage by default) endpoint. |
| `SCOUT_S3_BUCKET` | yes | Destination bucket. |
| `AWS_ACCESS_KEY_ID` | yes | Destination access key. |
| `AWS_SECRET_ACCESS_KEY` | yes | Destination secret key. |
| `AWS_REGION` | yes | Destination region. |
| `RUST_LOG` | no | Log filtering; default `info`. |

A missing source variable fails before listing with a message naming that variable. A missing
destination setting uses the same message style. Secrets are never printed.

## Behaviour

1. Build source and destination clients with path-style addressing; `http://` endpoints allow plain
   HTTP.
2. List every object in the source bucket, in listing order, ignoring keys under the `__scout/` prefix
   (startup-probe sentinels are not payloads and are never counted in `found`).
3. For each object: read the bytes, hash them, compare with the destination copy, and either skip
   (identical), create (absent), or overwrite (different).
4. Re-read the destination and compare hashes before counting the object as verified.
5. Run at most 8 transfers concurrently; list sequentially.
6. Never write or delete anything in the source store.
7. Continue after a failed object so one run reports the whole picture.

## Output

Progress and failures are logged as key/value lines, for example:

```text
migrate: listed key=raw/<project>/<event> size=10240
migrate: verified key=raw/<project>/<event> outcome=copied_new
migrate: failed key=attachments/<project>/<event>/0 outcome=copy_failed reason=destination verification mismatch
migrate: failed key=raw/<project>/<event-b> outcome=unreadable reason=source read failed
migrate: summary found=1200 copied=298 skipped=900 verified=1198 mismatched=1 failed=2 elapsed_ms=42000
```

The summary line is always the last line. `copied` counts only copies that were re-read and verified;
a copy that fails verification is counted in `failed` (and `mismatched`), never in `copied`. `failed`
keys are logged individually; the summary reports the count.

## Exit status

| Status | Meaning |
|---|---|
| `0` | Every listed object is verified (`found == verified`, `failed == 0`). |
| `1` | At least one object failed, or configuration was invalid. The summary names the counts; failures were logged with their keys. |

## Guarantees

- Re-running a completed migration copies nothing and reports the same verified counts.
- An interrupted run leaves only complete objects; the next run resumes by skipping verified copies.
- A destination object with different bytes is repaired; a source object is never modified.
- A destination object that differs but is overwritten and re-read successfully is a success
  (exit `0`); only unreadable objects or post-write verification failures fail the run.
- The command can run while the service is running or stopped; it never contacts the service. The
  operator is responsible for scheduling downtime before the final cutover pass.
